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

import com.facebook.buck.installer.android.AndroidInstallErrorClassifier
import com.facebook.buck.installer.android.AndroidInstallErrorTag
import com.facebook.buck.installer.android.AndroidInstallException
import java.io.File
import java.nio.file.Files
import java.nio.file.Paths
import java.security.MessageDigest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import org.mockito.kotlin.any
import org.mockito.kotlin.argThat
import org.mockito.kotlin.doAnswer
import org.mockito.kotlin.doReturn
import org.mockito.kotlin.eq
import org.mockito.kotlin.inOrder
import org.mockito.kotlin.mock
import org.mockito.kotlin.never
import org.mockito.kotlin.times
import org.mockito.kotlin.verify
import org.mockito.kotlin.whenever

class AndroidDeviceImplTest {

  private lateinit var mockAdbUtils: AdbUtils
  private lateinit var androidDevice: AndroidDeviceImpl
  private lateinit var apkFile: File
  private val serialNumber = "test-serial"
  private val packageName = "com.test.app"
  private val onDeviceApkPath = "/data/app/com.test.app-1/base.apk"

  @Before
  fun setUp() {
    mockAdbUtils = mock()
    androidDevice = AndroidDeviceImpl(serialNumber, mockAdbUtils)
    // A real file is needed because installApkOnDevice hashes the local apk to verify the install.
    val tempDir = Files.createTempDirectory("android-device-impl-test").toFile()
    apkFile = File(tempDir, "test.apk")
    apkFile.writeText("test apk contents")
  }

