/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.exopackage

import com.facebook.buck.android.ContentDigests
import com.facebook.buck.core.util.log.Logger
import com.facebook.buck.installer.android.AndroidInstallException
import com.google.common.base.Splitter
import com.google.common.collect.ImmutableSortedSet
import com.google.common.collect.Sets
import java.io.File
import java.io.IOException
import java.lang.Thread.sleep
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths
import java.nio.file.StandardCopyOption
import java.util.Optional
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ConcurrentMap
import java.util.regex.Pattern
import kotlin.system.measureTimeMillis

class AndroidDeviceImpl(val serial: String, val adbUtils: AdbUtils) : AndroidDevice {

  private val properties: ConcurrentMap<String, String> = ConcurrentHashMap()

  override fun installApkOnDevice(
      apk: File,
      installViaSd: Boolean,
      quiet: Boolean,
      verifyTempWritable: Boolean,
      stagedInstallMode: Boolean,
      userId: String?,
      allowFastDeploy: Boolean,
      packageName: String,
  ): Boolean = installApkOnDevice(
      apk,
      installViaSd,
      quiet,
      verifyTempWritable,
      stagedInstallMode,
      userId,
      allowFastDeploy,
      packageName,
      false,
  )

  override fun installApkOnDevice(
      apk: File,
      installViaSd: Boolean,
      quiet: Boolean,
      verifyTempWritable: Boolean,
      stagedInstallMode: Boolean,
      userId: String?,
      allowFastDeploy: Boolean,
      packageName: String,
      forceQueryable: Boolean,
  ): Boolean {
    val queryable = forceQueryable && sdkSupportsForceQueryable()
    val elapsed = measureTimeMillis {
      if (verifyTempWritable) {
        try {
          val uniqueFileName = "buck-experiment-${UUID.randomUUID()}"
          executeAdbShellCommand("echo exo > /data/local/tmp/$uniqueFileName")
          executeAdbShellCommand("rm /data/local/tmp/$uniqueFileName")
        } catch (e: AdbCommandFailedException) {
          // TODO: we should check for specific failure here
          LOG.error("Failed to write to /data/local/tmp: ${e.message}")
          throw AndroidInstallException.tempFolderNotWritable()
        }
      }

      var installArgs: String
      // Fast path: use --fastdeploy on SDK-supported devices.
      // On any failure we fall back to a plain install.
      if (allowFastDeploy && !stagedInstallMode && sdkSupportsFastDeploy()) {
        installArgs = buildInstallArgs(apk, true, stagedInstallMode, userId, queryable)
        try {
          executeInstallCatching(
              installArgs,
              "Failed to install ${apk.name} with --fastdeploy.",
          )
          verifyInstalledApkMatches(apk, packageName)
          if (queryable) {
            verifyPackageQueryable(packageName)
          }
          return@measureTimeMillis
        } catch (e: AndroidInstallException) {
          LOG.warn(
              "The fast install failed or left the on-device apk missing or stale: ${e.message}.\n" +
                  "Reinstalling ${apk.name} without --fastdeploy to recover.",
          )
        }
      }

      installArgs = buildInstallArgs(apk, false, stagedInstallMode, userId, queryable)
      try {
        executeInstall(installArgs)
      } catch (e: AdbCommandFailedException) {
        val conflictingPackage = extractSignatureMismatchPackage(e.message)
        if (conflictingPackage == null) {
          throw AndroidInstallException.adbCommandFailedException(
              "Failed to install ${apk.name}.",
              e.message,
          )
        }
        LOG.warn(
            "Install of ${apk.name} failed because $conflictingPackage is already installed with a" +
                " mismatched signature; uninstalling it and retrying the install.",
        )
        executeAdbCommandCatching(
            "uninstall $conflictingPackage",
            "Failed to uninstall $conflictingPackage while recovering from a signature mismatch.",
        )
        executeInstallCatching(
            installArgs,
            "Failed to install ${apk.name} after uninstalling $conflictingPackage.",
        )
      }

      if (!stagedInstallMode) {
        verifyInstalledApkMatches(apk, packageName)
        if (queryable) {
          verifyPackageQueryable(packageName)
        }
      }
    }
    val userSuffix = if (userId != null) " for user $userId" else ""
    val kbps = (apk.length() / 1024.0) / (elapsed / 1000.0)
    LOG.info(
        "Installed ${apk.name}$userSuffix (${apk.length()} bytes) in ${elapsed/1000.0} s ($kbps kB/s)",
    )
    return true
  }

