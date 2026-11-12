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
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.installer.InstallError;
import com.facebook.buck.installer.InstallId;
import com.facebook.buck.installer.InstallResult;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Set;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class AndroidInstallerManagerTest {
  @Rule public final TemporaryFolder temporaryFolder = new TemporaryFolder();

  /** Devices are resolved when the options arrive, so that is where a missing adb must be named. */
  @Test
  public void missingAdbFailsTheOptionsArtifactAsAdbNotFound() throws Exception {
    Path missingAdb = temporaryFolder.getRoot().toPath().resolve("no-such-adb");
    AndroidCommandLineOptions options = new AndroidCommandLineOptions();
    options.adbExecutablePath = missingAdb.toString();
    Path installOptions = temporaryFolder.newFile("install_android_options.json").toPath();
    Files.writeString(installOptions, "{}");

    InstallResult result =
        new AndroidInstallerManager(options)
            .fileReady("options", installOptions, InstallId.of("//app:app"));

    assertTrue(result.isError());
    InstallError error = result.getInstallError();
    assertEquals(Set.of(AndroidInstallErrorTag.ADB_NOT_FOUND), error.getTags());
    assertTrue(
        error.getMessage(),
        error
            .getMessage()
            .contains(
                "Could not find `adb` at "
                    + missingAdb
                    + ", the path given with --adb-executable-path or the buckconfig key"
                    + " `android.adb`."));
    assertFalse(error.getMessage(), error.getMessage().contains("PATH"));
  }

  /** A PATH change does not reach a running buck2 daemon, so the error names the flag. */
  @Test
  public void adbMissingFromDefaultLocationsNamesTheDaemonPathAndTheFlag() {
    InstallError error =
        AndroidInstallerManager.adbNotFound("/opt/android_sdk/platform-tools/adb", false);

    assertEquals(Set.of(AndroidInstallErrorTag.ADB_NOT_FOUND), error.getTags());
    String message = error.getMessage();
    assertTrue(
        message,
        message.contains(
            "Could not find `adb` in PATH, and could not find `adb` at"
                + " /opt/android_sdk/platform-tools/adb."));
    assertTrue(
        message,
        message.contains(
            "The PATH searched is the one the buck2 daemon started with, which can differ from"
                + " this command's."));
    assertTrue(
        message,
        message.contains(
            "`buck2 install <target> -- --adb-executable-path <absolute-path-to-adb>`"));
  }
}
