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
        error.getMessage(), error.getMessage().contains("could not find `adb` at " + missingAdb));
  }
}
