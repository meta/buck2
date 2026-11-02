/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.aapt

import com.facebook.buck.android.aapt.RDotTxtEntry.RType
import com.facebook.buck.core.filesystems.AbsPath
import com.facebook.buck.core.filesystems.RelPath
import com.facebook.buck.io.pathformat.PathFormatter
import com.google.common.collect.ImmutableMap
import com.google.common.collect.ImmutableSet
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths
import java.util.Locale
import java.util.TreeSet
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import org.kohsuke.args4j.CmdLineParser
import org.kohsuke.args4j.Option

/** Builds source provenance for the resources packaged into a Robolectric test. */
object ResourceSourceMapExecutableMain {
  private const val MAX_RESOURCE_WORKERS = 8

  @JvmStatic
  fun main(args: Array<String>) {
    val fallbackOutput = outputPath(args)
    try {
      run(args)
    } catch (failure: Throwable) {
      if (failure is VirtualMachineError || failure is ThreadDeath) throw failure
      System.err.println(
          "Robolectric resource source-map generation failed; " +
              "continuing coverage with resource attribution disabled.",
      )
      failure.printStackTrace(System.err)
      val output = fallbackOutput ?: throw failure
      try {
        output.parent?.let(Files::createDirectories)
        Files.writeString(output, "", StandardCharsets.UTF_8)
      } catch (writeFailure: Throwable) {
        writeFailure.addSuppressed(failure)
        throw writeFailure
      }
    }
  }

  private fun run(args: Array<String>) {
    val options = Options()
    CmdLineParser(options).parseArgument(*args)
    generate(options.resourceDirs, options.assetDirs, options.output)
  }

  private fun outputPath(args: Array<String>): Path? {
    val index = args.indexOf("--output")
    if (index >= 0 && index + 1 < args.size) return Paths.get(args[index + 1])
    val inlineValue = args.firstOrNull { it.startsWith("--output=") }?.substringAfter('=')
    return inlineValue?.takeIf(String::isNotEmpty)?.let(Paths::get)
  }

  internal fun generate(
      resourceDirsFile: Path,
      assetDirsFile: Path,
      output: Path,
      root: Path = Paths.get(".").toAbsolutePath().normalize(),
  ) {
    val resourceRoots = readResourceRoots(resourceDirsFile)
    val assetRoots = readRoots(assetDirsFile)
    val lines = TreeSet<String>()

    val resourceFiles = resourceRoots.flatMapIndexed { priority, input ->
      val resourceDir = resolve(root, input.path)
      val sourceResolver = input.sourcePath?.let { SourceFileResolver(resolve(root, it)) }
      filesUnder(resourceDir).mapNotNull { (relative, file) ->
        if (relative.nameCount < 2) {
          null
        } else {
          ResourceFile(relative, file, sourceResolver, input.ownerBuildFile, priority)
        }
      }
    }
    collectResources(root, resourceFiles, lines)
    assetRoots.forEachIndexed { priority, input ->
      collectAssets(root, input, priority, lines)
    }
    output.parent?.let(Files::createDirectories)
    val contents = lines.joinToString(separator = "\n", postfix = if (lines.isEmpty()) "" else "\n")
    Files.writeString(output, contents, StandardCharsets.UTF_8)
  }

  private fun readRoots(path: Path): List<InputRoot> =
      Files.readAllLines(path, StandardCharsets.UTF_8).mapNotNull { line ->
        if (line.isBlank()) {
          null
        } else {
          val parts = line.split('\t', limit = 2)
          InputRoot(Paths.get(parts[0]), null, parts.getOrElse(1) { "" })
        }
      }

  private fun readResourceRoots(path: Path): List<InputRoot> =
      Files.readAllLines(path, StandardCharsets.UTF_8).mapNotNull { line ->
        if (line.isBlank()) {
          null
        } else {
          val parts = line.split('\t', limit = 3)
          when (parts.size) {
            1 -> InputRoot(Paths.get(parts[0]), null, "")
            2 -> InputRoot(Paths.get(parts[0]), null, parts[1])
            else -> InputRoot(Paths.get(parts[0]), Paths.get(parts[1]), parts[2])
          }
        }
      }

