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

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.android.AdbHelper;
import com.facebook.buck.android.AdbOptions;
import com.facebook.buck.android.exopackage.InstallTimings;
import com.facebook.buck.android.exopackage.SetDebugAppMode;
import com.facebook.buck.testutil.TestLogSink;
import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.logging.Logger;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.kohsuke.args4j.CmdLineParser;

public class AndroidCommandLineOptionsTest {
  @Rule public final TemporaryFolder temporaryFolder = new TemporaryFolder();
  @Rule public final TestLogSink logSink = new TestLogSink(AndroidCommandLineOptionsTest.class);

  @Test
  public void forceQueryableDefaultsOff() throws Exception {
    AndroidCommandLineOptions options = new AndroidCommandLineOptions();
    new CmdLineParser(options).parseArgument();
    assertFalse(options.forceQueryable);
    assertFactoryForceQueryable(options, false);
    assertFalse(
        new AdbOptions("adb", 0, 0, false, false, false, false, "auto", false).isForceQueryable());
  }

  @Test
  public void forceQueryableIsExplicitOptIn() throws Exception {
    AndroidCommandLineOptions options = new AndroidCommandLineOptions();
    new CmdLineParser(options).parseArgument("--force-queryable");
    assertTrue(options.forceQueryable);
    assertFactoryForceQueryable(options, true);
  }

  private void assertFactoryForceQueryable(AndroidCommandLineOptions options, boolean expected)
      throws Exception {
    Path installOptions = temporaryFolder.newFile("install-options.json").toPath();
    Files.writeString(installOptions, "{}");
    AndroidInstallApkOptions apkOptions =
        new AndroidInstallApkOptions(installOptions, "adb-not-executed");
    try (AdbHelper helper =
        AdbHelperFactory.create(
            Logger.getLogger(AndroidCommandLineOptionsTest.class.getName()),
            options,
            apkOptions,
            SetDebugAppMode.SKIP,
            InstallTimings.NONE)) {
      assertNotNull(helper);
      assertTrue(
          logSink.getRecords().stream()
              .anyMatch(
                  record ->
                      record.getMessage().startsWith("adbOptions: ")
                          && record.getMessage().endsWith(", forceQueryable=" + expected + "}")));
    }
  }

  @Test
  public void usageIncludesForceQueryable() {
    ByteArrayOutputStream usage = new ByteArrayOutputStream();
    new CmdLineParser(new AndroidCommandLineOptions()).printUsage(usage);
    String text = usage.toString(StandardCharsets.UTF_8).replaceAll("\\s+", " ");
    assertTrue(text.contains("--force-queryable"));
    assertTrue(text.contains("visible to all packages on Android 11+"));
    assertTrue(text.contains("Reuses a matching APK when it is already queryable."));
    assertTrue(
        text.contains(
            "The app stays queryable until it is uninstalled or reinstalled with --uninstall"
                + " (-u) without this flag."));
    assertFalse(text.contains("Disables APK reuse"));
  }
}