  /**
   * Verifies that the apk installed for [packageName] matches [apk] byte-for-byte (adb stores it
   * verbatim). Always throws [AndroidInstallException] on failure: tagged
   * [AndroidInstallErrorTag.INSTALLED_APK_MISMATCH] if the package is absent or the on-device apk
   * differs, and tagged [AndroidInstallErrorTag.ADB_COMMAND_FAILED] if the on-device apk cannot be
   * read back.
   */
  private fun verifyInstalledApkMatches(apk: File, packageName: String) {
    val installed =
        try {
          compareInstalledApk(packageName, apk, exact = true)
        } catch (e: AdbCommandFailedException) {
          throw AndroidInstallException.adbCommandFailedException(
              "Could not read the on-device apk for $packageName to verify the install of" +
                  " ${apk.name}.",
              e.message,
          )
        }
    when (installed) {
      AndroidDevice.InstalledApk.ABSENT ->
          throw AndroidInstallException.installedApkMismatch(
              "Install of ${apk.name} could not be verified: $packageName is not present on the" +
                  " device after installing.",
          )
      AndroidDevice.InstalledApk.DIFFERENT ->
          throw AndroidInstallException.installedApkMismatch(
              "Install of ${apk.name} could not be verified: the on-device apk for $packageName" +
                  " does not match the local apk after installing.",
          )
      AndroidDevice.InstalledApk.SAME -> Unit
    }
  }

  private fun verifyPackageQueryable(packageName: String) {
    val visible =
        try {
          isPackageQueryable(packageName)
        } catch (e: AdbCommandFailedException) {
          throw AndroidInstallException.adbCommandFailedException(
              "Could not verify package queryability for $packageName.",
              e.message,
          )
        }
    if (!visible) {
      throw AndroidInstallException.adbCommandFailedException(
          "Install did not make $packageName queryable.",
          null,
      )
    }
  }

  @Throws(Exception::class)
  override fun compareInstalledApk(
      packageName: String,
      apk: File,
      exact: Boolean,
  ): AndroidDevice.InstalledApk {
    val installedApkPath =
        getPackageInfo(packageName).map { it.apkPath }.orElse(null)
            ?: return AndroidDevice.InstalledApk.ABSENT
    if (!exact) {
      try {
        val installedDigest = getApkManifestDigest(installedApkPath)
        val localDigest = ContentDigests.getJarManifestDigest(apk.path)
        LOG.info("Installed APK manifest digest: $installedDigest, local: $localDigest")
        return if (installedDigest == localDigest) AndroidDevice.InstalledApk.SAME
        else AndroidDevice.InstalledApk.DIFFERENT
      } catch (e: Exception) {
        // Any failure to get both digests (no `unzip` on the device, an unexpected `.SF`, a local
        // apk without a v1 signature) only means the cheap comparison is unavailable.
        LOG.info("Could not compare APK manifest digests; comparing content hashes: ${e.message}")
      }
    }
    val installedHash = getContentHash(installedApkPath)
    return if (installedHash.equals(ContentDigests.getContentHash(apk), ignoreCase = true))
        AndroidDevice.InstalledApk.SAME
    else AndroidDevice.InstalledApk.DIFFERENT
  }

  @Throws(Exception::class)
  override fun getContentHash(path: String): String {
    val output = executeAdbShellCommand("md5sum $path").trim()
    val hash = output.split(Regex("\\s+")).first()
    // `md5sum` can report an error on stdout (e.g. a missing file) while adb still exits 0, so
    // the first token is not always a digest. Treat any non-hex output as a read failure so the
    // caller surfaces ADB_COMMAND_FAILED rather than a misleading apk mismatch.
    if (!hash.matches(Regex("[0-9a-fA-F]{32}"))) {
      throw AdbCommandFailedException("md5sum returned unexpected output for $path: \"$output\"")
    }
    return hash
  }

  /**
   * Returns the package name from an adb `INSTALL_FAILED_UPDATE_INCOMPATIBLE` failure (signature
   * mismatch), or null if the failure is not a signature mismatch. The package name is parsed from
   * adb's message, e.g. "Existing package com.meta.ar.helixserver signatures do not match ...".
   */
  private fun extractSignatureMismatchPackage(message: String?): String? {
    if (message == null || !message.contains(INSTALL_FAILED_UPDATE_INCOMPATIBLE)) {
      return null
    }
    return SIGNATURE_MISMATCH_PACKAGE_PATTERN.find(message)?.groupValues?.getOrNull(1)?.takeIf {
      PACKAGE_NAME_PATTERN.matches(it)
    }
  }

  private fun buildInstallArgs(
      apk: File,
      fastDeploy: Boolean,
      stagedInstallMode: Boolean,
      userId: String?,
      forceQueryable: Boolean,
  ): String = buildString {
    append("-r -d")
    if (forceQueryable) append(" --force-queryable")
    if (fastDeploy) append(" --fastdeploy")
    if (stagedInstallMode) append(" --staged")
    if (userId != null) append(" --user $userId")
    append(" ${apk.absolutePath}")
  }

