/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.ksp

import com.facebook.buck.core.build.execution.context.IsolatedExecutionContext
import com.facebook.buck.core.filesystems.AbsPath
import com.facebook.buck.core.filesystems.RelPath
import com.facebook.buck.core.util.log.Logger
import com.facebook.buck.io.file.GlobPatternMatcher
import com.facebook.buck.jvm.cd.command.kotlin.LanguageVersion
import com.facebook.buck.jvm.core.BuildTargetValue
import com.facebook.buck.jvm.java.CompilerOutputPaths
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDAnalytics
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDLoggingContext
import com.facebook.buck.jvm.kotlin.util.getExpandedSourcePaths
import com.facebook.buck.step.StepExecutionResult
import com.facebook.buck.step.StepExecutionResults
import com.facebook.buck.step.isolatedsteps.IsolatedStep
import com.facebook.buck.util.CapturingPrintStream
import com.google.common.base.Joiner
import com.google.common.collect.ImmutableList
import com.google.common.collect.ImmutableSortedSet
import com.google.common.collect.Iterables
import com.google.devtools.ksp.impl.KotlinSymbolProcessing
import com.google.devtools.ksp.processing.KSPJvmConfig
import com.google.devtools.ksp.processing.SymbolProcessorProvider
import java.io.File
import java.io.IOException
import java.io.PrintStream
import java.net.URLClassLoader
import java.nio.charset.StandardCharsets
import java.nio.file.Path
import java.util.Optional
import java.util.ServiceLoader
import java.util.concurrent.TimeUnit
import kotlin.time.measureTimedValue

