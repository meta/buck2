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

import com.facebook.buck.installer.InstallErrorCategory
import com.facebook.buck.installer.InstallErrorCategory.ENVIRONMENT
import com.facebook.buck.installer.InstallErrorCategory.INFRA
import com.facebook.buck.installer.InstallErrorCategory.USER
import com.facebook.buck.installer.InstallErrorTag

enum class AndroidInstallErrorTag(val category: InstallErrorCategory) : InstallErrorTag {
  ERROR_MATERIALIZING_ARTIFACT(INFRA),
  OTHER_INFRA(INFRA),
  ADB_PROTOCOL_VERSION_MISMATCH(ENVIRONMENT),
  ADB_NOT_FOUND(ENVIRONMENT),
  FAILED_TO_CONNECT_TO_ADB(ENVIRONMENT),
  TEMP_FOLDER_NOT_WRITABLE(USER),
  ADB_COMMAND_REJECTED(USER),
  ADB_COMMAND_FAILED(USER),
  ADB_CONNECTION_RESET_BY_PEER(USER),
  ADB_CONNECTION_TIMED_OUT(USER),
  NO_ATTACHED_DEVICE(USER),
  MULTIPLE_DEVICES_MATCH_FILTER(USER),
  DEVICE_NOT_FOUND(USER),
  DEVICE_OFFLINE(USER),
  NO_SPACE_LEFT_ON_DEVICE(USER),
  REQUIRE_NEWER_SDK(USER),
  INCOMPATIBLE_NATIVE_LIB(USER),
  INCOMPATIBLE_ABI(USER),
  UPDATE_INCOMPATIBLE(USER),
  MISSING_SHARED_LIBRARY(USER),
  VERIFICATION_FAILED(USER),
  INSTALLED_APK_MISMATCH(USER),
  INVALID_APK(USER),
  INSTALL_CANCELLED_BY_USER(USER),
  UNKNOWN_DEVICE_ABI(USER),
  MANUAL_REBOOT_REQUIRED(USER),
  EXOPACKAGE_GARBAGE_COLLECTION_FAILED(USER);

  override fun getErrorCategory(): InstallErrorCategory {
    return category
  }

  override fun getName(): String {
    return name
  }
}