  private fun collectResources(
      root: Path,
      files: List<ResourceFile>,
      output: MutableSet<String>,
  ) {
    val (valuesFiles, otherFiles) =
        files.partition {
          val directory = it.relative.getName(0).toString()
          directory == "values" || directory.startsWith("values-")
        }
    // MiniAapt uses shared XPathExpression instances for non-values XML, and XPathExpression is
    // not thread-safe. Values parsing does not use those expressions, so only values files run in
    // parallel.
    val workerCount = minOf(MAX_RESOURCE_WORKERS, Runtime.getRuntime().availableProcessors())
    if (valuesFiles.size > 1 && workerCount > 1) {
      val executor = Executors.newFixedThreadPool(workerCount)
      try {
        executor
            .invokeAll(valuesFiles.map { file -> Callable { resourceLinesOrEmpty(root, file) } })
            .forEach { output.addAll(it.get()) }
      } finally {
        executor.shutdownNow()
      }
    } else {
      valuesFiles.forEach { output.addAll(resourceLinesOrEmpty(root, it)) }
    }
    otherFiles.forEach { output.addAll(resourceLinesOrEmpty(root, it)) }
  }

  private fun resourceLinesOrEmpty(root: Path, file: ResourceFile): List<String> {
    val sourceFiles =
        file.sourceResolver?.resolve(file.relative).orEmpty().ifEmpty { listOf(file.path) }
    try {
      return resourceLines(root, file, sourceFiles)
    } catch (failure: Throwable) {
      if (failure is VirtualMachineError || failure is ThreadDeath) throw failure
      System.err.println(
          "Failed to map Robolectric resource file ${file.path}; skipping this file.",
      )
      failure.printStackTrace(System.err)
      return emptyList()
    }
  }

  private fun resourceLines(root: Path, file: ResourceFile, sourceFiles: List<Path>): List<String> {
    val miniAapt = MiniAapt(ImmutableSet.of())
    miniAapt.processAllFilesIncludingExcludedFromBuckResourceMap(
        ImmutableMap.of(file.relative, file.path),
    )
    val sources = sourceFiles.map { sourcePath(root, it, file.ownerBuildFile) }.distinct()
    val qualifiers = qualifiers(file.relative.getName(0).toString())
    return miniAapt.resourceCollector.resources.flatMap { entry ->
      // Styleables are generated arrays and indices, not runtime resource-table entries. MiniAapt
      // emits their underlying attributes separately as R.attr entries.
      if (entry.type == RType.STYLEABLE) {
        emptyList()
      } else {
        sources.map { source ->
          ResourceSourceMapFormat.ResourceRow(
                  file.priority,
                  entry.type.name.lowercase(Locale.ROOT),
                  entry.name,
                  qualifiers,
                  source,
                  file.ownerBuildFile,
              )
              .serialize()
        }
      }
    }
  }

  private fun collectAssets(
      root: Path,
      input: InputRoot,
      priority: Int,
      output: MutableSet<String>,
  ) {
    val assetDir = resolve(root, input.path)
    val assetFiles =
        try {
          filesUnder(assetDir)
        } catch (failure: Throwable) {
          if (failure is VirtualMachineError || failure is ThreadDeath) throw failure
          System.err.println(
              "Failed to enumerate Robolectric asset directory $assetDir; skipping this directory.",
          )
          failure.printStackTrace(System.err)
          return
        }
    for ((relative, file) in assetFiles) {
      try {
        output.add(
            ResourceSourceMapFormat.AssetRow(
                    priority,
                    PathFormatter.pathWithUnixSeparators(relative),
                    sourcePath(root, file, input.ownerBuildFile),
                    input.ownerBuildFile,
                )
                .serialize(),
        )
      } catch (failure: Throwable) {
        if (failure is VirtualMachineError || failure is ThreadDeath) throw failure
        System.err.println(
            "Failed to map Robolectric asset file $file; skipping this file.",
        )
        failure.printStackTrace(System.err)
      }
    }
  }

