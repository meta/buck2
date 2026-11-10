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

import com.facebook.buck.android.AdbHelper;
import com.facebook.buck.android.IsolatedApkInfo;
import com.facebook.buck.android.exopackage.AndroidDeviceInfo;
import com.facebook.buck.android.exopackage.ExopackageInstaller;
import com.facebook.buck.android.exopackage.IsolatedExopackageInfo;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.installer.InstallId;
import com.facebook.buck.installer.InstallResult;
import com.facebook.infer.annotation.Nullsafe;
import java.time.Duration;
import java.time.Instant;
import java.time.ZoneId;
import java.time.format.DateTimeFormatter;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.Set;
import java.util.logging.Level;
import java.util.logging.Logger;

/** Installs an Android Apk */
@Nullsafe(Nullsafe.Mode.LOCAL)
class AndroidInstall {
  private static final Logger LOG = Logger.getLogger(AndroidInstall.class.getName());
  private static final DateTimeFormatter INSTALL_COMPLETION_TIME_FORMAT =
      DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss");
  private static final Set<String> ENABLE_APP_LINKS_ALLOWLIST =
      Set.of("com.facebook.wakizashi", "com.facebook.lite", "com.instagram.lite");

  private final IsolatedApkInfo apkInfo;
  private final Optional<IsolatedExopackageInfo> exopackageInfo;
  private final AbsPath rootPath;
  private final InstallId installId;
  private final AndroidCommandLineOptions cliOptions;
  private final boolean installViaSd = false;
  private final Logger logger;
  private final AdbHelper adbHelper;
  private final InstallState state;

  public AndroidInstall(
      Logger logger,
      AbsPath rootPath,
      AndroidCommandLineOptions cliOptions,
      IsolatedApkInfo apkInfo,
      Optional<IsolatedExopackageInfo> exopackageInfo,
      InstallId installId,
      InstallState state,
      AdbHelper adbHelper) {
    this.logger = logger;
    this.rootPath = rootPath;
    this.apkInfo = apkInfo;
    this.exopackageInfo = exopackageInfo;
    this.installId = installId;
    this.cliOptions = cliOptions;
    this.state = state;
    this.adbHelper = adbHelper;
  }

  /** Uses AdbHelper to do actual install with APK */
  public synchronized InstallResult installApk() {
    List<Map<String, String>> deviceInfos = new ArrayList();
    try {
      if (cliOptions.cleanUp) {
        adbHelper.uninstallApp(
            Objects.requireNonNull(state.packageName()), cliOptions.keepUserData);
      } else {
        if (cliOptions.uninstallFirst) {
          adbHelper.uninstallApp(
              Objects.requireNonNull(state.packageName()), cliOptions.keepUserData);
        }
        logger.info(String.format("Attempting install of %s", apkInfo.getApkPath()));
        Instant start = Instant.now();
        // Everything from here on is device work, including probing it for its properties.
        long deviceWorkStart = System.currentTimeMillis();

        boolean isExopackage = exopackageInfo.map(ExopackageInstaller::isExopackage).orElse(false);
        Set<AndroidDeviceInfo> androidDeviceInfos = adbHelper.getAndroidDeviceInfo(apkInfo);
        for (AndroidDeviceInfo deviceInfo : androidDeviceInfos) {
          Map<String, String> infoMap = new LinkedHashMap<>();
          infoMap.put("sdk", deviceInfo.getSdk());
          infoMap.put("abi", deviceInfo.getAbi());
          infoMap.put("locale", deviceInfo.getLocale());
          infoMap.put("build_fingerprint", deviceInfo.getBuildFingerprint());
          infoMap.put("is_emulator", deviceInfo.isEmulator() ? "1" : "0");
          infoMap.put("density", deviceInfo.getDensity().toString());
          infoMap.put("transport", deviceInfo.getTransport());
          // With the device fields rather than the timings below, so that it is still reported
          // when an install fails before any timing is complete.
          infoMap.put("is_exopackage", isExopackage ? "1" : "0");
          deviceInfos.add(infoMap);
        }
        for (AndroidDeviceInfo deviceInfo : androidDeviceInfos) {
          adbHelper.throwIfIncompatibleAbi(deviceInfo, apkInfo);
        }

        boolean startsActivity =
            cliOptions.run || cliOptions.activity != null || cliOptions.intentUri != null;
        adbHelper.installApk(
            apkInfo,
            exopackageInfo,
            rootPath,
            installViaSd,
            /* quiet= */ false,
            installId.getValue(),
            Objects.requireNonNull(state.packageName()),
            /* restartApp= */ startsActivity);
        state.metrics().recordDeviceWork(deviceWorkStart, System.currentTimeMillis());

        // Only now are the stage timings complete, so the metrics cannot be gathered any earlier.
        Map<String, String> installMetrics =
            state.metrics().summarise(System.currentTimeMillis(), state.artifacts().arrivals());
        deviceInfos.forEach(infoMap -> infoMap.putAll(installMetrics));
        Instant completedAt = Instant.now();
        logger.info(
            formatCompletionMessage(
                apkInfo.getApkPath().getFileName().toString(),
                start,
                completedAt,
                ZoneId.systemDefault()));

        String packageName = state.packageName();

        // Determine if app links should be enabled based on command line option or allowlist
        boolean shouldEnableAppLinks = false;
        if (cliOptions.enableAppLinks != null) {
          // Explicit option provided by user
          shouldEnableAppLinks = cliOptions.enableAppLinks;
        } else {
          // No option provided, check allowlist
          shouldEnableAppLinks = ENABLE_APP_LINKS_ALLOWLIST.contains(packageName);
        }

        if (shouldEnableAppLinks) {
          try {
            adbHelper.adbCall(
                "enable app links",
                (device) -> {
                  device.enableAppLinks(packageName);
                  return true;
                },
                true);
          } catch (Exception e) {
            logger.warning("Failed to enable app links: " + e.getMessage());
          }
        }

        if (startsActivity) {
          adbHelper.startActivityForIsolatedApk(
              apkInfo,
              installId.getValue(),
              cliOptions.activity,
              cliOptions.intentUri,
              cliOptions.waitForDebugger,
              cliOptions.skipSetDebugApp);
        }
      }
      return new InstallResult(deviceInfos, Optional.empty());
    } catch (AndroidInstallException exc) {
      return new InstallResult(deviceInfos, Optional.of(exc.getInstallError()));
    } catch (Exception err) {
      String errMsg = err.getMessage();
      logger.log(
          Level.SEVERE,
          String.format("Error while installing %s. Error message: %s", installId, errMsg),
          err);
      return new InstallResult(
          deviceInfos,
          Optional.of(Objects.requireNonNull(errMsg))
              .map(AndroidInstallErrorClassifier.INSTANCE::fromErrorMessage));
    }
  }

  static String formatCompletionMessage(
      String apkName, Instant start, Instant completedAt, ZoneId zoneId) {
    return String.format(
        "Install of %s finished in %d seconds at %s",
        apkName,
        Duration.between(start, completedAt).getSeconds(),
        INSTALL_COMPLETION_TIME_FORMAT.withZone(zoneId).format(completedAt));
  }
}
