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
import com.fasterxml.jackson.annotation.JsonValue;
import com.google.common.collect.ImmutableMap;

/**
 * ExitCode class defines Buck binary protocol, i.e. exit codes that Buck can report to a running
 * shell. In addition to exit codes defined below, we have to honor following conventions.
 *
 * <p>Buck bootstrapper must conform to the protocol, i.e. properly implement FATAL_BOOTSTRAP
 *
 * <p>Exit codes for interrupts do follow POSIX convention, i.e. 128 + SIGNAL_CODE, i.e SIGINT is
 * returned as 128 + 2 = 130
 *
 * <p>Exit codes 1-9 are reserved for non-fatal errors, like build errors
 *
 * <p>Exit codes 10-19 are reserved for fatal errors, like unexpected runtime exceptions
 *
 * <p>Binary protocol is open to extension but closed to modification.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public enum ExitCode {
  // Success 0

  /** Buck command completed successfully */
  SUCCESS(0),

  // Non-fatal generic errors 1-9

  /** Build resulted in user-specific error */
  BUILD_ERROR(1),
  /** Buck daemon is busy processing another command */
  BUSY(2),
  /** User supplied incorrect command line options */
  COMMANDLINE_ERROR(3),
  /** There is nothing to build or evaluate for the command */
  NOTHING_TO_DO(4),
  /** There was build file parsing or graph construction error */
  PARSE_ERROR(5),
  /** Running a binary or installing binary to a device has failed */
  RUN_ERROR(6),

  // Fatal errors 10-19

  /** Generic Buck-internal non-recoverable error */
  FATAL_GENERIC(10),
  /** Non-recoverable error in Buck bootstrapper */
  FATAL_BOOTSTRAP(11),
  /** Non-recoverable OutOfMemory error */
  FATAL_OOM(12),
  /** Non-recoverable generic I/0 error */
  FATAL_IO(13),
  /** No space on device */
  FATAL_DISK_FULL(14),

  /** Indicates that the buck fix script returned a non-zero exit */
  FIX_FAILED(16),

  // Other non-fatal errors 20 - 127

  /** Test run had user-specific test errors */
  TEST_ERROR(32),
  /** There was no tests found to run */
  TEST_NOTHING(64),

  // Signal processors 128+

  /** Command was interrupted (Ctrl + C) */
  SIGNAL_INTERRUPT(130);

  private final int code;

  ExitCode(int code) {
    this.code = code;
  }

  /**
   * @return integer value of the exit code
   */
  @JsonValue
  public int getCode() {
    return code;
  }

  /**
   * @return true if error is Buck internal non-recoverable failure
   */
  public boolean isFatal() {
    return code >= 10 && code < 20;
  }

  static final ImmutableMap<Integer, ExitCode> EXIT_CODE_MAPPING =
      ImmutableMap.of(
          0,
          ExitCode.SUCCESS,
          32,
          ExitCode.TEST_ERROR,
          42,
          ExitCode.TEST_ERROR,
          64,
          ExitCode.TEST_NOTHING,
          70,
          ExitCode.TEST_ERROR);

  /**
   * Map integer value received from custom toolchain subcall to one of appropriate ExitCode values.
   * This function is for backwards compatibility only, please construct ExitCode directly instead
   */
  public static ExitCode map(int code) {
    return EXIT_CODE_MAPPING.getOrDefault(code, ExitCode.BUILD_ERROR);
  }
}