class Ksp2Step(
    private val invokingRule: BuildTargetValue,
    private val outputPaths: CompilerOutputPaths,
    private val rootPath: AbsPath,
    private val shouldTrackClassUsage: Boolean,
    private val allClasspaths: ImmutableList<AbsPath>,
    private val kotlinPluginGeneratedOutFullPath: String,
    private val annotationProcessorParams: ImmutableSortedSet<String>,
    private val sourceFilePaths: ImmutableSortedSet<RelPath>,
    private val kspDepFilePath: RelPath,
    private val moduleName: String,
    private val kspProcessorsClasspathList: List<String>,
    private val kspClassesOutput: RelPath,
    private val kspKotlinOutput: RelPath,
    private val kspJavaOutput: RelPath,
    private val kspOutputBaseDir: RelPath,
    private val jvmTarget: Optional<String>,
    private val languageVersion: LanguageVersion,
    private val jvmDefaultMode: String,
    private val javaBinary: Optional<String>,
    private val kotlinCDAnalytics: KotlinCDAnalytics,
    private val kspCachesOutput: RelPath,
    private val friendPaths: Collection<AbsPath> = emptyList(),
) : IsolatedStep {

  private val noOpDetector = Ksp2NoOpDetector()

  /** What the processor's [KspInertShortcut] did in this step, for telemetry; null without one. */
  private var inertShortcutResult: String? = null

  @Throws(IOException::class, InterruptedException::class)
  override fun executeIsolatedStep(context: IsolatedExecutionContext): StepExecutionResult {
    CapturingPrintStream().use { stderr ->
      try {
        noOpDetector.stepStarted()
        val (timed, cpu) =
            Ksp2CpuMeter.measure { measureTimedValue { executeKsp2(stderr, context) } }
        val (exitCode, elapsed) = timed
        noOpDetector.stepFinished()
        val durationMs = elapsed.inWholeMilliseconds
        // Same shape KotlincStep already emits, so the two steps are greppable together.
        LOG.info(
            "KOTLINCD_STEP_DURATION|%s|%s|%d|%d",
            invokingRule.fullyQualifiedName,
            this::class.java.simpleName,
            durationMs,
            sourceFilePaths.size,
        )
        kotlinCDAnalytics.log(
            KotlinCDLoggingContext(languageVersion, durationMs).apply {
              addExtras(
                  this@Ksp2Step::class.java.simpleName,
                  "Ksp2 step duration: $durationMs ms",
              )
              addExtras(CPU_EXTRAS_KEY, cpu.toExtrasValue())
              val counts = noOpDetector.countsByProcessor
              if (
                  shouldRecordProcessorCounts(
                      exitCode == KotlinSymbolProcessing.ExitCode.OK,
                      counts.isNotEmpty(),
                  )
              ) {
                addExtras(
                    PROCESSOR_OUTPUT_EXTRAS_KEY,
                    counts.entries.joinToString(",") { "${it.key}=${it.value}" },
                )
                addExtras(
                    PROCESSOR_TIME_EXTRAS_KEY,
                    formatProcessorTimes(noOpDetector.timesByProcessor),
                )
                formatPhases(noOpDetector.phasesMs)?.let { addExtras(PHASE_EXTRAS_KEY, it) }
              }
              inertShortcutResult?.let { addExtras(INERT_SHORTCUT_EXTRAS_KEY, it) }
            },
        )
        return when (exitCode) {
          KotlinSymbolProcessing.ExitCode.OK -> StepExecutionResults.SUCCESS
          KotlinSymbolProcessing.ExitCode.PROCESSING_ERROR ->
              StepExecutionResult(
                  StepExecutionResults.ERROR_EXIT_CODE,
                  Optional.of(stderr.getContentsAsString(StandardCharsets.UTF_8)),
              )
        }
      } catch (e: LinkageError) {
        return StepExecutionResult(
            StepExecutionResults.ERROR_EXIT_CODE,
            Optional.of(
                "${stderr.getContentsAsString(StandardCharsets.UTF_8)}\n${e.stackTraceToString()}For URLClassLoader LinkError similar to P1626402598, try adding affected class to FilteringClassLoader's allowlist. See D63143327",
            ),
        )
      } catch (e: Throwable) {
        return StepExecutionResult(
            StepExecutionResults.ERROR_EXIT_CODE,
            Optional.of(
                "${stderr.getContentsAsString(StandardCharsets.UTF_8)}\n${e.stackTraceToString()}",
            ),
        )
      }
    }
  }

  private fun executeKsp2(
      stdErr: PrintStream,
      context: IsolatedExecutionContext,
  ): KotlinSymbolProcessing.ExitCode {
    val logger = BuckKsp2Logger(stdErr)

    // Load processors
    val processorClassloader: ClassLoader = URLClassLoader(
        kspProcessorsClasspathList.map { File(it).toURI().toURL() }.toTypedArray(),
        filteringClassLoader,
    )
    val processorProviders =
        ServiceLoader.load(
                processorClassloader.loadClass(
                    "com.google.devtools.ksp.processing.SymbolProcessorProvider",
                ),
                processorClassloader,
            )
            .toList() as List<SymbolProcessorProvider>

    // Build processor options
    val apOptions = getApOptions()
    val (kspLibraries, kspFriends) =
        librariesAndFriends(allClasspaths.map { it.toFile() }, friendPaths.map { it.toFile() })

    // Expand source paths
    val sourceFilePathsExpanded = getExpandedSourcePathsOrThrow(
        ruleCellRoot = context.ruleCellRoot,
        kotlinSourceFilePaths = sourceFilePaths,
        ignoredPathMatcher =
            if (invokingRule.isSourceOnlyAbi) GlobPatternMatcher.of("**/kosabi_stub.android.**.kt")
            else null,
        workingDirectory = Optional.of(outputPaths.workingDirectory.getPath()),
        invokingRule = invokingRule,
        logger = logger,
    )
    val sourceFilePathsResolved = sourceFilePathsExpanded.map { rootPath.resolve(it).toFile() }

    val outputDirs =
        KspInertShortcut.OutputDirs(
            rootPath.resolve(kspClassesOutput).toFile(),
            rootPath.resolve(kspKotlinOutput).toFile(),
            rootPath.resolve(kspJavaOutput).toFile(),
        )
    val reportFile = apOptions[FILE_ACCESS_HISTORY_REPORT_OPTION]?.let { File(it) }
    val askStart = System.nanoTime()
    val lookup =
        KspInertShortcut.lookup(
            processorClassloader,
            processorProviders.map { it::class.java.name },
            sourceFilePathsResolved,
            apOptions,
        )
    val askMs = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - askStart)
    if (lookup is KspInertShortcut.Lookup.Failed) {
      inertShortcutResult = "error: ${lookup.error.toString().take(200)}; ask $askMs ms"
      logger.info(
          "KOTLINCD_KSP_INERT_SHORTCUT|error|${invokingRule.fullyQualifiedName}|${lookup.error}",
      )
    }
    val shortcut = (lookup as? KspInertShortcut.Lookup.Found)?.shortcut
    val answer = (lookup as? KspInertShortcut.Lookup.Found)?.answer
    if (shortcut != null && answer != null) {
      val outcome =
          answer.declined?.let { "declined: ${it.take(200)}" } ?: shortcut.mode.name.lowercase()
      inertShortcutResult = "$outcome; ask $askMs ms"
      if (answer.declined == null && shortcut.mode == KspInertShortcut.Mode.SKIP) {
        val written =
            KspInertShortcut.guarded(
                { error ->
                  // `write` is all or nothing: KSP2 runs on the directories as they were.
                  inertShortcutResult = "skip-error: ${error.toString().take(200)}; ask $askMs ms"
                  logger.info(
                      "KOTLINCD_KSP_INERT_SHORTCUT|skip-error|${invokingRule.fullyQualifiedName}|$error",
                  )
                  false
                },
            ) {
              shortcut.write(answer, outputDirs, reportFile)
              true
            }
        if (written) {
          logger.info("KOTLINCD_KSP_INERT_SHORTCUT|skip|${invokingRule.fullyQualifiedName}")
          return KotlinSymbolProcessing.ExitCode.OK
        }
      }
    }
    // Only a comparison (verify or shadow, with an answer) reads the report before KSP2 runs; a
    // failed read is recorded and skips the comparison, never the step.
    var compare =
        shortcut != null &&
            answer != null &&
            answer.declined == null &&
            shortcut.mode != KspInertShortcut.Mode.SKIP
    val reportBefore =
        if (!compare) {
          null
        } else {
          KspInertShortcut.guarded(
              { error ->
                compare = false
                inertShortcutResult =
                    "${shortcut!!.mode.name.lowercase()}-error: report ${error.toString().take(200)}; ask $askMs ms"
                null
              },
          ) {
            reportFile?.takeIf { it.exists() }?.readText(StandardCharsets.UTF_8)
          }
        }

    // Build KSP config
    val kspConfig =
        KSPJvmConfig.Builder()
            .apply {
              // All configurations happen here. See [KSPConfig] for all available options.
              projectBaseDir = rootPath.toFile()
              classOutputDir = rootPath.resolve(kspClassesOutput).toFile()
              kotlinOutputDir = rootPath.resolve(kspKotlinOutput).toFile()
              javaOutputDir = rootPath.resolve(kspJavaOutput).toFile()
              resourceOutputDir = rootPath.resolve(kspClassesOutput).toFile()
              outputBaseDir = rootPath.resolve(kspOutputBaseDir).toFile()
              processorOptions = apOptions
              moduleName = this@Ksp2Step.moduleName
              jvmTarget = this@Ksp2Step.jvmTarget.orElse("1.8")
              sourceRoots = sourceFilePathsResolved
              javaSourceRoots = sourceFilePathsResolved
              languageVersion = this@Ksp2Step.languageVersion.value
              apiVersion = this@Ksp2Step.languageVersion.value
              libraries = kspLibraries
              friends = kspFriends
              jvmDefaultMode = this@Ksp2Step.jvmDefaultMode
              resolveJdkHome(this@Ksp2Step.javaBinary)?.let { jdkHome = it }

              cachesDir = rootPath.resolve(kspCachesOutput).toFile()
            }
            .build()

    logger.info(
        """Running KSP2 with
              |processors: ${processorProviders.joinToString(", ") { it::class.java.name }}
              |KSPJvmConfig: [
              |  projectBaseDir = ${kspConfig.projectBaseDir}
              |  classOutputDir = ${kspConfig.classOutputDir}
              |  kotlinOutputDir = ${kspConfig.kotlinOutputDir}
              |  javaOutputDir = ${kspConfig.javaOutputDir}
              |  resourceOutputDir = ${kspConfig.resourceOutputDir}
              |  cachesDir = ${kspConfig.cachesDir}
              |  outputBaseDir = ${kspConfig.outputBaseDir}
              |  processorOptions = [
              |    ${kspConfig.processorOptions.map { it.key + "=" + it.value }.joinToString(",\n    ")}]
              |  moduleName = ${kspConfig.moduleName}
              |  jvmTarget = ${kspConfig.jvmTarget}
              |  sourceRoots = ${kspConfig.sourceRoots}
              |  javaSourceRoots = ${kspConfig.javaSourceRoots}
              |  languageVersion = ${kspConfig.languageVersion}
              |  apiVersion = ${kspConfig.apiVersion}
              |  libraries = ${kspConfig.libraries}
              |  friends = ${kspConfig.friends}
              |  jvmDefaultMode = ${kspConfig.jvmDefaultMode}
              |  jdkHome = ${kspConfig.jdkHome}
              |]"""
            .trimMargin(),
    )
    // Run!
    val kotlinSymbolProcessing =
        KotlinSymbolProcessing(kspConfig, noOpDetector.wrap(processorProviders), logger)
    val exitCode = kotlinSymbolProcessing.execute()
    if (shortcut == null || shortcut.mode == KspInertShortcut.Mode.SKIP) {
      return exitCode
    }
    if (!compare || answer == null) {
      return exitCode
    }
    val mode = shortcut.mode.name.lowercase()
    if (exitCode != KotlinSymbolProcessing.ExitCode.OK) {
      inertShortcutResult = "$mode-not-compared: KSP exit $exitCode; ask $askMs ms"
      return exitCode
    }
    val compareStart = System.nanoTime()
    val comparison =
        KspInertShortcut.guarded(
            { error ->
              // Only `verify` may fail the step, and only on a real mismatch.
              inertShortcutResult = "$mode-error: ${error.toString().take(200)}; ask $askMs ms"
              logger.info(
                  "KOTLINCD_KSP_INERT_SHORTCUT|compare-error|${invokingRule.fullyQualifiedName}|$error",
              )
              null
            },
        ) {
          shortcut.compare(answer, outputDirs, reportFile, reportBefore)
        } ?: return exitCode
    val compareMs = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - compareStart)
    val timing = "ask $askMs ms; compare $compareMs ms"
    if (comparison.mismatch == null) {
      inertShortcutResult = "$mode-ok; dep file ${comparison.depFile}; $timing"
      return exitCode
    }
    val mismatchLine =
        "${KspInertShortcut.MISMATCH}|${invokingRule.fullyQualifiedName}|${comparison.mismatch}"
    inertShortcutResult = "$mode-mismatch: ${comparison.mismatch.take(300)}; $timing"
    if (shortcut.mode == KspInertShortcut.Mode.VERIFY) {
      stdErr.println(mismatchLine)
      return KotlinSymbolProcessing.ExitCode.PROCESSING_ERROR
    }
    // Shadow: KSP2's own outputs stand; the mismatch is only recorded.
    logger.info(mismatchLine)
    return exitCode
  }

  private fun getApOptions(): MutableMap<String, String> {
    val apOptions = mutableMapOf<String, String>()

    // KSP needs the full classpath in order to resolve resources
    val allClasspath =
        Joiner.on(File.pathSeparator)
            .join(Iterables.transform(allClasspaths) { path: AbsPath -> path.getPath().toString() })
    apOptions["cp"] = allClasspath.replace(',', '-')

    if (shouldTrackClassUsage) {
      apOptions[FILE_ACCESS_HISTORY_REPORT_OPTION] = rootPath.resolve(kspDepFilePath).toString()
    }

    if (invokingRule.isSourceOnlyAbi) {
      apOptions["com.facebook.buck.kotlin.generating_abi"] = "true"
    }

    for (param: String in annotationProcessorParams) {
      val paramAndValue = param.split("=".toRegex()).dropLastWhile { it.isEmpty() }.toTypedArray()
      check(paramAndValue.size == 2)
      apOptions[paramAndValue[0]] = paramAndValue[1]
    }

    apOptions["com.facebook.buck.kotlin.ksp_generated_out_path"] = kotlinPluginGeneratedOutFullPath
    return apOptions
  }

  override fun getIsolatedStepDescription(context: IsolatedExecutionContext): String {
    return "Running ksp2 for $moduleName"
  }

  override fun getShortName(): String {
    return "ksp2"
  }

  private fun getExpandedSourcePathsOrThrow(
      ruleCellRoot: AbsPath,
      kotlinSourceFilePaths: ImmutableSortedSet<RelPath>,
      ignoredPathMatcher: GlobPatternMatcher?,
      workingDirectory: Optional<Path>,
      invokingRule: BuildTargetValue,
      logger: BuckKsp2Logger,
  ) =
      try {
        getExpandedSourcePaths(ruleCellRoot, kotlinSourceFilePaths, workingDirectory).filterNot {
            path: Path ->
          ignoredPathMatcher?.matches(path) ?: false
        }
      } catch (exception: IOException) {
        logger.exception(exception)
        throw RuntimeException(
            "Unable to expand sources for ${invokingRule.fullyQualifiedName} into $workingDirectory",
        )
      }

  companion object {
    /**
     * The KSP2 libraries and friends: the friend jars are passed as friends and left out of the
     * libraries, because a jar that is also a library hides the friend module's `internal`
     * declarations from KSP2.
     */
    @JvmStatic
    fun librariesAndFriends(
        classpath: List<File>,
        friendPaths: List<File>,
    ): Pair<List<File>, List<File>> {
      if (friendPaths.isEmpty()) {
        return classpath to emptyList()
      }
      val friends = friendPaths.toSet()
      return classpath.filterNot { it in friends } to friendPaths
    }

    private val LOG: Logger = Logger.get(Ksp2Step::class.java)

    private const val PROCESSOR_OUTPUT_EXTRAS_KEY = "ksp2_processor_generated_files"

    private const val CPU_EXTRAS_KEY = "ksp2_cpu"

    private const val PROCESSOR_TIME_EXTRAS_KEY = "ksp2_processor_time_ms"

    private const val PHASE_EXTRAS_KEY = "ksp2_phase_ms"

    /**
     * `provider=create;process;finish;rounds` per provider, joined by `,`: the values use `;` so
     * that the pairs split like the generated-file counts.
     */
    @JvmStatic
    fun formatProcessorTimes(times: Map<String, Ksp2NoOpDetector.ProcessorTime>): String =
        times.entries.joinToString(",") { (name, time) ->
          "$name=${time.createMs};${time.processMs};${time.finishMs};${time.rounds}"
        }

    /** `setup=…,processors=…,other=…`, or null when the step's phases were not measured. */
    @JvmStatic
    fun formatPhases(phases: Map<String, Long>): String? =
        phases.takeIf { it.isNotEmpty() }?.entries?.joinToString(",") { "${it.key}=${it.value}" }

    private const val INERT_SHORTCUT_EXTRAS_KEY = "ksp2_inert_shortcut"

    private const val FILE_ACCESS_HISTORY_REPORT_OPTION = "fileAccessHistoryReportFile"

    /**
     * Telemetry records only successful runs that produced counts: an aborted run leaves never-ran
     * processors reading zero.
     */
    fun shouldRecordProcessorCounts(
        succeeded: Boolean,
        hasCounts: Boolean,
    ): Boolean = succeeded && hasCounts

    private val jdkHomeCache = java.util.concurrent.ConcurrentHashMap<String, File>()
    private val JAVA_HOME_REGEX = Regex("""java\.home\s*=\s*(.+)""")

    /**
     * Resolves the JDK home directory from the declared java binary input.
     *
     * KSP2 needs jdkHome to resolve JDK types (java.lang.AutoCloseable, etc.) for JVM targets.
     * Android targets use android.jar instead and do not need jdkHome — returns null when
     * javaBinary is absent so callers can skip setting jdkHome (KSP2 defaults to
     * System.getProperty("java.home")).
     *
     * Similar to compile_kotlin.py's _get_jdk_home() which uses jdk_locator, this invokes the java
     * binary with -XshowSettings:properties to discover java.home at action time. The java binary
     * is a declared Buck2 input (from java_toolchain.java), making this hermetic.
     *
     * Result is cached per java binary path (subprocess runs at most once per distinct binary).
     */
    private fun resolveJdkHome(javaBinary: Optional<String>): File? {
      val binary = javaBinary.filter { it.isNotEmpty() }.orElse(null) ?: return null
      return jdkHomeCache.computeIfAbsent(binary) { resolveJdkHomeUncached(it) }
    }

    private fun resolveJdkHomeUncached(binary: String): File {
      val process =
          ProcessBuilder(binary, "-XshowSettings:properties", "-version")
              .redirectErrorStream(true)
              .start()
      // Drain the pipe in a background thread to avoid deadlock: the process can
      // write more than the OS pipe buffer (~64KB), blocking if no one reads.
      // Meanwhile waitFor() provides the timeout guarantee.
      val outputFuture =
          java.util.concurrent.CompletableFuture.supplyAsync {
            process.inputStream.bufferedReader().readText()
          }
      val completed = process.waitFor(60, java.util.concurrent.TimeUnit.SECONDS)
      if (!completed) {
        process.destroyForcibly()
        throw RuntimeException("Timed out resolving JDK home from java binary: $binary")
      }
      val exitCode = process.exitValue()
      val output = outputFuture.get()
      if (exitCode != 0) {
        throw RuntimeException(
            "java binary exited with code $exitCode: $binary\n" + "Output: ${output.take(500)}",
        )
      }
      val match = JAVA_HOME_REGEX.find(output)
      val path =
          match?.groupValues?.get(1)?.trim()
              ?: throw RuntimeException(
                  "Could not parse java.home from java binary output: $binary\n" +
                      "Output (first 500 chars): ${output.take(500)}",
              )
      return File(path)
    }

    private val filteringClassLoader = FilteringClassLoader(
        this::class.java.classLoader,
        ClassLoader.getPlatformClassLoader(),
        "com.google.devtools.ksp.",
        "kotlin.",
        "ksp.",
    )
  }
}
