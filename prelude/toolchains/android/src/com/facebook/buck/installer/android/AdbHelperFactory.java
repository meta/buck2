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

import com.facebook.buck.android.AdbExecutionContext;
import com.facebook.buck.android.AdbHelper;
import com.facebook.buck.android.AdbOptions;
import com.facebook.buck.android.device.TargetDeviceOptions;
import com.facebook.buck.android.exopackage.AdbUtils;
import com.facebook.buck.android.exopackage.InstallTimings;
import com.facebook.buck.android.exopackage.SetDebugAppMode;
import java.util.Optional;
import java.util.logging.Logger; // NOPMD

/**
 * Builds the {@link AdbHelper} for an install.
 *
 * <p>The one place the command line and the install options are turned into the objects adb needs,
 * so a second construction cannot drift from it and reach a different set of devices.
 */
class AdbHelperFactory {

  private AdbHelperFactory() {}

  static AdbHelper create(
      Logger logger,
      AndroidCommandLineOptions cliOptions,
      AndroidInstallApkOptions apkOptions,
      SetDebugAppMode setDebugAppMode,
      InstallTimings timings) {
    AdbOptions adbOptions =
        new AdbOptions(
            cliOptions.adbExecutablePath,
            cliOptions.adbThreadCount,
            cliOptions.adbServerPort,
            cliOptions.multiInstallMode,
            apkOptions.stagedInstallMode,
            cliOptions.ignoreMissingDevices,
            apkOptions.apexMode,
            cliOptions.restartMode.name(),
            cliOptions.waitForDeviceReady,
            cliOptions.forceQueryable);
    logger.info("adbOptions: " + adbOptions);

    TargetDeviceOptions targetDeviceOptions =
        new TargetDeviceOptions(
            cliOptions.useEmulatorsOnlyMode,
            cliOptions.useRealDevicesOnlyMode,
            Optional.ofNullable(cliOptions.serialNumber));
    logger.info("targetDeviceOptions: " + targetDeviceOptions);

    AdbUtils adbUtils =
        new AdbUtils(
            Optional.ofNullable(apkOptions.adbExecutable)
                .orElseThrow(AndroidInstallException.Companion::adbPathNotFound),
            adbOptions.getAdbServerPort());

    return new AdbHelper(
        adbUtils,
        adbOptions,
        targetDeviceOptions,
        new AdbExecutionContext(),
        new IsolatedAndroidInstallerPrinter(logger),
        apkOptions.restartAdbOnFailure,
        setDebugAppMode,
        timings);
  }
}
