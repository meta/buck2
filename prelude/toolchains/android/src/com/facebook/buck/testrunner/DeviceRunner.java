/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.testrunner;

import com.facebook.buck.android.exopackage.AdbUtils;
import com.facebook.buck.android.exopackage.AndroidDevice;
import com.facebook.buck.android.exopackage.AndroidDeviceImpl;
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.net.InetAddress;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.List;
import java.util.Objects;
import org.jspecify.annotations.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class DeviceRunner {

  private static final String ADB_PATH = "ADB_PATH";
  private static final String PWD = "PWD";

  static class DeviceArgs {
    final boolean autoRunOnConnectedDevice;
    final @Nullable String deviceSerial;
    final @Nullable String adbExecutablePath;

    DeviceArgs(
        boolean autoRunOnConnectedDevice,
        @Nullable String deviceSerial,
        @Nullable String adbExecutablePath) {
      this.autoRunOnConnectedDevice = autoRunOnConnectedDevice;
      this.deviceSerial = deviceSerial;
      this.adbExecutablePath = adbExecutablePath;
    }
  }

  protected DeviceArgs deviceArgs;
  protected final AdbUtils adbUtils;

  DeviceRunner(DeviceArgs args) {
    this.deviceArgs = args;
    this.adbUtils = new AdbUtils(Objects.requireNonNull(getAdbPath()), 0);
  }

  static DeviceArgs getDeviceArgs(String[] args) {
    String adbExecutablePath = null;
    boolean autoRunOnConnectedDevice = false;
    for (int i = 0; i < args.length; i++) {
      switch (args[i]) {
        case "--adb-executable-path":
          adbExecutablePath = args[++i];
          break;
        case "--auto-run-on-connected-device":
          autoRunOnConnectedDevice = true;
          break;
      }
    }

    if (adbExecutablePath == null) {
      System.err.println("Must pass --adb-executable-path argument.");
      System.exit(1);
    }

    final String adbPath = System.getenv(ADB_PATH);
    if (adbPath != null && !adbPath.isEmpty()) {
      adbExecutablePath = Paths.get(adbPath).toAbsolutePath().toString();
    }

    String buckdeviceSerial = System.getProperty("buck.device.id");
    String androidSerial = System.getenv("ANDROID_SERIAL");

    String deviceSerial = null;

    if (buckdeviceSerial != null) {
      deviceSerial = buckdeviceSerial;
    } else if (androidSerial != null) {
      deviceSerial = androidSerial;
    }

    if (deviceSerial != null) {
      // If a device serial was set, we use that instead of trying to auto connect.
      // This behavior is in line with other android tools.
      autoRunOnConnectedDevice = false;
    }

    if (deviceSerial == null && !autoRunOnConnectedDevice) {
      System.err.println(
          "Must pass buck.device.id system property, as this run is not configured to"
              + " auto-connect to device.");
      System.exit(1);
    }

    return new DeviceArgs(autoRunOnConnectedDevice, deviceSerial, adbExecutablePath);
  }

  /**
   * Method that either returns the given string as-is, or if the given string begins with a '@',
   * reads a single string from the specified file.
   */
  protected static String getArgValue(String input) throws IOException {
    if (!input.startsWith("@")) {
      return input;
    }

    Path inputPath = Paths.get(input.substring(1));
    return new String(Files.readAllBytes(inputPath));
  }

  /**
   * Returns an {@link AndroidDevice} for the configured device. Uses {@link AdbUtils} for device
   * discovery — no ddmlib dependency.
   */
  @Nullable AndroidDevice getAndroidDevice(boolean autoRunOnConnectedDevice, String deviceSerial) {
    if (autoRunOnConnectedDevice) {
      List<AndroidDevice> devices = adbUtils.getDevices();
      if (devices.isEmpty()) {
        String hostname = null;
        try {
          hostname = InetAddress.getLocalHost().getHostName();
        } catch (Exception _e) {
          // no op
        }
        System.err.println(
            String.format(
                "Found 0 device %s-- please connect a device.",
                hostname == null ? "" : String.format("on %s ", hostname)));
        System.exit(1);
        return null;
      }
      return devices.get(0);
    } else {
      if (deviceSerial == null) {
        System.err.println("Unable to get device/emulator: no serial provided");
        System.exit(1);
        return null;
      }
      return new AndroidDeviceImpl(deviceSerial, adbUtils);
    }
  }

  protected @Nullable String getAdbPath() {
    return deviceArgs.adbExecutablePath;
  }

  /** We minimize external dependencies, but we'd like to have {@link javax.annotation.Nullable}. */
  @interface Nullable {}
}
