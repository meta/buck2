/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.installer;

import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.Path;
import java.util.Set;

/**
 * Install Command interface. Support for a specific installer such as iOS or Android is implemented
 * as subclasses of this interface. *
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface InstallCommand {
  /**
   * Names this implementation, e.g. {@code android}. Reported with every install so telemetry can
   * tell one installer's installs from another's.
   */
  String name();

  /** Installs an artifact from a given path/location. */
  InstallResult fileReady(String artifact, Path artifactPath, InstallId installId);

  /** Indicate that all files have been received by the installer */
  InstallResult allFilesReady(InstallId installId);

  /**
   * Notifies which artifacts the client intends to send, before any of them arrive.
   *
   * <p>Lets an implementation tell "this build has no such artifact" apart from "it has not turned
   * up yet", which is not otherwise decidable while artifacts are still in flight.
   */
  default void onInstallStarted(InstallId installId, Set<String> expectedArtifacts) {}
}