  @Throws(Exception::class)
  override fun isPackageQueryable(packageName: String): Boolean {
    require(packageName.matches(Regex("[A-Za-z0-9_]+(?:\\.[A-Za-z0-9_]+)*"))) {
      "Invalid package name: $packageName"
    }
    if (!sdkSupportsForceQueryable()) {
      return true
    }
    val lines = executeAdbShellCommand("dumpsys package queries $packageName").lines()
    val queries = lines.indexOfFirst { it.trim() == "Queries:" }
    if (queries >= 0 && lines.getOrNull(queries + 1)?.trim() == "DISABLED") {
      return true
    }
    val start = lines.indexOfFirst {
      it.trim() == "queries via forceQueryable:" || it.trim() == "forceQueryable:"
    }
    val end = lines.indexOfFirst { it.trim() == "queries via package name:" }
    if (
        start < 0 &&
            queries >= 0 &&
            end > queries &&
            lines.subList(queries + 1, end).any { it.trim().startsWith("system apps queryable:") }
    ) {
      return false
    }
    if (queries < 0 || start <= queries || end <= start) {
      throw AdbCommandFailedException("Could not read the forceQueryable set for $packageName")
    }
    return lines.subList(start + 1, end).any { line ->
      line.trim().removeSurrounding("[", "]").split(',').any { it.trim() == packageName }
    }
  }

  private fun sdkSupportsForceQueryable(): Boolean {
    val sdkVersion =
        try {
          getProperty("ro.build.version.sdk").toInt()
        } catch (e: Exception) {
          throw AndroidInstallException.adbCommandFailedException(
              "Unable to determine SDK version for --force-queryable.",
              e.message,
          )
        }
    return sdkVersion >= MIN_SDK_VERSION_FOR_FORCE_QUERYABLE
  }

  private fun sdkSupportsFastDeploy(): Boolean {
    val sdkVersion =
        try {
          getProperty("ro.build.version.sdk").toInt()
        } catch (e: Exception) {
          LOG.warn("Unable to determine SDK version, defaulting to legacy install: ${e.message}")
          -1
        }

    return sdkVersion >= MIN_SDK_VERSION_FOR_FASTDEPLOY
  }

  override fun prepareForApexInstallation(): Boolean {
    executeAdbCommand("root")
    sleep(5000)
    executeAdbCommand("wait-for-device")

    // Root kills adbd, and sometimes, it takes a while for it to come back
    for (i in 1..3) {
      if (executeAdbShellCommand("whoami").equals("root")) {
        break
      }
      sleep(1000)
    }

    val softRebootAvailable =
        executeAdbShellCommand("pm", ignoreFailure = true).contains("force-non-staged")
    LOG.info("Soft reboot available: $softRebootAvailable")
    return softRebootAvailable
  }

  override fun installApexOnDevice(
      apex: File,
      quiet: Boolean,
      restart: Boolean,
      softRebootAvailable: Boolean,
      waitForDeviceReady: Boolean,
  ): Boolean {
    val elapsed = measureTimeMillis {
      try {
        val installArgs = "--apex ${if (softRebootAvailable) "--force-non-staged" else ""}".trim()
        executeInstall("$installArgs ${apex.absolutePath}")
      } catch (e: AdbCommandFailedException) {
        if ((e.message ?: "").contains("INSTALL_FAILED_VERIFICATION_FAILURE: Staged session ")) {
          throw AndroidInstallException.rebootRequired(
              "Device is already staged; You need to run 'adb reboot' on your device.",
          )
        }

        // if the device can't install because the list of native libs is different,
        // retry without the --force-non-staged flag. Then reboot automatically.
        if (
            (e.message ?: "").contains(
                "INSTALL_FAILED_INTERNAL_ERROR: APEX installation failed: Set of native libs required",
            )
        ) {
          // try install again without --force-non-staged
          executeInstallCatching(
              "-d --apex ${apex.absolutePath}",
              "Failed to install ${apex.name}.",
          )
          throw AndroidInstallException.rebootRequired(
              "Installed ${apex.name} on device; however --force-non-staged doesn't work when the" +
                  " native lib dependencies of an apex have changed. You need to run 'adb" +
                  " reboot' on your device to complete the install. See also:" +
                  " https://www.internalfb.com/intern/wiki/RL/RL_Release_and_Reliability/Build_and_Release_Infra/APEX_in_fbsource/Pit_falls/",
          )
        }

        // If the apex package has changed (e.g. not previously), fall back to
        // remount + push so the apex lands directly in /system/apex.
        if ((e.message ?: "").contains("INSTALL_FAILED_PACKAGE_CHANGED")) {
          LOG.info(
              "INSTALL_FAILED_PACKAGE_CHANGED for ${apex.name}, " +
                  "attempting fallback install via remount and push",
          )
          try {
            // Remount so that we can write to /system_ext/apex
            executeAdbCommand("root")
            executeAdbCommand("wait-for-device")
            executeAdbCommand("remount")

            // Remount sometimes requires a reboot because verity is enabled.
            if (executeAdbShellCommand("getprop ro.boot.veritymode") == "enforcing") {
              executeAdbCommand("reboot")
              executeAdbCommand("wait-for-device")
              waitForBootComplete()

              executeAdbCommand("root")
              executeAdbCommand("wait-for-device")
              executeAdbCommand("remount")
            }

            // Potentially make this customizable in the future.
            executeAdbCommand("push ${apex.absolutePath} /system_ext/apex/")

            // Reboot to activate the apex
            executeAdbCommand("reboot")
            executeAdbCommand("wait-for-device")
            waitForBootComplete()
          } catch (fallbackError: AdbCommandFailedException) {
            throw AndroidInstallException.adbCommandFailedException(
                "Failed to install ${apex.name} via fallback remount+push.",
                fallbackError.message,
            )
          }
          return@measureTimeMillis
        }

        throw AndroidInstallException.adbCommandFailedException(
            "Failed to install ${apex.name}.",
            e.message,
        )
      }

      if (!softRebootAvailable) {
        throw AndroidInstallException.rebootRequired(
            "--force-non-staged is not available on device" +
                "(is the device running an older build?); " +
                "${apex.name} was installed successfully but will not be active until " +
                "you run 'adb reboot' on your device",
        )
      }

      if (restart) {
        try {
          executeAdbShellCommand("stop")
          executeAdbShellCommand("start")

          if (waitForDeviceReady) {
            // Wait for device to be fully ready after soft reboot
            waitForBootComplete()
            waitUntilPackageManagerReady()
            waitForStorageReady()
          }
        } catch (e: AdbCommandFailedException) {
          throw AndroidInstallException.rebootRequired(
              "Failed to stop+start shell; ${apex.name} was installed successfully but device will be in an unknown state until you run 'adb reboot'",
          )
        }
      }
    }
    val kbps = (apex.length() / 1024.0) / (elapsed / 1000.0)
    LOG.info("Installed ${apex.name} (${apex.length()} bytes) in ${elapsed/1000.0} s ($kbps kB/s)")
    return true
  }