  private fun filesUnder(directory: Path): List<Map.Entry<Path, Path>> =
      MiniAapt.getAllResourceFiles(AbsPath.of(directory), RelPath.get("")).entries.sortedBy {
        PathFormatter.pathWithUnixSeparators(it.key)
      }

  private fun resolve(root: Path, path: Path): Path =
      if (path.isAbsolute) path.normalize() else root.resolve(path).normalize()

  private fun sourcePath(root: Path, path: Path, ownerBuildFile: String): String {
    val realPath = path.toRealPath()
    if (realPath.startsWith(root)) {
      val relative = PathFormatter.pathWithUnixSeparators(root.relativize(realPath))
      if (!relative.startsWith("buck-out/")) return relative
    }
    return ownerBuildFile
  }

  private fun qualifiers(resourceDirectory: String): String =
      resourceDirectory.substringAfter('-', missingDelimiterValue = "")

  private fun qualifierParts(resourceDirectory: String): Set<String> =
      qualifiers(resourceDirectory).takeIf(String::isNotEmpty)?.split('-')?.toSet() ?: emptySet()

  private class SourceFileResolver(private val root: Path) {
    private val candidatesByResource: Map<ResourceKey, List<SourceCandidate>> by lazy {
      filesUnder(root)
          .mapNotNull { (relative, path) -> sourceCandidate(relative, path) }
          .groupBy({ it.first }, { it.second })
    }

    fun resolve(relative: Path): List<Path> {
      val exact = root.resolve(relative)
      if (Files.exists(exact)) return listOf(exact)

      val generated = resourceIdentity(relative) ?: return emptyList()
      val candidates = candidatesByResource[generated.key] ?: return emptyList()
      val ranked = candidates.mapNotNull { candidate ->
        if (generated.qualifiers.containsAll(candidate.qualifiers)) {
          candidate to candidate.qualifiers.size
        } else {
          null
        }
      }
      val bestScore = ranked.maxOfOrNull { it.second } ?: return emptyList()
      return ranked.filter { it.second == bestScore }.map { it.first.path }.distinct()
    }

    private fun sourceCandidate(relative: Path, path: Path): Pair<ResourceKey, SourceCandidate>? {
      val identity = resourceIdentity(relative) ?: return null
      return identity.key to SourceCandidate(identity.qualifiers, path)
    }

    private fun resourceIdentity(relative: Path): ResourceIdentity? {
      if (relative.nameCount < 2) return null
      val resourceDirectory = relative.getName(0).toString()
      val type = resourceDirectory.substringBefore('-')
      if (type == "values") return null
      val name = relative.fileName.toString().substringBefore('.')
      if (name.isEmpty()) return null
      return ResourceIdentity(ResourceKey(type, name), qualifierParts(resourceDirectory))
    }
  }

  private data class ResourceKey(val type: String, val name: String)

  private data class ResourceIdentity(val key: ResourceKey, val qualifiers: Set<String>)

  private data class SourceCandidate(val qualifiers: Set<String>, val path: Path)

  private data class InputRoot(
      val path: Path,
      val sourcePath: Path?,
      val ownerBuildFile: String,
  )

  private data class ResourceFile(
      val relative: Path,
      val path: Path,
      val sourceResolver: SourceFileResolver?,
      val ownerBuildFile: String,
      val priority: Int,
  )

  private class Options {
    @field:Option(name = "--resource-dirs", required = true) lateinit var resourceDirs: Path

    @field:Option(name = "--asset-dirs", required = true) lateinit var assetDirs: Path

    @field:Option(name = "--output", required = true) lateinit var output: Path
  }
}