  /** Stubs the post-install verification so the on-device apk matches [apkFile]. */
  private fun stubInstallVerified(sdk: String = "34") {
    whenever(mockAdbUtils.executeAdbShellCommand("pm path $packageName", serialNumber))
        .thenReturn("package:$onDeviceApkPath")
    whenever(mockAdbUtils.executeAdbShellCommand("md5sum $onDeviceApkPath", serialNumber))
        .thenReturn("${md5Hex(apkFile)}  $onDeviceApkPath")
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "dumpsys package queries $packageName",
            serialNumber,
        ),
    )
        .thenReturn(
            if (sdk in listOf("30", "31", "32")) {
              "Queries:\n  system apps queryable: false\n  forceQueryable:\n    $packageName\n  queries via package name:\n"
            } else {
              "Queries:\n  queries via forceQueryable:\n    forceQueryable:\n      $packageName\n  queries via package name:\n"
            },
        )
  }

  private fun md5Hex(file: File): String =
      MessageDigest.getInstance("MD5").digest(file.readBytes()).joinToString("") {
        "%02x".format(it.toInt() and 0xFF)
      }

  @Test
  fun testInstallApkOnDevice() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("28")
    whenever(mockAdbUtils.executeAdbCommand("install -r -d ${apkFile.absolutePath}", serialNumber))
        .thenReturn("Success")
    stubInstallVerified()

    // Test with verifyTempWritable = true, SDK < 29 (no fastdeploy)
    val result = androidDevice.installApkOnDevice(apkFile, false, false, true, false, packageName)

    // Verify temp file check uses UUID pattern
    verify(mockAdbUtils)
        .executeAdbShellCommand(
            argThat { matches(Regex("echo exo > /data/local/tmp/buck-experiment-[0-9a-f\\-]+")) },
            org.mockito.kotlin.eq(serialNumber),
            org.mockito.kotlin.any(),
        )
    verify(mockAdbUtils)
        .executeAdbShellCommand(
            argThat { matches(Regex("rm /data/local/tmp/buck-experiment-[0-9a-f\\-]+")) },
            org.mockito.kotlin.eq(serialNumber),
            org.mockito.kotlin.any(),
        )

    verify(mockAdbUtils).executeAdbCommand("install -r -d ${apkFile.absolutePath}", serialNumber)
    assertTrue(result)
  }

  @Test
  fun testInstallApkOnDeviceWithInvalidSdkVersion() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("invalid")
    whenever(mockAdbUtils.executeAdbCommand("install -r -d ${apkFile.absolutePath}", serialNumber))
        .thenReturn("Success")
    stubInstallVerified()

    // Test with invalid SDK version (should fall back to no fastdeploy)
    val result = androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName)

    verify(mockAdbUtils).executeAdbCommand("install -r -d ${apkFile.absolutePath}", serialNumber)
    assertTrue(result)
  }

  @Test
  fun testForceQueryableOnApi30() {
    assertForceQueryableInstall("30", " --force-queryable")
  }

  @Test
  fun testForceQueryableOnApi34() {
    assertForceQueryableInstall("34", " --force-queryable")
  }

  @Test
  fun testForceQueryableOmittedOnApi29() {
    assertForceQueryableInstall("29", "")
  }

  private fun assertForceQueryableInstall(sdk: String, queryableArg: String) {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn(sdk)
    val command = "install -r -d$queryableArg --fastdeploy ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(any(), eq(serialNumber), any())).thenReturn("Success")
    whenever(mockAdbUtils.executeAdbCommand(command, serialNumber)).thenReturn("Success")
    stubInstallVerified(sdk)

    assertTrue(
        androidDevice.installApkOnDevice(
            apkFile,
            false,
            false,
            false,
            false,
            null,
            true,
            packageName,
            true,
        ),
    )

    verify(mockAdbUtils).executeAdbCommand(command, serialNumber)
  }

  @Test
  fun testForceQueryableWithStagedInstallAndUser() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("34")
    val command = "install -r -d --force-queryable --staged --user 10 ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(command, serialNumber)).thenReturn("Success")

    assertTrue(
        androidDevice.installApkOnDevice(
            apkFile,
            false,
            false,
            false,
            true,
            "10",
            true,
            packageName,
            true,
        ),
    )

    verify(mockAdbUtils).executeAdbCommand(command, serialNumber)
  }

  @Test
  fun testForceQueryableRetainedOnSignatureMismatchRetry() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("34")
    val command = "install -r -d --force-queryable ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(command, serialNumber))
        .thenReturn(
            "Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE: Existing package $packageName " +
                "signatures do not match newer version]",
        )
        .thenReturn("Success")
    stubInstallVerified()

    assertTrue(
        androidDevice.installApkOnDevice(
            apkFile,
            false,
            false,
            false,
            false,
            null,
            false,
            packageName,
            true,
        ),
    )

    verify(mockAdbUtils, times(2)).executeAdbCommand(command, serialNumber)
    verify(mockAdbUtils).executeAdbCommand("uninstall $packageName", serialNumber)
  }

  @Test
  fun testForceQueryableFailsBeforeInstallingWhenSdkIsUnknown() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("invalid")
    try {
      androidDevice.installApkOnDevice(
          apkFile,
          false,
          false,
          false,
          false,
          null,
          true,
          packageName,
          true,
      )
      fail("Expected unknown SDK to fail an explicit force-queryable install")
    } catch (e: AndroidInstallException) {
      assertTrue(e.message!!.contains("Unable to determine SDK version for --force-queryable"))
    }
    verify(mockAdbUtils, never()).executeAdbCommand(any(), eq(serialNumber), any())
  }

  @Test
  fun testForceQueryableFallsBackWhenFastdeployLeavesOnlyTheMatchingApk() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("34")
    val fast = "install -r -d --force-queryable --fastdeploy ${apkFile.absolutePath}"
    val plain = "install -r -d --force-queryable ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(fast, serialNumber))
        .thenReturn("PM Create Session Failed")
    whenever(mockAdbUtils.executeAdbCommand(plain, serialNumber)).thenReturn("Success")
    stubInstallVerified()
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "dumpsys package queries $packageName",
            serialNumber,
        ),
    )
        .thenReturn("Queries:\n  queries via forceQueryable:\n  queries via package name:\n")
        .thenReturn(
            "Queries:\n  queries via forceQueryable:\n    forceQueryable:\n      $packageName\n  queries via package name:\n",
        )

    assertTrue(
        androidDevice.installApkOnDevice(
            apkFile,
            false,
            false,
            false,
            false,
            null,
            true,
            packageName,
            true,
        ),
    )

    verify(mockAdbUtils).executeAdbCommand(fast, serialNumber)
    verify(mockAdbUtils).executeAdbCommand(plain, serialNumber)
    verify(mockAdbUtils, times(2))
        .executeAdbShellCommand("dumpsys package queries $packageName", serialNumber)
  }

  @Test
  fun testForceQueryableFallsBackWhenFastdeployVisibilityIsUnreadable() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("34")
    val fast = "install -r -d --force-queryable --fastdeploy ${apkFile.absolutePath}"
    val plain = "install -r -d --force-queryable ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(fast, serialNumber)).thenReturn("Success")
    whenever(mockAdbUtils.executeAdbCommand(plain, serialNumber)).thenReturn("Success")
    stubInstallVerified()
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "dumpsys package queries $packageName",
            serialNumber,
        ),
    )
        .thenReturn("Permission Denial")
        .thenReturn(
            "Queries:\n  queries via forceQueryable:\n    forceQueryable:\n      $packageName\n  queries via package name:\n",
        )

    assertTrue(
        androidDevice.installApkOnDevice(
            apkFile,
            false,
            false,
            false,
            false,
            null,
            true,
            packageName,
            true,
        ),
    )

    verify(mockAdbUtils).executeAdbCommand(fast, serialNumber)
    verify(mockAdbUtils).executeAdbCommand(plain, serialNumber)
  }

  @Test
  fun testForceQueryableFailsIfPlainInstallDoesNotApplyVisibility() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("34")
    val plain = "install -r -d --force-queryable ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(plain, serialNumber)).thenReturn("Success")
    stubInstallVerified()
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "dumpsys package queries $packageName",
            serialNumber,
        ),
    )
        .thenReturn("Queries:\n  queries via forceQueryable:\n  queries via package name:\n")

    try {
      androidDevice.installApkOnDevice(
          apkFile,
          false,
          false,
          false,
          false,
          null,
          false,
          packageName,
          true,
      )
      fail("Expected missing queryability to fail")
    } catch (e: AndroidInstallException) {
      assertTrue(e.message!!.contains("did not make $packageName queryable"))
    }
  }

  @Test
  fun testForceQueryableFailsIfVisibilityCannotBeVerified() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("34")
    val plain = "install -r -d --force-queryable ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(plain, serialNumber)).thenReturn("Success")
    stubInstallVerified()
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "dumpsys package queries $packageName",
            serialNumber,
        ),
    )
        .thenReturn("Permission Denial")

    try {
      androidDevice.installApkOnDevice(
          apkFile,
          false,
          false,
          false,
          false,
          null,
          false,
          packageName,
          true,
      )
      fail("Expected unreadable queryability to fail")
    } catch (e: AndroidInstallException) {
      assertTrue(e.message!!.contains("Could not verify package queryability"))
    }
  }

  @Test
  fun testPackageQueryableUsesTheEffectiveSet() {
    stubQueryableOutput("    $packageName\n")
    assertTrue(androidDevice.isPackageQueryable(packageName))
  }

  @Test
  fun testApi30LegacyQueryDumpFindsQueryablePackage() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("30")
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "dumpsys package queries $packageName",
            serialNumber,
        ),
    )
        .thenReturn(
            "Queries:\n  system apps queryable: false\n  forceQueryable:\n    $packageName\n  queries via package name:\n",
        )
    assertTrue(androidDevice.isPackageQueryable(packageName))
  }

  @Test
  fun testApi30LegacyEmptyQuerySetNeedsAnOverride() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("30")
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "dumpsys package queries $packageName",
            serialNumber,
        ),
    )
        .thenReturn("Queries:\n  system apps queryable: false\n  queries via package name:\n")
    assertFalse(androidDevice.isPackageQueryable(packageName))
  }

  @Test
  fun testPackageQueryableRecognizesSharedUidMembers() {
    stubQueryableOutput("    [com.other.app,$packageName]\n")
    assertTrue(androidDevice.isPackageQueryable(packageName))
  }

  @Test
  fun testPackageQueryableDoesNotMatchPackagePrefixes() {
    stubQueryableOutput("    $packageName.extra\n")
    assertFalse(androidDevice.isPackageQueryable(packageName))
  }

  @Test
  fun testPackageQueryableDoesNotMatchAnotherQueriesSection() {
    stubQueryableOutput("", "    com.other.app:\n      $packageName\n")
    assertFalse(androidDevice.isPackageQueryable(packageName))
  }

  @Test
  fun testEmptyForceQueryableSetIsNotQueryable() {
    stubQueryableOutput("")
    assertFalse(androidDevice.isPackageQueryable(packageName))
  }

  @Test
  fun testPackageQueryabilityIsNotCachedAcrossInstalls() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("34")
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "dumpsys package queries $packageName",
            serialNumber,
        ),
    )
        .thenReturn("Queries:\n  queries via forceQueryable:\n  queries via package name:\n")
        .thenReturn(
            "Queries:\n  queries via forceQueryable:\n    forceQueryable:\n      $packageName\n  queries via package name:\n",
        )
    assertFalse(androidDevice.isPackageQueryable(packageName))
    assertTrue(androidDevice.isPackageQueryable(packageName))
  }

  @Test
  fun testApi29DoesNotNeedQueryableMetadata() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("29")
    assertTrue(androidDevice.isPackageQueryable(packageName))
    verify(mockAdbUtils, never())
        .executeAdbShellCommand(argThat { startsWith("dumpsys") }, eq(serialNumber), any())
  }

  @Test
  fun testDisabledPackageFilteringIsQueryable() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("34")
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "dumpsys package queries $packageName",
            serialNumber,
        ),
    )
        .thenReturn("Queries:\n  DISABLED\n")
    assertTrue(androidDevice.isPackageQueryable(packageName))
  }

  @Test
  fun testMalformedQueryableOutputIsNotSuccessfulVisibility() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("34")
    for (output in
        listOf(
            "",
            "Permission Denial",
            "queries via forceQueryable:\n    $packageName\n  queries via package name:\n",
            "Queries:\n  queries via forceQueryable:\n    $packageName",
        )) {
      whenever(
          mockAdbUtils.executeAdbShellCommand(
              "dumpsys package queries $packageName",
              serialNumber,
          ),
      )
          .thenReturn(output)
      try {
        androidDevice.isPackageQueryable(packageName)
        fail("Expected malformed queryability output to fail")
      } catch (e: AdbCommandFailedException) {
        assertTrue(e.message!!.contains("Could not read the forceQueryable set"))
      }
    }
  }

  @Test
  fun testQueryableTransportFailureIsPropagated() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("34")
    doAnswer { throw AdbCommandFailedException("transport failed") }
        .whenever(mockAdbUtils)
        .executeAdbShellCommand("dumpsys package queries $packageName", serialNumber)
    try {
      androidDevice.isPackageQueryable(packageName)
      fail("Expected transport failure")
    } catch (e: AdbCommandFailedException) {
      assertEquals("transport failed", e.message)
    }
  }

  @Test
  fun testQueryablePackageNameRejectsShellCharacters() {
    try {
      androidDevice.isPackageQueryable("com.test.app; touch /tmp/invalid")
      fail("Expected invalid package name")
    } catch (e: IllegalArgumentException) {
      assertTrue(e.message!!.contains("Invalid package name"))
    }
    verify(mockAdbUtils, never()).executeAdbShellCommand(any(), eq(serialNumber), any())
  }

  private fun stubQueryableOutput(forceQueryableRows: String, packageQueries: String = "") {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("34")
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "dumpsys package queries $packageName",
            serialNumber,
        ),
    )
        .thenReturn(
            "Queries:\n  queries via forceQueryable:\n    forceQueryable:\n$forceQueryableRows  queries via package name:\n$packageQueries",
        )
  }

  @Test
  fun testDefaultApi34InstallStillUsesFastDeployWithoutForceQueryable() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("34")
    val command = "install -r -d --fastdeploy ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(command, serialNumber)).thenReturn("Success")
    stubInstallVerified()

    assertTrue(androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName))

    verify(mockAdbUtils).executeAdbCommand(command, serialNumber)
    verify(mockAdbUtils, never())
        .executeAdbCommand(
            argThat { contains("--force-queryable") },
            eq(serialNumber),
            any(),
        )
  }

  @Test
  fun testInstallApexOnDeviceWithRestart() {
    val apexFile = mock<File>()
    whenever(apexFile.absolutePath).thenReturn("/path/to/test.apex")
    whenever(apexFile.name).thenReturn("test.apex")
    whenever(apexFile.length()).thenReturn(1024L)

    // Mock boot completion check
    whenever(mockAdbUtils.executeAdbShellCommand("getprop sys.boot_completed", serialNumber, true))
        .thenReturn("1")
    // Mock package manager ready check
    whenever(mockAdbUtils.executeAdbShellCommand("pm", serialNumber, true))
        .thenReturn("Package manager is ready")
    // Mock storage ready check
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "ls /storage/emulated/0 2>&1 || echo STORAGE_NOT_READY",
            serialNumber,
            true,
        ),
    )
        .thenReturn("Android\nDownload\nPictures")

    whenever(
        mockAdbUtils.executeAdbCommand(
            "install --apex --force-non-staged /path/to/test.apex",
            serialNumber,
        ),
    )
        .thenReturn("Success")
    val result = androidDevice.installApexOnDevice(apexFile, false, true, true, true)

    verify(mockAdbUtils)
        .executeAdbCommand(
            "install --apex --force-non-staged ${apexFile.absolutePath}",
            serialNumber,
        )
    verify(mockAdbUtils).executeAdbShellCommand("stop", serialNumber)
    verify(mockAdbUtils).executeAdbShellCommand("start", serialNumber)
    verify(mockAdbUtils).executeAdbShellCommand("getprop sys.boot_completed", serialNumber, true)
    verify(mockAdbUtils).executeAdbShellCommand("pm", serialNumber, true)
    verify(mockAdbUtils)
        .executeAdbShellCommand(
            "ls /storage/emulated/0 2>&1 || echo STORAGE_NOT_READY",
            serialNumber,
            true,
        )
    assertTrue(result)
  }

  @Test
  fun testInstallApexOnDeviceWithRestartWithoutWaitingForPackageManagerReady() {
    val apexFile = mock<File>()
    whenever(apexFile.absolutePath).thenReturn("/path/to/test.apex")
    whenever(apexFile.name).thenReturn("test.apex")
    whenever(apexFile.length()).thenReturn(1024L)

    whenever(
        mockAdbUtils.executeAdbCommand(
            "install --apex --force-non-staged /path/to/test.apex",
            serialNumber,
        ),
    )
        .thenReturn("Success")
    val result = androidDevice.installApexOnDevice(apexFile, false, true, true, false)

    verify(mockAdbUtils)
        .executeAdbCommand(
            "install --apex --force-non-staged ${apexFile.absolutePath}",
            serialNumber,
        )
    verify(mockAdbUtils).executeAdbShellCommand("stop", serialNumber)
    verify(mockAdbUtils).executeAdbShellCommand("start", serialNumber)
    verify(mockAdbUtils, never())
        .executeAdbShellCommand("getprop sys.boot_completed", serialNumber, true)
    verify(mockAdbUtils, never()).executeAdbShellCommand("pm", serialNumber, true)
    verify(mockAdbUtils, never())
        .executeAdbShellCommand(
            "ls /storage/emulated/0 2>&1 || echo STORAGE_NOT_READY",
            serialNumber,
            true,
        )
    assertTrue(result)
  }

  @Test
  fun testInstallApexOnDeviceWithoutRestart() {
    val apexFile = mock<File>()
    whenever(apexFile.absolutePath).thenReturn("/path/to/test.apex")
    whenever(apexFile.name).thenReturn("test.apex")
    whenever(apexFile.length()).thenReturn(1024L)

    whenever(
        mockAdbUtils.executeAdbCommand(
            "install --apex --force-non-staged /path/to/test.apex",
            serialNumber,
        ),
    )
        .thenReturn("Success")
    val result = androidDevice.installApexOnDevice(apexFile, false, false, true, false)

    verify(mockAdbUtils)
        .executeAdbCommand(
            "install --apex --force-non-staged ${apexFile.absolutePath}",
            serialNumber,
        )
    // Verify that stop/start and boot checks are NOT called
    verify(mockAdbUtils, never()).executeAdbShellCommand("stop", serialNumber)
    verify(mockAdbUtils, never()).executeAdbShellCommand("start", serialNumber)
    verify(mockAdbUtils, never())
        .executeAdbShellCommand("getprop sys.boot_completed", serialNumber, true)
    assertTrue(result)
  }

  @Test
  fun testStopPackage() {
    androidDevice.stopPackage("com.test.app")

    verify(mockAdbUtils).executeAdbShellCommand("am force-stop com.test.app", serialNumber)
  }

  private val debugAppQuery = "settings get global debug_app"
  private val waitForDebuggerQuery = "settings get global wait_for_debugger"
  private val setDebugApp = "am set-debug-app --persistent com.test.app "

  private fun stubDebugAppSettings(debugApp: String, waitForDebugger: String) {
    whenever(mockAdbUtils.getAmSetDebugAppCommand("com.test.app")).thenReturn(setDebugApp)
    whenever(mockAdbUtils.executeAdbShellCommand(debugAppQuery, serialNumber)).thenReturn(debugApp)
    whenever(mockAdbUtils.executeAdbShellCommand(waitForDebuggerQuery, serialNumber))
        .thenReturn(waitForDebugger)
  }

  @Test
  fun testSetDebugAppSkippedWhenAlreadyTheDebugApp() {
    stubDebugAppSettings("com.test.app\n", "0\n")

    androidDevice.setDebugAppPackageName("com.test.app")

    verify(mockAdbUtils, never()).executeAdbShellCommand(setDebugApp, serialNumber)
  }

  @Test
  fun testSetDebugAppRunsWhenTheSettingOutputIsUnexpected() {
    stubDebugAppSettings("\ncom.test.app\n0\n", "0\n")

    androidDevice.setDebugAppPackageName("com.test.app")

    verify(mockAdbUtils).executeAdbShellCommand(setDebugApp, serialNumber)
  }

  @Test
  fun testSetDebugAppRunsWhenAnotherAppIsTheDebugApp() {
    stubDebugAppSettings("com.other.app\n", "0\n")

    androidDevice.setDebugAppPackageName("com.test.app")

    verify(mockAdbUtils).executeAdbShellCommand(setDebugApp, serialNumber)
    verify(mockAdbUtils, never()).executeAdbShellCommand(waitForDebuggerQuery, serialNumber)
  }

  @Test
  fun testSetDebugAppRunsWhenWaitingForADebugger() {
    stubDebugAppSettings("com.test.app\n", "1\n")

    androidDevice.setDebugAppPackageName("com.test.app")

    verify(mockAdbUtils).executeAdbShellCommand(setDebugApp, serialNumber)
  }

  @Test
  fun testSetDebugAppRunsWhenTheSettingsCannotBeRead() {
    stubDebugAppSettings("", "")
    doAnswer { throw AdbCommandFailedException("settings: not found") }
        .whenever(mockAdbUtils)
        .executeAdbShellCommand(debugAppQuery, serialNumber)

    androidDevice.setDebugAppPackageName("com.test.app")

    verify(mockAdbUtils).executeAdbShellCommand(setDebugApp, serialNumber)
  }

  private val unzipList = "unzip -l $onDeviceApkPath | grep -E -o 'META-INF/[A-Z]+\\.SF'"

  @Test
  fun testCompareInstalledApkReportsAnAbsentPackage() {
    whenever(mockAdbUtils.executeAdbShellCommand("pm path $packageName", serialNumber))
        .thenReturn("")

    assertEquals(
        AndroidDevice.InstalledApk.ABSENT,
        androidDevice.compareInstalledApk(packageName, apkFile, false),
    )
  }

  @Test
  fun testCompareInstalledApkExactComparesContentHashOnly() {
    stubInstallVerified()

    assertEquals(
        AndroidDevice.InstalledApk.SAME,
        androidDevice.compareInstalledApk(packageName, apkFile, true),
    )
    verify(mockAdbUtils, never()).executeAdbShellCommand(unzipList, serialNumber)
  }

  @Test
  fun testCompareInstalledApkFallsBackToContentHashWhenTheLocalApkHasNoSignatureFile() {
    stubInstallVerified()
    whenever(mockAdbUtils.executeAdbShellCommand(unzipList, serialNumber))
        .thenReturn("META-INF/CERT.SF")
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "unzip -p $onDeviceApkPath META-INF/CERT.SF | grep -E " +
                "'SHA1-Digest-Manifest:|SHA-256-Digest-Manifest:'",
            serialNumber,
        ),
    )
        .thenReturn("SHA-256-Digest-Manifest: aGFzaA==")

    assertEquals(
        AndroidDevice.InstalledApk.SAME,
        androidDevice.compareInstalledApk(packageName, apkFile, false),
    )
  }

  @Test
  fun testCompareInstalledApkFallsBackToContentHashWithoutUnzip() {
    whenever(mockAdbUtils.executeAdbShellCommand("pm path $packageName", serialNumber))
        .thenReturn("package:$onDeviceApkPath")
    whenever(mockAdbUtils.executeAdbShellCommand(unzipList, serialNumber))
        .thenReturn("/system/bin/sh: unzip: not found")
    whenever(mockAdbUtils.executeAdbShellCommand("md5sum $onDeviceApkPath", serialNumber))
        .thenReturn("${"0".repeat(32)}  $onDeviceApkPath")

    assertEquals(
        AndroidDevice.InstalledApk.DIFFERENT,
        androidDevice.compareInstalledApk(packageName, apkFile, false),
    )
  }

  @Test
  fun testGetPackageInfo() {
    whenever(mockAdbUtils.executeAdbShellCommand("pm path com.test.app", serialNumber))
        .thenReturn("package:/data/app/com.test.app-1/base.apk")

    val result = androidDevice.getPackageInfo("com.test.app")

    assertTrue(result.isPresent)
    assertEquals("/data/app/com.test.app-1/base.apk", result.get().apkPath)
  }

  @Test
  fun testUninstallPackage() {
    androidDevice.uninstallPackage("com.test.app")

    verify(mockAdbUtils).executeAdbCommand("uninstall com.test.app", serialNumber)
  }

  @Test
  fun testGetApkManifestDigest() {
    val packagePath = "/data/app/com.test.app-1/base.apk"
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "unzip -l $packagePath | grep -E -o 'META-INF/[A-Z]+\\.SF'",
            serialNumber,
        ),
    )
        .thenReturn("META-INF/CERT.SF")
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "unzip -p $packagePath META-INF/CERT.SF | grep -E 'SHA1-Digest-Manifest:|SHA-256-Digest-Manifest:'",
            serialNumber,
        ),
    )
        .thenReturn("SHA1-Digest-Manifest: abcdef1234567890")

    val result = androidDevice.getApkManifestDigest(packagePath)

    assertEquals("abcdef1234567890", result)
  }

  @Test
  fun testGetApkManifestDigestWithTwoDigests() {
    val packagePath = "/data/app/com.test.app-1/base.apk"
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "unzip -l $packagePath | grep -E -o 'META-INF/[A-Z]+\\.SF'",
            serialNumber,
        ),
    )
        .thenReturn("META-INF/CERT.SF")
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "unzip -p $packagePath META-INF/CERT.SF | grep -E 'SHA1-Digest-Manifest:|SHA-256-Digest-Manifest:'",
            serialNumber,
        ),
    )
        .thenReturn("SHA1-Digest-Manifest: abcd1234=\nSHA-256-Digest-Manifest: efgh5678+/==\n")

    assertEquals("abcd1234=", androidDevice.getApkManifestDigest(packagePath))
  }

  /** API 27 and below ship no `unzip`; the shell's complaint must not pass for a digest. */
  @Test
  fun testGetApkManifestDigestWithoutUnzip() {
    val packagePath = "/data/app/com.test.app-1/base.apk"
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "unzip -l $packagePath | grep -E -o 'META-INF/[A-Z]+\\.SF'",
            serialNumber,
        ),
    )
        .thenReturn("/system/bin/sh: unzip: not found")

    try {
      androidDevice.getApkManifestDigest(packagePath)
      fail("Expected AdbCommandFailedException")
    } catch (e: AdbCommandFailedException) {
      assertTrue(e.message!!.contains("unzip: not found"))
    }
  }

  @Test
  fun testGetSerialNumber() {
    assertEquals(serialNumber, androidDevice.getSerialNumber())
  }

  @Test
  fun testIsEmulator() {
    // A device caches the read-only properties it reads, so each case needs its own instance.
    // Setup for non-emulator
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.kernel.qemu", serialNumber))
        .thenReturn("0")

    assertFalse(AndroidDeviceImpl(serialNumber, mockAdbUtils).isEmulator)

    // Setup for emulator
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.kernel.qemu", serialNumber))
        .thenReturn("1")

    assertTrue(AndroidDeviceImpl(serialNumber, mockAdbUtils).isEmulator)

    // Setup for Genymotion device
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.kernel.qemu", serialNumber))
        .thenReturn("0")
    assertTrue(AndroidDeviceImpl("192.168.57.101:5555", mockAdbUtils).isEmulator)
  }

  @Test
  fun testReadOnlyPropertiesAreOnlyQueriedOnce() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.product.cpu.abilist", serialNumber))
        .thenReturn("arm64-v8a")

    androidDevice.getDeviceAbis()
    androidDevice.getDeviceAbis()
    androidDevice.getProperty("ro.product.cpu.abilist")

    verify(mockAdbUtils, times(1))
        .executeAdbShellCommand("getprop ro.product.cpu.abilist", serialNumber)
  }

  /** Anything outside `ro.` can change mid-install, so it must be read through every time. */
  @Test
  fun testMutablePropertiesAreNotCached() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop sys.boot_completed", serialNumber))
        .thenReturn("1")

    androidDevice.getProperty("sys.boot_completed")
    androidDevice.getProperty("sys.boot_completed")

    verify(mockAdbUtils, times(2))
        .executeAdbShellCommand("getprop sys.boot_completed", serialNumber)
  }

  @Test
  fun testGetDeviceAbis() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.product.cpu.abilist", serialNumber))
        .thenReturn("arm64-v8a,armeabi-v7a,armeabi")

    val result = androidDevice.getDeviceAbis()

    assertEquals(listOf("arm64-v8a", "armeabi-v7a", "armeabi"), result)
  }

  @Test
  fun testGetDeviceAbisTrimsTheAbiList() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.product.cpu.abilist", serialNumber))
        .thenReturn("arm64-v8a, armeabi-v7a, armeabi")

    assertEquals(listOf("arm64-v8a", "armeabi-v7a", "armeabi"), androidDevice.getDeviceAbis())
  }

  @Test
  fun testGetDeviceAbisWithoutAbiListThrows() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.product.cpu.abilist", serialNumber))
        .thenReturn("")
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.product.cpu.abi", serialNumber))
        .thenReturn("x86")
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.product.cpu.abi2", serialNumber))
        .thenReturn("")

    try {
      androidDevice.getDeviceAbis()
      fail("Expected AndroidInstallException")
    } catch (e: AndroidInstallException) {
      assertEquals(setOf(AndroidInstallErrorTag.UNKNOWN_DEVICE_ABI), e.installError.tags)
    }
    verify(mockAdbUtils, never()).executeAdbShellCommand("getprop ro.product.cpu.abi", serialNumber)
  }

  @Test
  fun testGetDiskSpace() {
    whenever(
        mockAdbUtils.executeAdbShellCommand(
            "df -h /data",
            serialNumber,
        ),
    )
        .thenReturn(
            "Filesystem Size Used Avail Use% Mounted on\n/dev/block/dm-0 64G 32G 32G 50% /data",
        )

    val result = androidDevice.getDiskSpace()

    assertEquals(listOf("64G", "32G", "32G"), result)
  }

  /**
   * Toolbox `df` of API 23 and below treats `-h` as a path and prints an error ahead of the row.
   */
  @Test
  fun testGetDiskSpaceToolboxDf() {
    whenever(mockAdbUtils.executeAdbShellCommand("df -h /data", serialNumber))
        .thenReturn(
            "Filesystem Size Used Free Blksize\n" +
                "-h: No such file or directory\n" +
                "/data 3.9G 187.5M 3.7G 4096\n",
        )

    val result = androidDevice.getDiskSpace()

    assertEquals(listOf("3.9G", "187.5M", "3.7G"), result)
  }

  @Test
  fun testInstallApexFallbackOnPackageChanged() {
    val apexFile = mock<File>()
    whenever(apexFile.absolutePath).thenReturn("/path/to/test.apex")
    whenever(apexFile.name).thenReturn("test.apex")
    whenever(apexFile.length()).thenReturn(1024L)

    // First install attempt fails with INSTALL_FAILED_PACKAGE_CHANGED
    doAnswer { throw AdbCommandFailedException("INSTALL_FAILED_PACKAGE_CHANGED") }
        .whenever(mockAdbUtils)
        .executeAdbCommand(
            eq("install --apex --force-non-staged /path/to/test.apex"),
            eq(serialNumber),
            any(),
        )

    // Stub adb commands used in the fallback path
    doReturn("").whenever(mockAdbUtils).executeAdbCommand(eq("root"), eq(serialNumber), any())
    doReturn("")
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq("wait-for-device"), eq(serialNumber), any())
    doReturn("remount succeeded")
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq("remount"), eq(serialNumber), any())
    doReturn("")
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq("push /path/to/test.apex /system_ext/apex/"), eq(serialNumber), any())
    doReturn("").whenever(mockAdbUtils).executeAdbCommand(eq("reboot"), eq(serialNumber), any())

    // Verity is not enabled — no intermediate reboot needed
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.boot.veritymode", serialNumber))
        .thenReturn("disabled")

    // Mock boot completion check for final reboot
    whenever(mockAdbUtils.executeAdbShellCommand("getprop sys.boot_completed", serialNumber, true))
        .thenReturn("1")

    val result = androidDevice.installApexOnDevice(apexFile, false, false, true, false)
    assertTrue(result)

    val inOrder = inOrder(mockAdbUtils)
    // Original install attempt
    inOrder
        .verify(mockAdbUtils)
        .executeAdbCommand(
            eq("install --apex --force-non-staged /path/to/test.apex"),
            eq(serialNumber),
            any(),
        )
    // Step 1: root, wait-for-device, remount
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("root"), eq(serialNumber), any())
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("wait-for-device"), eq(serialNumber), any())
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("remount"), eq(serialNumber), any())
    // Step 5: push apex
    inOrder
        .verify(mockAdbUtils)
        .executeAdbCommand(eq("push /path/to/test.apex /system_ext/apex/"), eq(serialNumber), any())
    // Step 6: reboot
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("reboot"), eq(serialNumber), any())
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("wait-for-device"), eq(serialNumber), any())
  }

  @Test
  fun testInstallApexFallbackOnPackageChangedWithVerityReboot() {
    val apexFile = mock<File>()
    whenever(apexFile.absolutePath).thenReturn("/path/to/test.apex")
    whenever(apexFile.name).thenReturn("test.apex")
    whenever(apexFile.length()).thenReturn(1024L)

    // First install attempt fails with INSTALL_FAILED_PACKAGE_CHANGED
    doAnswer { throw AdbCommandFailedException("INSTALL_FAILED_PACKAGE_CHANGED") }
        .whenever(mockAdbUtils)
        .executeAdbCommand(
            eq("install --apex --force-non-staged /path/to/test.apex"),
            eq(serialNumber),
            any(),
        )

    // Stub adb commands used in the fallback path
    doReturn("").whenever(mockAdbUtils).executeAdbCommand(eq("root"), eq(serialNumber), any())
    doReturn("")
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq("wait-for-device"), eq(serialNumber), any())
    doReturn("remount succeeded")
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq("remount"), eq(serialNumber), any())
    doReturn("")
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq("push /path/to/test.apex /system_ext/apex/"), eq(serialNumber), any())
    doReturn("").whenever(mockAdbUtils).executeAdbCommand(eq("reboot"), eq(serialNumber), any())

    // Verity is enabled — triggers intermediate reboot
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.boot.veritymode", serialNumber))
        .thenReturn("enforcing")

    // Mock boot completion check
    whenever(mockAdbUtils.executeAdbShellCommand("getprop sys.boot_completed", serialNumber, true))
        .thenReturn("1")

    val result = androidDevice.installApexOnDevice(apexFile, false, false, true, false)
    assertTrue(result)

    val inOrder = inOrder(mockAdbUtils)
    // Original install attempt
    inOrder
        .verify(mockAdbUtils)
        .executeAdbCommand(
            eq("install --apex --force-non-staged /path/to/test.apex"),
            eq(serialNumber),
            any(),
        )
    // Step 1: root, wait-for-device, remount
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("root"), eq(serialNumber), any())
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("wait-for-device"), eq(serialNumber), any())
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("remount"), eq(serialNumber), any())
    // Steps 2-3: verity reboot cycle
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("reboot"), eq(serialNumber), any())
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("wait-for-device"), eq(serialNumber), any())
    // Step 4: root, wait-for-device, remount again after reboot
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("root"), eq(serialNumber), any())
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("wait-for-device"), eq(serialNumber), any())
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("remount"), eq(serialNumber), any())
    // Step 5: push apex
    inOrder
        .verify(mockAdbUtils)
        .executeAdbCommand(eq("push /path/to/test.apex /system_ext/apex/"), eq(serialNumber), any())
    // Step 6: final reboot
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("reboot"), eq(serialNumber), any())
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq("wait-for-device"), eq(serialNumber), any())
  }

  @Test
  fun testInstallApexFallbackOnPackageChangedPushFails() {
    val apexFile = mock<File>()
    whenever(apexFile.absolutePath).thenReturn("/path/to/test.apex")
    whenever(apexFile.name).thenReturn("test.apex")
    whenever(apexFile.length()).thenReturn(1024L)

    // First install attempt fails with INSTALL_FAILED_PACKAGE_CHANGED
    doAnswer { throw AdbCommandFailedException("INSTALL_FAILED_PACKAGE_CHANGED") }
        .whenever(mockAdbUtils)
        .executeAdbCommand(
            eq("install --apex --force-non-staged /path/to/test.apex"),
            eq(serialNumber),
            any(),
        )

    // Stub adb commands
    doReturn("").whenever(mockAdbUtils).executeAdbCommand(eq("root"), eq(serialNumber), any())
    doReturn("")
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq("wait-for-device"), eq(serialNumber), any())
    doReturn("remount succeeded")
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq("remount"), eq(serialNumber), any())

    // Verity not enabled
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.boot.veritymode", serialNumber))
        .thenReturn("disabled")

    // Push fails
    doAnswer { throw AdbCommandFailedException("push failed: no space left on device") }
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq("push /path/to/test.apex /system_ext/apex/"), eq(serialNumber), any())

    try {
      androidDevice.installApexOnDevice(apexFile, false, false, true, false)
      fail("Expected AndroidInstallException")
    } catch (e: AndroidInstallException) {
      assertTrue(e.message!!.contains("fallback remount+push"))
    }
  }

  @Test
  fun testInstallApkFailsWhenThePackageManagerReportsFailureButAdbExitsZero() {
    val installCommand = "install -r -d ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(installCommand, serialNumber))
        .thenReturn(
            "Performing Streamed Install\nFailure [INSTALL_FAILED_INSUFFICIENT_STORAGE]\n",
        )

    try {
      androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName)
      fail("Expected AndroidInstallException")
    } catch (e: AndroidInstallException) {
      assertTrue(e.message!!.contains("Failed to install test.apk"))
      assertEquals(setOf(AndroidInstallErrorTag.NO_SPACE_LEFT_ON_DEVICE), e.installError.tags)
    }
    verify(mockAdbUtils, never()).executeAdbShellCommand("md5sum $onDeviceApkPath", serialNumber)
    verify(mockAdbUtils, never())
        .executeAdbCommand(argThat { startsWith("uninstall") }, eq(serialNumber), any())
  }

  @Test
  fun testInstallApkSucceedsWhenAdbFallsBackFromARefusedIncrementalInstall() {
    val installCommand = "install -r -d ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(installCommand, serialNumber))
        .thenReturn(
            "Serving...\nPerforming Incremental Install\n" +
                "Failure [INSTALL_FAILED_SESSION_INVALID: Incremental installation of this " +
                "package is not allowed.]\nPerforming Streamed Install\nSuccess\n",
        )
    stubInstallVerified()

    assertTrue(
        androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName),
    )
  }

  @Test
  fun testInstallApkFailsWhenTheStreamedFallbackAlsoFails() {
    val installCommand = "install -r -d ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(installCommand, serialNumber))
        .thenReturn(
            "Performing Incremental Install\n" +
                "Failure [INSTALL_FAILED_SESSION_INVALID: Incremental installation of this " +
                "package is not allowed.]\nPerforming Streamed Install\n" +
                "Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE]\n",
        )

    try {
      androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName)
      fail("Expected AndroidInstallException")
    } catch (e: AndroidInstallException) {
      assertEquals(setOf(AndroidInstallErrorTag.NO_SPACE_LEFT_ON_DEVICE), e.installError.tags)
    }
  }

  @Test
  fun testInstallApkRecoversFromSignatureMismatchReportedWithExitZero() {
    val installCommand = "install -r -d ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(installCommand, serialNumber))
        .thenReturn(
            "Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE: Package com.meta.ar.helixserver " +
                "signatures do not match previously installed version; ignoring!]",
            "Success",
        )
    doReturn("Success")
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq("uninstall com.meta.ar.helixserver"), eq(serialNumber), any())
    stubInstallVerified()

    assertTrue(
        androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName),
    )
    verify(mockAdbUtils)
        .executeAdbCommand(eq("uninstall com.meta.ar.helixserver"), eq(serialNumber), any())
  }

  @Test
  fun testInstallApkWithoutAVerdictLineIsCheckedByVerification() {
    val installCommand = "install -r -d ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(installCommand, serialNumber))
        .thenReturn(
            "pkg: /data/local/tmp/Failure [not a verdict].apk\nPerforming Streamed Install\n",
        )
    stubInstallVerified()

    assertTrue(
        androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName),
    )
  }

  @Test
  fun testInstallApkRecoversFromSignatureMismatch() {
    val installCommand = "install -r -d ${apkFile.absolutePath}"

    // First install attempt fails with a signature mismatch; the retry (after uninstall) succeeds.
    var installAttempts = 0
    doAnswer {
          installAttempts++
          if (installAttempts == 1) {
            throw AdbCommandFailedException(
                "Executing 'adb $installCommand' on $serialNumber failed with code 1.\nError:\n" +
                    "Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE: Existing package " +
                    "com.meta.ar.helixserver signatures do not match newer version; ignoring!]",
            )
          }
          "Success"
        }
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq(installCommand), eq(serialNumber), any())

    doReturn("Success")
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq("uninstall com.meta.ar.helixserver"), eq(serialNumber), any())
    stubInstallVerified()

    val result = androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName)
    assertTrue(result)

    val inOrder = inOrder(mockAdbUtils)
    // Original failing install attempt.
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq(installCommand), eq(serialNumber), any())
    // Uninstall of the conflicting package.
    inOrder
        .verify(mockAdbUtils)
        .executeAdbCommand(eq("uninstall com.meta.ar.helixserver"), eq(serialNumber), any())
    // Retried install.
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq(installCommand), eq(serialNumber), any())
  }

  @Test
  fun testInstallApkClassifiesInsufficientStorageWithoutUninstalling() {
    val installCommand = "install -r -d ${apkFile.absolutePath}"
    doAnswer {
          throw AdbCommandFailedException(
              "Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE: Not enough space]",
          )
        }
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq(installCommand), eq(serialNumber), any())

    try {
      androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName)
      fail("Expected AndroidInstallException")
    } catch (e: AndroidInstallException) {
      assertTrue(e.message!!.contains("Failed to install test.apk"))
      assertTrue(e.message!!.contains("NO_SPACE_LEFT_ON_DEVICE"))
    }

    // Classification must not make the low-level installer uninstall implicitly.
    verify(mockAdbUtils, never())
        .executeAdbCommand(argThat { startsWith("uninstall") }, eq(serialNumber), any())
  }

  @Test
  fun testClassifiesAndroidInsufficientStorageErrorCode() {
    val error =
        AndroidInstallErrorClassifier.fromErrorMessage(
            "Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE: Failed to override installation location]",
        )

    assertEquals(setOf(AndroidInstallErrorTag.NO_SPACE_LEFT_ON_DEVICE), error.tags)
  }

  @Test
  fun testClassifiesQuotaExceededAsInsufficientStorage() {
    val error =
        AndroidInstallErrorClassifier.fromErrorMessage(
            "ADB_COMMAND_FAILED: Failed to create dir /data/local/tmp/exopackage/com.whatsapp..\n" +
                "mkdir: '/data/local/tmp/exopackage/com.whatsapp': Quota exceeded",
        )

    assertEquals(setOf(AndroidInstallErrorTag.NO_SPACE_LEFT_ON_DEVICE), error.tags)
    assertTrue(error.message.contains("/data/local/tmp/exopackage"))
  }

  @Test
  fun testInstallApkUsesFastdeployOnModernSdk() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("29")
    whenever(
        mockAdbUtils.executeAdbCommand(
            "install -r -d --fastdeploy ${apkFile.absolutePath}",
            serialNumber,
        ),
    )
        .thenReturn("Success")
    stubInstallVerified()

    val result = androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName)

    verify(mockAdbUtils)
        .executeAdbCommand("install -r -d --fastdeploy ${apkFile.absolutePath}", serialNumber)
    assertTrue(result)
  }

  @Test
  fun testFastInstallFallsBackToPlainWhenApkStaleDespiteSuccess() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("29")
    // --fastdeploy reports success, but the on-device apk does not match (a stale apk). This must
    // fall back to a plain install, after which the apk matches.
    whenever(
        mockAdbUtils.executeAdbCommand(
            "install -r -d --fastdeploy ${apkFile.absolutePath}",
            serialNumber,
        ),
    )
        .thenReturn("Success")
    whenever(mockAdbUtils.executeAdbCommand("install -r -d ${apkFile.absolutePath}", serialNumber))
        .thenReturn("Success")
    whenever(mockAdbUtils.executeAdbShellCommand("pm path $packageName", serialNumber))
        .thenReturn("package:$onDeviceApkPath")
    // First read (after --fastdeploy) is stale; second read (after plain install) matches.
    whenever(mockAdbUtils.executeAdbShellCommand("md5sum $onDeviceApkPath", serialNumber))
        .thenReturn("stalehash  $onDeviceApkPath")
        .thenReturn("${md5Hex(apkFile)}  $onDeviceApkPath")

    val result = androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName)
    assertTrue(result)

    val inOrder = inOrder(mockAdbUtils)
    inOrder
        .verify(mockAdbUtils)
        .executeAdbCommand("install -r -d --fastdeploy ${apkFile.absolutePath}", serialNumber)
    inOrder
        .verify(mockAdbUtils)
        .executeAdbCommand("install -r -d ${apkFile.absolutePath}", serialNumber)
  }

  @Test
  fun testInstallFailsWhenApkNeverMatches() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("28")
    whenever(mockAdbUtils.executeAdbCommand("install -r -d ${apkFile.absolutePath}", serialNumber))
        .thenReturn("Success")
    // adb reports success but the on-device apk never matches the local apk.
    whenever(mockAdbUtils.executeAdbShellCommand("pm path $packageName", serialNumber))
        .thenReturn("package:$onDeviceApkPath")
    whenever(mockAdbUtils.executeAdbShellCommand("md5sum $onDeviceApkPath", serialNumber))
        .thenReturn(
            "00000000000000000000000000000000  $onDeviceApkPath",
        )

    try {
      androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName)
      fail("Expected AndroidInstallException")
    } catch (e: AndroidInstallException) {
      assertTrue(e.message!!.contains("could not be verified"))
      assertEquals(setOf(AndroidInstallErrorTag.INSTALLED_APK_MISMATCH), e.installError.tags)
    }
  }

  @Test
  fun testInstallFailsWhenPackageNotPresentAfterInstall() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("28")
    whenever(mockAdbUtils.executeAdbCommand("install -r -d ${apkFile.absolutePath}", serialNumber))
        .thenReturn("Success")
    // `pm path` prints nothing for a package that is not installed.
    whenever(mockAdbUtils.executeAdbShellCommand("pm path $packageName", serialNumber))
        .thenReturn("")

    try {
      androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName)
      fail("Expected AndroidInstallException")
    } catch (e: AndroidInstallException) {
      assertTrue(e.message!!.contains("is not present on the"))
      assertEquals(setOf(AndroidInstallErrorTag.INSTALLED_APK_MISMATCH), e.installError.tags)
    }
  }

  @Test
  fun testInstallVerifiesBaseApkWhenPmPathReturnsSplits() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("28")
    whenever(mockAdbUtils.executeAdbCommand("install -r -d ${apkFile.absolutePath}", serialNumber))
        .thenReturn("Success")
    // Split install: `pm path` returns the base apk plus config splits, one per line. Verification
    // must hash the base apk (first line) and not choke on the multi-line output.
    whenever(mockAdbUtils.executeAdbShellCommand("pm path $packageName", serialNumber))
        .thenReturn(
            "package:$onDeviceApkPath\npackage:/data/app/com.test.app-1/split_config.arm64_v8a.apk",
        )
    whenever(mockAdbUtils.executeAdbShellCommand("md5sum $onDeviceApkPath", serialNumber))
        .thenReturn("${md5Hex(apkFile)}  $onDeviceApkPath")

    assertTrue(androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName))
  }

  @Test
  fun testInstallFailsWithAdbErrorWhenHashUnreadable() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("28")
    whenever(mockAdbUtils.executeAdbCommand("install -r -d ${apkFile.absolutePath}", serialNumber))
        .thenReturn("Success")
    whenever(mockAdbUtils.executeAdbShellCommand("pm path $packageName", serialNumber))
        .thenReturn("package:$onDeviceApkPath")
    // md5sum reports an error on stdout while adb still exits 0, so the first token is not a
    // digest. This must be classified as a read failure, not an apk mismatch.
    whenever(mockAdbUtils.executeAdbShellCommand("md5sum $onDeviceApkPath", serialNumber))
        .thenReturn("md5sum: $onDeviceApkPath: No such file or directory")

    try {
      androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName)
      fail("Expected AndroidInstallException")
    } catch (e: AndroidInstallException) {
      assertEquals(setOf(AndroidInstallErrorTag.ADB_COMMAND_FAILED), e.installError.tags)
    }
  }

  @Test
  fun testStagedInstallDoesNotUseFastdeploy() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("30")
    whenever(
        mockAdbUtils.executeAdbCommand(
            "install -r -d --staged ${apkFile.absolutePath}",
            serialNumber,
        ),
    )
        .thenReturn("Success")

    // --fastdeploy is incompatible with staged installs, so a staged install must not use it even
    // on SDK >= 29. Staged installs are not verified (not applied until reboot).
    val result = androidDevice.installApkOnDevice(apkFile, false, false, false, true, packageName)

    verify(mockAdbUtils)
        .executeAdbCommand("install -r -d --staged ${apkFile.absolutePath}", serialNumber)
    verify(mockAdbUtils, never())
        .executeAdbCommand(argThat { contains("--fastdeploy") }, eq(serialNumber), any())
    assertTrue(result)
  }

  @Test
  fun testFastInstallFallsBackToPlainWhenFastdeployExitsNonZero() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("29")

    // --fastdeploy exits non-zero; fall back to a plain install, which succeeds.
    val fastInstall = "install -r -d --fastdeploy ${apkFile.absolutePath}"
    doAnswer { throw AdbCommandFailedException("adb: failed to install via fastdeploy") }
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq(fastInstall), eq(serialNumber), any())
    whenever(mockAdbUtils.executeAdbCommand("install -r -d ${apkFile.absolutePath}", serialNumber))
        .thenReturn("Success")
    stubInstallVerified()

    val result = androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName)
    assertTrue(result)

    val inOrder = inOrder(mockAdbUtils)
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq(fastInstall), eq(serialNumber), any())
    inOrder
        .verify(mockAdbUtils)
        .executeAdbCommand("install -r -d ${apkFile.absolutePath}", serialNumber)
  }

  @Test
  fun testInstallApkDetectsSilentInstallFailure() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("29")

    // --fastdeploy exits 0 but leaves a stale apk (verify catches it), then the plain fallback
    // fails
    // for real with insufficient storage. This must surface as a failure, not a spurious success.
    val fastInstall = "install -r -d --fastdeploy ${apkFile.absolutePath}"
    val plainInstall = "install -r -d ${apkFile.absolutePath}"
    whenever(mockAdbUtils.executeAdbCommand(fastInstall, serialNumber))
        .thenReturn("Performing Streamed Install")
    whenever(mockAdbUtils.executeAdbShellCommand("pm path $packageName", serialNumber))
        .thenReturn("package:$onDeviceApkPath")
    whenever(mockAdbUtils.executeAdbShellCommand("md5sum $onDeviceApkPath", serialNumber))
        .thenReturn("stalehash  $onDeviceApkPath")
    doAnswer {
          throw AdbCommandFailedException(
              "Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE: Not enough space]",
          )
        }
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq(plainInstall), eq(serialNumber), any())

    try {
      androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName)
      fail("Expected AndroidInstallException")
    } catch (e: AndroidInstallException) {
      assertTrue(e.message!!.contains("Failed to install test.apk"))
    }

    // An insufficient-storage failure is not a signature mismatch, so no uninstall must happen.
    verify(mockAdbUtils, never())
        .executeAdbCommand(argThat { startsWith("uninstall") }, eq(serialNumber), any())
  }

  @Test
  fun testFastInstallRecoversFromSignatureMismatchViaPlainFallback() {
    whenever(mockAdbUtils.executeAdbShellCommand("getprop ro.build.version.sdk", serialNumber))
        .thenReturn("29")

    // --fastdeploy exits 0 but leaves a stale apk (verify catches it); the plain fallback then
    // surfaces the signature mismatch (adb exits non-zero), triggering uninstall + retry.
    whenever(
        mockAdbUtils.executeAdbCommand(
            "install -r -d --fastdeploy ${apkFile.absolutePath}",
            serialNumber,
        ),
    )
        .thenReturn("Performing Streamed Install")

    val plainInstall = "install -r -d ${apkFile.absolutePath}"
    var plainAttempts = 0
    doAnswer {
          plainAttempts++
          if (plainAttempts == 1) {
            throw AdbCommandFailedException(
                "Executing 'adb $plainInstall' on $serialNumber failed with code 1.\nError:\n" +
                    "Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE: Existing package " +
                    "com.meta.ar.helixserver signatures do not match newer version; ignoring!]",
            )
          }
          "Success"
        }
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq(plainInstall), eq(serialNumber), any())

    doReturn("Success")
        .whenever(mockAdbUtils)
        .executeAdbCommand(eq("uninstall com.meta.ar.helixserver"), eq(serialNumber), any())
    whenever(mockAdbUtils.executeAdbShellCommand("pm path $packageName", serialNumber))
        .thenReturn("package:$onDeviceApkPath")
    // Stale after --fastdeploy (triggers fallback); matches after the plain reinstall.
    whenever(mockAdbUtils.executeAdbShellCommand("md5sum $onDeviceApkPath", serialNumber))
        .thenReturn("stalehash  $onDeviceApkPath")
        .thenReturn("${md5Hex(apkFile)}  $onDeviceApkPath")

    val result = androidDevice.installApkOnDevice(apkFile, false, false, false, false, packageName)
    assertTrue(result)

    val inOrder = inOrder(mockAdbUtils)
    inOrder
        .verify(mockAdbUtils)
        .executeAdbCommand("install -r -d --fastdeploy ${apkFile.absolutePath}", serialNumber)
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq(plainInstall), eq(serialNumber), any())
    inOrder
        .verify(mockAdbUtils)
        .executeAdbCommand(eq("uninstall com.meta.ar.helixserver"), eq(serialNumber), any())
    inOrder.verify(mockAdbUtils).executeAdbCommand(eq(plainInstall), eq(serialNumber), any())
  }

  @Test
  fun testInstallBuildUuidFileSetsTheUmaskInTheShellThatWritesTheFile() {
    val result =
        androidDevice.installBuildUuidFile(
            Paths.get("/data/local/tmp/build_metadata"),
            packageName,
            "some-build-uuid",
        )

    // One command, not two: `umask` is per-process, so one set in its own `adb shell` is gone by
    // the time a second shell's redirect creates the file, which then takes adbd's default mode
    // rather than 0644. Pinning the single chained command is what holds that.
    assertTrue(result)
    verify(mockAdbUtils)
        .executeAdbShellCommand(
            "umask 022 && mkdir -p /data/local/tmp/build_metadata/$packageName && " +
                "echo some-build-uuid > /data/local/tmp/build_metadata/$packageName/build_uuid.txt",
            serialNumber,
            false,
        )
  }

  @Test
  fun testInstallBuildUuidFileDoesNotFailTheInstallWhenTheDeviceRefusesTheWrite() {
    // doAnswer, not thenThrow: Kotlin emits no `throws` clause, so Mockito rejects a checked
    // exception as a stubbed one even though the code under test can observe it.
    doAnswer { throw AdbCommandFailedException("read-only file system") }
        .whenever(mockAdbUtils)
        .executeAdbShellCommand(any(), eq(serialNumber), any())

    assertTrue(
        androidDevice.installBuildUuidFile(
            Paths.get("/data/local/tmp/build_metadata"),
            packageName,
            "some-build-uuid",
        ),
    )
  }
}