  private fun waitForBootComplete() {
    waitForCondition(
        command = "getprop sys.boot_completed",
        condition = { output -> output.trim() == "1" },
        successMessage = "Boot completed after soft reboot",
        timeoutMessage = "Device did not complete boot after soft reboot within timeout",
        timeoutMs = 80000, // Account for Horizon OS Emulator.
    )
  }

  private fun waitUntilPackageManagerReady() {
    waitForCondition(
        command = "pm",
        condition = { output -> output.isNotEmpty() && !output.contains("Can't find service") },
        successMessage = "Package manager service ready",
        timeoutMessage = "Package manager service did not become ready within timeout",
    )
  }

  private fun waitForStorageReady() {
    waitForCondition(
        command = "ls /storage/emulated/0 2>&1 || echo STORAGE_NOT_READY",
        condition = { output ->
          !output.contains("Transport endpoint is not connected") &&
              !output.contains("STORAGE_NOT_READY") &&
              !output.contains("No such file or directory")
        },
        successMessage = "Storage filesystem ready",
        timeoutMessage = "Storage filesystem did not become ready within timeout",
        timeoutMs = 30000,
    )
  }

  /**
   * Polls a shell command until a condition is met or timeout is reached.
   *
   * @param command the shell command to execute
   * @param condition a predicate that returns true when the desired state is reached
   * @param successMessage message to log on success
   * @param timeoutMessage message for the exception if timeout is reached
   * @param timeoutMs maximum time to wait in milliseconds
   * @param pollIntervalMs time between polling attempts in milliseconds
   * @throws AndroidInstallException if the condition is not met within the timeout
   */
  private fun waitForCondition(
      command: String,
      condition: (String) -> Boolean,
      successMessage: String,
      timeoutMessage: String,
      timeoutMs: Long = 10000,
      pollIntervalMs: Long = 100,
  ) {
    LOG.info("Waiting for condition (timeout: ${timeoutMs}ms): $successMessage")
    val startTime = System.currentTimeMillis()
    var attempt = 0

    while (System.currentTimeMillis() - startTime < timeoutMs) {
      attempt++
      if (condition(executeAdbShellCommand(command, ignoreFailure = true))) {
        LOG.info(successMessage)
        return
      }
      LOG.info("Attempt $attempt: condition not met, retrying in ${pollIntervalMs}ms...")
      sleep(pollIntervalMs)
    }

    throw AndroidInstallException.rebootRequired(timeoutMessage)
  }

  override fun stopPackage(packageName: String) {
    executeAdbShellCommandCatching(
        "am force-stop $packageName",
        "Failed to stop package $packageName.",
    )
  }

