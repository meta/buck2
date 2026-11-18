/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.installer.android

import com.facebook.buck.installer.InstallError

object AndroidInstallErrorClassifier {
  fun fromErrorMessage(input: String): InstallError {
    if (isInsufficientStorageFailure(input)) {
      return createInstallError(
          AndroidInstallErrorTag.NO_SPACE_LEFT_ON_DEVICE,
          "No space left on device. Free up space on the device and try again. Exopackage " +
              "files are kept per package under /data/local/tmp/exopackage: see their sizes " +
              "with `adb shell du -sh /data/local/tmp/exopackage/*` and delete those of " +
              "packages you no longer need with " +
              "`adb shell rm -rf /data/local/tmp/exopackage/<package>`.",
      )
    }
    return when {
      input.contains("Failed to connect to adb.") || ADB_SERVER_FAILURE.containsMatchIn(input) ->
          createInstallError(AndroidInstallErrorTag.FAILED_TO_CONNECT_TO_ADB, input)
      input.contains("Didn't find any attached Android devices/emulators.") ->
          createInstallError(AndroidInstallErrorTag.DEVICE_NOT_FOUND, input)
      input.contains("devices match specified device filter") ->
          createInstallError(AndroidInstallErrorTag.MULTIPLE_DEVICES_MATCH_FILTER, input)
      Regex("You are trying to install .* onto a device with the following CPU")
          .containsMatchIn(input) ->
          createInstallError(AndroidInstallErrorTag.INCOMPATIBLE_ABI, input)
      Regex("You are trying to install an APK with incompatible native libraries")
          .containsMatchIn(input) ->
          createInstallError(AndroidInstallErrorTag.INCOMPATIBLE_NATIVE_LIB, input)
      else -> createInstallError(AndroidInstallErrorTag.OTHER_INFRA, input)
    }
  }

  // These commands name no device, so their failure is the adb server's: it could not be reached or
  // started.
  private val ADB_SERVER_FAILURE = Regex("Executing 'adb (devices|start-server)' on null failed")

  private fun createInstallError(tag: AndroidInstallErrorTag, msg: String): InstallError {
    return InstallError(msg, tag)
  }
}
