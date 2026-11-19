/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.exopackage;

import com.facebook.buck.android.IsolatedApkInfo;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.collect.ImmutableList;
import java.io.Closeable;
import java.util.Set;

/**
 * AndroidDevicesHelper provides a way to interact with multiple devices as AndroidDevices (rather
 * than IDevices).
 *
 * <p>All of ExopackageInstaller's interaction with devices and adb goes through this class and
 * AndroidDevice making it easy to provide different implementations in tests.
 */
@VisibleForTesting
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface AndroidDevicesHelper extends Closeable {
  /**
   * This is basically the same as AdbHelper.AdbCallable except that it takes an AndroidDevice
   * instead of an IDevice.
   */
  interface AdbDeviceCallable {
    boolean apply(AndroidDevice device) throws Exception;
  }

  /** A simple wrapper around adbCall that will throw if adbCall returns false. */
  default void adbCallOrThrow(String description, AdbDeviceCallable func, boolean quiet)
      throws InterruptedException {
    adbCall(description, func, quiet);
  }

  void adbCall(String description, AdbDeviceCallable func, boolean quiet)
      throws InterruptedException;

  ImmutableList<AndroidDevice> getDevices(boolean quiet);

  /** Uninstall apk from all matching devices. */
  void uninstallApp(String packageName, boolean shouldKeepUserData) throws InterruptedException;

  Set<AndroidDeviceInfo> getAndroidDeviceInfo(IsolatedApkInfo isolatedApkInfo)
      throws InterruptedException;
}