  @Throws(Exception::class)
  override fun getPackageInfo(packageName: String): Optional<PackageInfo> {
    val output: String =
        try {
          executeAdbShellCommand("pm path $packageName")
        } catch (e: AdbCommandFailedException) {
          LOG.warn("Failed to get package info for $packageName: ${e.message}")
          return Optional.empty()
        }
    // `pm path` prints one `package:<path>` line per installed apk (base plus any config splits),
    // and prints nothing for a package that is not installed. Use the base apk (first line); treat
    // output with no `package:` line as "not installed".
    val apkPath =
        output
            .lineSequence()
            .map { it.trim() }
            .firstOrNull { it.startsWith("package:") }
            ?.removePrefix("package:") ?: return Optional.empty()
    return Optional.of(PackageInfo(apkPath, "", ""))
  }

  @Throws(Exception::class)
  override fun uninstallPackage(packageName: String) {
    executeAdbCommandCatching("uninstall $packageName", "Failed to uninstall $packageName.")
  }

  @Throws(Exception::class)
  override fun getApkManifestDigest(packagePath: String): String {
    val entry: String =
        executeAdbShellCommand("unzip -l $packagePath | grep -E -o 'META-INF/[A-Z]+\\.SF'").trim()
    // API 27 and below ship no `unzip`, and the shell reports that on stdout.
    if (!entry.matches(Regex("META-INF/[A-Z]+\\.SF"))) {
      throw AdbCommandFailedException("No signature file found in $packagePath: \"$entry\"")
    }
    val result: String = executeAdbShellCommand(
        "unzip -p $packagePath $entry | grep -E 'SHA1-Digest-Manifest:|SHA-256-Digest-Manifest:'",
    )
    // The first digest, as `ContentDigests.getJarManifestDigest` reads it locally.
    val digest =
        result
            .lineSequence()
            .firstOrNull { it.isNotBlank() }
            .orEmpty()
            .substringAfter(":", "")
            .trim()
    if (!digest.matches(Regex("[A-Za-z0-9+/]+={0,2}"))) {
      throw AdbCommandFailedException("No manifest digest found in $packagePath: \"$result\"")
    }
    return digest
  }

  @Throws(Exception::class)
  override fun listDirRecursive(root: Path): ImmutableSortedSet<Path> {
    val lsOutput: String =
        executeAdbCommandCatching("shell ls -R $root", "Failed to list path $root.")

    val paths: MutableSet<Path> = HashSet()
    val dirs: MutableSet<Path?> = HashSet()
    var currentDir: Path? = null
    for (line in Splitter.on(LINE_ENDING).omitEmptyStrings().split(lsOutput)) {
      if (line.endsWith(":")) {
        currentDir = root.relativize(Paths.get(line.removeSuffix(":")))
        dirs.add(currentDir)
      } else {
        checkNotNull(currentDir)
        paths.add(currentDir.resolve(line))
      }
    }
    return ImmutableSortedSet.copyOf<Path>(Sets.difference(paths, dirs))
  }

  override fun rmFiles(dirPath: String, filesToDelete: Iterable<String>) {
    val elapsed: Long = measureTimeMillis {
      val tempFile = File.createTempFile("files_to_delete", ".txt")
      try {
        tempFile.writeText(
            filesToDelete.joinToString("\n") { Paths.get(dirPath).resolve(it).toString() },
        )
        executeAdbCommand("push -z brotli ${tempFile.absolutePath} /data/local/tmp")
        executeAdbShellCommand("cat /data/local/tmp/${tempFile.name} | xargs rm -f")
      } catch (e: AdbCommandFailedException) {
        throw AndroidInstallException.adbCommandFailedException(
            "Failed delete ${filesToDelete.count()} files from $dirPath.",
            e.message,
        )
      } finally {
        tempFile.delete()
        executeAdbShellCommand("rm -f /data/local/tmp/${tempFile.name}")
      }
    }
    LOG.info("Deleted ${filesToDelete.count()} files from $dirPath in ${elapsed/1000.0} seconds.")
  }

