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

import com.facebook.buck.core.util.log.Logger
import com.facebook.buck.installer.InstallError

private const val INSUFFICIENT_STORAGE_ERROR = "INSTALL_FAILED_INSUFFICIENT_STORAGE"
private const val NO_SPACE_LEFT_ON_DEVICE_ERROR = "No space left on device"
// EDQUOT: the shell user cannot write into the space /data reserves for the system, so a device
// can refuse every write while `df` still reports space available.
private const val QUOTA_EXCEEDED_ERROR = "Quota exceeded"

internal fun isInsufficientStorageFailure(message: String): Boolean =
    message.contains(INSUFFICIENT_STORAGE_ERROR) ||
        message.contains(NO_SPACE_LEFT_ON_DEVICE_ERROR) ||
        message.contains(QUOTA_EXCEEDED_ERROR)

class AndroidInstallException(val installError: InstallError) :
    RuntimeException(installError.message) {
  init {
    LOG.error(installError.message)
  }

  companion object {
    private val LOG: Logger = Logger.get(AndroidInstallException::class.java.name)

    fun rebootRequired(msg: String) =
        AndroidInstallException(InstallError(msg, AndroidInstallErrorTag.MANUAL_REBOOT_REQUIRED))

    fun tempFolderNotWritable(): AndroidInstallException = AndroidInstallException(
        InstallError(
            "Temp folder is not writable.",
            AndroidInstallErrorTag.TEMP_FOLDER_NOT_WRITABLE,
        ),
    )

    fun operationNotSupported(operation: String): AndroidInstallException = AndroidInstallException(
        InstallError(
            "Operation $operation is not supported.",
            AndroidInstallErrorTag.OTHER_INFRA,
        ),
    )

    fun deviceAbiUnknown() = AndroidInstallException(
        InstallError("Device ABI is unknown.", AndroidInstallErrorTag.UNKNOWN_DEVICE_ABI),
    )

    fun exopackageGarbageCollectionFailed(message: String?) = AndroidInstallException(
        InstallError(
            "Failed to delete stale exopackage files, aborting before pushing new ones. " +
                "Continuing would leak several GB per install and eventually fill the device." +
                (message?.let { "\n$it" } ?: ""),
            AndroidInstallErrorTag.EXOPACKAGE_GARBAGE_COLLECTION_FAILED,
        ),
    )

    fun devicesDeparted(serials: Collection<String>) = AndroidInstallException(
        InstallError(
            "Device(s) disconnected while the build was running, so the install could not " +
                "reach them: ${serials.sorted().joinToString(", ")}",
            AndroidInstallErrorTag.DEVICE_NOT_FOUND,
        ),
    )

    fun adbPathNotFound() = AndroidInstallException(
        InstallError("Adb path not found.", AndroidInstallErrorTag.ADB_NOT_FOUND),
    )

    fun installedApkMismatch(message: String) = AndroidInstallException(
        InstallError(message, AndroidInstallErrorTag.INSTALLED_APK_MISMATCH),
    )

    fun adbCommandFailedException(
        message: String,
        exceptionMessage: String?,
    ): AndroidInstallException {
      val errorMessage = exceptionMessage?.let { "\n" + it } ?: ""
      val fullMessage = "$message.$errorMessage"
      val tag =
          if (isInsufficientStorageFailure(fullMessage)) {
            AndroidInstallErrorTag.NO_SPACE_LEFT_ON_DEVICE
          } else {
            AndroidInstallErrorTag.ADB_COMMAND_FAILED
          }
      return AndroidInstallException(InstallError(fullMessage, tag))
    }
  }
}
