/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util;

import com.facebook.infer.annotation.Nullsafe;

/**
 * An indication of how verbose Buck should be. Enum values are in order from least to most verbose.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public enum Verbosity {
  // TODO(mbolin): Consider introducing more Verbosity levels that affect the Java Logger
  // setting.

  /** Do not print anything to the console. */
  SILENT(0),

  /** Prints out the bare minimum required information, such as errors from build steps. */
  STANDARD_INFORMATION(1),

  /** Print extra output from generated binaries and tests being run, but nothing else. */
  BINARY_OUTPUTS(2),

  /** Print the command being executed, but do not print its output. */
  COMMANDS(3),

  /** Commands plus the output from some select commands of interest. */
  COMMANDS_AND_SPECIAL_OUTPUT(4),

  /** Print the command being executed followed by its output. */
  COMMANDS_AND_OUTPUT(5),

  /**
   * If the command being executed has its own {@code --verbose} option or equivalent, it should be
   * used.
   */
  ALL(100),
  ;

  private final int level;

  Verbosity(int level) {
    this.level = level;
  }

  public boolean isSilent() {
    return level <= SILENT.level;
  }

  public boolean shouldPrintStandardInformation() {
    return level >= STANDARD_INFORMATION.level;
  }

  public boolean shouldPrintBinaryRunInformation() {
    return level >= BINARY_OUTPUTS.level;
  }

  public boolean shouldPrintCommand() {
    return level >= COMMANDS.level;
  }

  public boolean shouldPrintSelectCommandOutput() {
    return level >= COMMANDS_AND_SPECIAL_OUTPUT.level;
  }

  public boolean shouldPrintOutput() {
    return level >= COMMANDS_AND_OUTPUT.level;
  }

  /**
   * @return {@code true} if the command being executed should use its own {@code --verbose}
   *     argument (or equivalent) for this Verbosity level
   */
  public boolean shouldUseVerbosityFlagIfAvailable() {
    return this == ALL;
  }
}