  @Throws(Exception::class)
  override fun installFiles(
      filesType: String,
      installPaths: Map<Path, Path>,
      packageName: String,
  ) {
    LOG.debug(
        "%s: %s",
        filesType,
        installPaths
            .map { "${it.value.parent} -> ${it.key.parent}" }
            .distinct()
            .joinToString(separator = "\n\t", prefix = "[", postfix = "]"),
    )
    val timeSpent: Long = measureTimeMillis {
      when (filesType) {
        // 1- create a temp folder for each destination folder
        // 2- copy all the files to the temp folder. adb does not support symbolic links so
        // this is necessary
        // 3- push the temp folder to the device using one call to adb. this is much faster
        // than pushing each file individually
        "secondary_dex",
        "native_library",
        "resources" -> {
          // create a temp folder for each destination folder
          val tempFolders = mutableMapOf<Path, Path>()
          val stagingDir = scratchDirFor(packageName)
          var pushFailure: Throwable? = null
          try {
            installPaths.keys
                .stream()
                .map { it.parent }
                .distinct()
                .forEach { tempFolders[it] = Files.createTempDirectory("${it.fileName}_") }
            installPaths.forEach { (destination, source) ->
              val targetPath = tempFolders[destination.parent]?.resolve(destination.fileName)
              stageForPush(source, checkNotNull(targetPath))
            }
            // push the temp folder to the device
            mkDirP(stagingDir)
            tempFolders.forEach { (destination, source) ->
              try {
                executeAdbCommand("push -z brotli $source $stagingDir")
                // In staging, where the glob covers this shard's files and nothing else. The
                // destination holds every shard's, so chmodding there costs the whole directory
                // once per shard. The app will not start if its dex files are writable.
                executeAdbShellCommand("chmod 644 $stagingDir/${source.fileName}/*")
                executeAdbShellCommand("mv $stagingDir/${source.fileName}/* $destination")
              } catch (e: AdbCommandFailedException) {
                throw AndroidInstallException.adbCommandFailedException(
                    "Failed to push $source to $destination.",
                    e.message,
                )
              }
            }
          } catch (t: Throwable) {
            pushFailure = t
            throw t
          } finally {
            // `values`, not `keys`: the keys are on-device destinations.
            tempFolders.values.forEach { it.toFile().deleteRecursively() }
            // Only what this call pushed, since concurrent calls share the package's staging
            // directory.
            if (tempFolders.isNotEmpty()) {
              val pushed = tempFolders.values.joinToString(" ") { "$stagingDir/${it.fileName}" }
              try {
                executeAdbShellCommand("rm -rf $pushed")
              } catch (e: Exception) {
                // Leaving a payload behind is a failure in its own right, but not one worth losing
                // the push failure over: while unwinding, attach it instead of replacing it.
                pushFailure?.addSuppressed(e) ?: throw e
              }
            }
          }
        }
        else -> {
          installPaths.forEach { (destination, source) ->
            LOG.debug("\tPushing $source to $destination")
            executeAdbCommandCatching(
                "push $source $destination",
                "Failed to push $source to $destination.",
            )
            // As for the payloads above: nothing the app reads is left writable. One named file
            // rather than a glob, since these are pushed one at a time.
            executeAdbShellCommandCatching(
                "chmod 644 $destination",
                "Failed to set permissions on $destination.",
            )
          }
        }
      }
    }
    LOG.info("$filesType: Transferred ${installPaths.size} files in ${timeSpent/1000.0} seconds")
  }

  @Throws(Exception::class)
  override fun rmStaleFiles(packageName: String) {
    val scratchDir = scratchDirFor(packageName)
    executeAdbShellCommandCatching("rm -rf $scratchDir", "Failed to remove $scratchDir.")
  }

  /**
   * Where payloads for [packageName] are pushed before being moved into place. Per package, so that
   * reclaiming one app's leftovers cannot destroy a transfer another install has in flight.
   */
  private fun scratchDirFor(packageName: String): String {
    // Interpolated into the adb shell commands that move payloads into place.
    require(PACKAGE_NAME_PATTERN.matches(packageName)) { "Not a package name: $packageName" }
    return "$SCRATCH_ROOT/$packageName"
  }

  @Throws(Exception::class)
  override fun mkDirP(dirpath: String) {
    executeAdbShellCommandCatching(
        "umask 022 && mkdir -p $dirpath",
        "Failed to create dir $dirpath.",
    )
  }

  @Throws(Exception::class)
  override fun getProperty(name: String): String {
    val read = { executeAdbShellCommandCatching("getprop $name", "Failed to get property $name.") }
    // Only `ro.` properties are fixed at boot and so safe to hold on to; anything else can change
    // under us mid-install. Caching is worth it because a single exopackage install otherwise
    // re-queries ro.product.cpu.abilist five times over adb.
    return if (name.startsWith("ro.")) properties.computeIfAbsent(name) { read() } else read()
  }

  /**
   * Hardlinks an artifact into the staging directory, copying only if it cannot be linked -- a
   * different filesystem, typically, when the temp directory is on another volume.
   *
   * Buck materialises exopackage payloads as symlink farms and adb will not follow symlinks, so
   * something has to resolve them; staging also renames each file to the hash-based name it takes
   * on the device. Neither needs the bytes copied, and a payload is several GB.
   *
   * The link shares an inode with the artifact in buck-out, so nothing may modify a staged file.
   * The `chmod` after the push deliberately runs on the device, not here.
   */
  private fun stageForPush(source: Path, target: Path) {
    try {
      Files.createLink(target, source.toRealPath())
    } catch (e: IOException) {
      Files.copy(source, target, StandardCopyOption.REPLACE_EXISTING)
    } catch (e: UnsupportedOperationException) {
      Files.copy(source, target, StandardCopyOption.REPLACE_EXISTING)
    }
  }

