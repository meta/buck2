/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.installer.android;

import static org.junit.Assert.assertEquals;

import com.facebook.buck.installer.InstallError;
import java.util.Set;
import org.junit.Test;

/** Messages below are the ones the installer reports, as buck2 logged them. */
public class AndroidInstallErrorClassifierTest {

  private static void assertClassified(AndroidInstallErrorTag expected, String message) {
    InstallError error = AndroidInstallErrorClassifier.INSTANCE.fromErrorMessage(message);
    assertEquals(Set.of(expected), error.getTags());
    assertEquals(expected.name() + ": " + message, error.getMessage());
  }

  @Test
  public void noAttachedDeviceIsDeviceNotFound() {
    assertClassified(
        AndroidInstallErrorTag.DEVICE_NOT_FOUND,
        "java.lang.RuntimeException: Didn't find any attached Android devices/emulators.");
  }

  @Test
  public void severalMatchingDevicesIsMultipleDevicesMatchFilter() {
    assertClassified(
        AndroidInstallErrorTag.MULTIPLE_DEVICES_MATCH_FILTER,
        "java.lang.RuntimeException: 4 devices match specified device filter (1 expected).\n"
            + "Either disconnect other devices or enable multi-install mode (-x).");
  }

  @Test
  public void missingAdbIsAdbNotFound() {
    assertClassified(
        AndroidInstallErrorTag.ADB_NOT_FOUND,
        "Could not find `adb` in PATH, and could not find `adb` at"
            + " /opt/android_sdk/platform-tools/adb. Please add `adb` to your PATH");
  }

  @Test
  public void adbServerThatCannotStartIsFailedToConnectToAdb() {
    assertClassified(
        AndroidInstallErrorTag.FAILED_TO_CONNECT_TO_ADB,
        "java.lang.IllegalStateException:"
            + " com.facebook.buck.android.exopackage.AdbCommandFailedException: Executing 'adb"
            + " start-server' on null failed with code 255.\n"
            + "Error:\n"
            + "* daemon not running; starting now at tcp:5037\n"
            + "ADB server didn't ACK\n"
            + "* failed to start daemon\n"
            + "error: cannot connect to daemon\n");
  }

  @Test
  public void adbDevicesFailingIsFailedToConnectToAdb() {
    assertClassified(
        AndroidInstallErrorTag.FAILED_TO_CONNECT_TO_ADB,
        "java.lang.IllegalStateException:"
            + " com.facebook.buck.android.exopackage.AdbCommandFailedException: Executing 'adb"
            + " devices' on null failed with code 1.\n"
            + "Error:\n"
            + "error: cannot connect to daemon\n");
  }

  @Test
  public void adbCommandFailingOnADeviceIsNotAnAdbServerFailure() {
    assertClassified(
        AndroidInstallErrorTag.OTHER_INFRA,
        "com.facebook.buck.android.exopackage.AdbCommandFailedException: Executing 'adb shell am"
            + " set-debug-app --persistent com.instagram.android ' on emulator-5554 failed with"
            + " code 1.");
  }
}
