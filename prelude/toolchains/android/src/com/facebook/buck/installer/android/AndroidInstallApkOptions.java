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

import com.facebook.buck.util.environment.EnvVariablesProvider;
import com.facebook.buck.util.environment.Platform;
import com.facebook.buck.util.json.ObjectMappers;
import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.type.TypeReference;
import java.io.File;
import java.io.IOException;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Map;
import java.util.Optional;
import java.util.TreeMap;
import java.util.logging.Logger;

/**
 * Constructs configurations for an android install that are set in install_android_options.json and
 * sent to the AndroidInstaller
 */
public class AndroidInstallApkOptions {
  private static final Logger LOG = Logger.getLogger(AndroidInstallApkOptions.class.getName());
  public final String adbExecutable;
  public final boolean apexMode;

  AndroidInstallApkOptions(Path jsonArtifactPath, String adbExecutablePath)
      throws RuntimeException, IOException {
    JsonParser parser = ObjectMappers.createParser(jsonArtifactPath);
    Map<String, String> jsonData =
        parser.readValueAs(new TypeReference<TreeMap<String, String>>() {});
    this.adbExecutable = getAdbExecutable(adbExecutablePath);
    this.apexMode = readBoolean(jsonData, "apex_mode");
  }

  private boolean readBoolean(Map<String, String> jsonData, String name) {
    return Boolean.parseBoolean(jsonData.getOrDefault(name, Boolean.FALSE.toString()));
  }

  private int readInt(Map<String, String> jsonData, String name, int defaultValue) {
    String readValue = jsonData.getOrDefault(name, "");
    if (readValue.isEmpty()) {
      return defaultValue;
    }
    return Integer.parseInt(readValue);
  }

  /*
   * Here is the order of precedence for adb_executable:
   * 1. --adb-executable-path from command line option
   * 2. adb from PATH
   * 3. /opt/android_sdk/platform-tools/adb
   */
  private String getAdbExecutable(String adbExecutablePath) {
    String adbExecutable =
        Optional.ofNullable(adbExecutablePath)
            .orElse(getAdbFromPath().orElse("/opt/android_sdk/platform-tools/adb"));
    LOG.info("adbExecutable: " + adbExecutable);
    return adbExecutable;
  }

  private Optional<String> getAdbFromPath() {
    String path = EnvVariablesProvider.getSystemEnv().get("PATH");
    if (path != null) {
      String binaryExtension = Platform.detect() == Platform.WINDOWS ? ".exe" : "";
      String adbBinary = "adb" + binaryExtension;
      for (String pathEntry : path.split(File.pathSeparator)) {
        Path entry = Paths.get(pathEntry);
        Path adb = entry.resolve(adbBinary);
        if (adb.toFile().exists()) {
          return Optional.of(adb.toString());
        }
      }
    }
    return Optional.empty();
  }

  @Override
  public String toString() {
    return "AndroidInstallApkOptions{"
        + "adbExecutable='"
        + adbExecutable
        + '\''
        + ", apexMode="
        + apexMode
        + '}';
  }
}