  @Throws(Exception::class)
  override fun getDeviceAbis(): List<String> {
    val abiListProperty = getProperty("ro.product.cpu.abilist")
    if (abiListProperty.isEmpty()) {
      throw AndroidInstallException.deviceAbiUnknown()
    }
    // Trimmed like AdbHelper.deviceAbisBySerial: " armeabi-v7a" matches no ABI directory.
    return abiListProperty.split(',').map { it.trim() }.filter { it.isNotEmpty() }
  }

  @Throws(Exception::class)
  override fun killProcess(processName: String) {
    throw AndroidInstallException.operationNotSupported("killProcess")
  }

  override fun getSerialNumber(): String {
    return serial
  }

  @Throws(Exception::class)
  override fun getWindowManagerProperty(propertyName: String): String {
    throw AndroidInstallException.operationNotSupported("getWindowManagerProperty")
  }

  override fun uninstallApkFromDevice(packageName: String, keepData: Boolean): Boolean {
    try {
      executeAdbShellCommand("rm -rf ${ExopackageInstaller.EXOPACKAGE_INSTALL_ROOT}/$packageName")
      executeAdbShellCommand("pm uninstall ${if (keepData) "-k " else ""} $packageName")
      return true
    } catch (e: AdbCommandFailedException) {
      LOG.warn("Failed to uninstall $packageName: ${e.message}")
      return false
    }
  }

  /**
   * Sometimes installation fails and leaves the root directory in a weird state. Adding a cheap
   * fix-root at the beginning to make sure all the folders have access permission.
   */
  override fun fixRootDir(rootDir: String) {
    LOG.info("Fixing root dir $rootDir")
    executeAdbShellCommandCatching(
        "find $rootDir -type d -exec chmod a+x {} +",
        "Failed to fix root dir $rootDir.",
    )
  }

  override fun setDebugAppPackageName(packageName: String?): Boolean {
    if (packageName != null && !isPersistentDebugApp(packageName)) {
      executeAdbShellCommand(adbUtils.getAmSetDebugAppCommand(packageName))
    }
    return true
  }

  /**
   * Whether [packageName] is already the persistent debug app, not waiting for a debugger: the
   * state `am set-debug-app --persistent` writes. That command force-stops the package even when it
   * changes nothing, so it runs only when this is false. A query that adb reports as failed counts
   * as false; if adb cannot run at all, the error propagates, as it would from `am set-debug-app`.
   */
  private fun isPersistentDebugApp(packageName: String): Boolean =
      try {
        readGlobalSetting("debug_app") == packageName &&
            readGlobalSetting("wait_for_debugger") in setOf("0", "null")
      } catch (e: AdbCommandFailedException) {
        LOG.info("Could not read the debug app settings: ${e.message}")
        false
      }

  private fun readGlobalSetting(name: String): String =
      executeAdbShellCommand("settings get global $name").trim()

  override fun enableAppLinks(packageName: String?) {
    if (packageName != null) {
      executeAdbShellCommand("pm set-app-links --package $packageName 1 all")
    }
  }

  override fun getDiskSpace(): List<String> {
    try {
      // API 27 and below ship no `awk`. The toolbox `df` of API 23 and below rejects `-h` as a
      // path, printing an error line ahead of the row. The `/data` row is always the last line.
      val result: String = executeAdbShellCommand("df -h /data")
      return result.lines().last { it.isNotBlank() }.trim().split(Regex("\\s+")).subList(1, 4)
    } catch (e: Exception) {
      LOG.warn("Failed to get disk space: $e")
      return listOf("_", "_", "_")
    }
  }

  override fun isEmulator(): Boolean {
    return isLocalTransport() || getProperty("ro.kernel.qemu") == "1"
  }

  override fun isOnline(): Boolean {
    return getState() == "device"
  }

  private fun getState(): String {
    return try {
      adbUtils.executeAdbCommand("get-state", serialNumber)
    } catch (e: AdbCommandFailedException) {
      // When a device is offline, adb get-state fails with exit code 1.
      // Return "offline" to indicate the device state instead of throwing an exception.
      // This allows the installer to continue with other available devices.
      LOG.warn("Failed to get state for device $serialNumber: ${e.message}")
      "offline"
    }
  }

  /**
   * To be consistent with adb, we treat all local transports (as opposed to USB transports) as
   * emulators instead of devices.
   */
  private fun isLocalTransport(): Boolean {
    /** Pattern that matches Genymotion serial numbers; ex. 127.0.0.1:15562 */
    return Pattern.compile("\\d+\\.\\d+\\.\\d+\\.\\d+:\\d+").matcher(serialNumber).find()
  }

  @Throws(Exception::class)
  override fun installBuildUuidFile(
      dataRoot: Path,
      packageName: String,
      buildUuid: String,
  ): Boolean {
    val destinationPath: String = dataRoot.resolve(packageName).toString()
    try {
      // One `adb shell`, not two, because `umask` is per-process. Split across two shells the
      // second one never runs the umask, so the file its redirect creates takes adbd's default
      // mode rather than 0644. The directory is unaffected either way — the FIRST shell is what
      // creates it — and that asymmetry is what makes this easy to miss: on a host whose default
      // umask is already 022 both land 0644 and the split looks harmless. The file is read back
      // as build provenance, so it should not be writable by anything but the installer.
      executeAdbShellCommand(
          "umask 022 && mkdir -p $destinationPath && " +
              "echo $buildUuid > $destinationPath/build_uuid.txt",
      )
    } catch (e: Exception) {
      // we don't want to fail the install if we can't install the build_uuid.txt file
      LOG.warn("Failed to install build_uuid.txt file on $serial: ${e.message}")
    }
    return true
  }

  override fun deviceStartIntent(intent: AndroidIntent?): String {
    if (intent == null) {
      return ""
    }

    try {
      executeAdbShellCommand(AndroidIntent.getAmStartCommand(intent))
      return ""
    } catch (e: AdbCommandFailedException) {
      throw AndroidInstallException.adbCommandFailedException("Failed to start intent.", e.message)
    }
  }

  private fun executeAdbShellCommandCatching(command: String, message: String): String {
    try {
      return adbUtils.executeAdbShellCommand(command, serialNumber)
    } catch (e: AdbCommandFailedException) {
      throw AndroidInstallException.adbCommandFailedException(message, e.message)
    }
  }

  /**
   * Runs `adb install` with [installArgs] and fails it when the package manager reports failure,
   * which adb does not always turn into a non-zero exit (on API 23 it exits 0). The first verdict
   * line decides: one starting with `Success`, or one that is entirely `Failure [CODE]` or `Failure
   * [CODE: detail]`. Without a verdict line the exit code alone decides, since some versions print
   * none on success and adb adds lines of its own around the verdict.
   */
  private fun executeInstall(installArgs: String): String {
    val command = "install $installArgs"
    val output = executeAdbCommand(command)
    // adb prints a verdict per attempt: a device that refuses an incremental install reports
    // Failure, then adb retries as a streamed install and reports its own verdict. The last one is
    // the outcome.
    val verdict =
        output
            .lineSequence()
            .map { it.trim() }
            .lastOrNull {
              it.startsWith("Success") || INSTALL_FAILURE_LINE.matches(it)
            }
    if (verdict != null && !verdict.startsWith("Success")) {
      throw AdbCommandFailedException(
          "Executing 'adb $command' on $serialNumber reported a failure.\nOutput:\n$output",
      )
    }
    return output
  }

  private fun executeInstallCatching(installArgs: String, message: String): String {
    try {
      return executeInstall(installArgs)
    } catch (e: AdbCommandFailedException) {
      throw AndroidInstallException.adbCommandFailedException(message, e.message)
    }
  }

  private fun executeAdbCommandCatching(command: String, message: String): String {
    try {
      return adbUtils.executeAdbCommand(command, serialNumber)
    } catch (e: AdbCommandFailedException) {
      throw AndroidInstallException.adbCommandFailedException(message, e.message)
    }
  }

  private fun executeAdbShellCommand(command: String, ignoreFailure: Boolean = false): String =
      adbUtils.executeAdbShellCommand(command, serialNumber, ignoreFailure)

  private fun executeAdbCommand(command: String, ignoreFailure: Boolean = false): String =
      adbUtils.executeAdbCommand(command, serialNumber, ignoreFailure)

  companion object {
    private val LINE_ENDING: Pattern = Pattern.compile("\r?\n")
    private val LOG: Logger = Logger.get(AndroidDeviceImpl::class.java.name)

    // --fastdeploy is only supported on Android 10+ (API 29+)
    // https://developer.android.com/tools/releases/platform-tools#2905_october_2019
    private const val MIN_SDK_VERSION_FOR_FASTDEPLOY = 29
    private const val MIN_SDK_VERSION_FOR_FORCE_QUERYABLE = 30

    private const val INSTALL_FAILED_UPDATE_INCOMPATIBLE = "INSTALL_FAILED_UPDATE_INCOMPATIBLE"

    private val INSTALL_FAILURE_LINE = Regex("Failure\\s+\\[(([^:]*)(:.*)?)\\]")

    // Matches the package name in adb's signature-mismatch message, which is phrased as either
    // "Existing package <pkg> signatures do not match ..." or "Package <pkg> signatures do not
    // match ..." depending on the Android version.
    private val SIGNATURE_MISMATCH_PACKAGE_PATTERN =
        Regex("package (\\S+) signatures do not match", RegexOption.IGNORE_CASE)

    private val PACKAGE_NAME_PATTERN = Regex("[\\w.]+")

    // Payloads are pushed here and then moved into place. Keeping them under one directory, rather
    // than loose in /data/local/tmp, is what makes leftovers from an interrupted install
    // identifiable, and so reclaimable.
    private const val SCRATCH_ROOT = "/data/local/tmp/buck-exo-staging"
  }
}
