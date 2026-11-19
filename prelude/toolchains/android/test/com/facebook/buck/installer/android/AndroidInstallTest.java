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

import java.time.Instant;
import java.time.ZoneId;
import org.junit.Test;

public class AndroidInstallTest {

  @Test
  public void completionMessageIncludesTimestamp() {
    assertEquals(
        "Install of basel_split_arm64_exo-native.apk finished in 5 seconds at 2026-08-27 14:05:09",
        AndroidInstall.formatCompletionMessage(
            "basel_split_arm64_exo-native.apk",
            Instant.parse("2026-08-27T21:05:04Z"),
            Instant.parse("2026-08-27T21:05:09.987654321Z"),
            ZoneId.of("America/Los_Angeles")));
  }

  /** A step stopped at its time limit is interrupted, and its exception carries no message. */
  @Test
  public void failureWithoutAMessageIsNamedByItsException() {
    assertEquals(
        "java.lang.InterruptedException",
        AndroidInstall.failureMessage(new InterruptedException()));
  }

  @Test
  public void failureWithAMessageReportsTheMessage() {
    assertEquals(
        "device offline",
        AndroidInstall.failureMessage(new IllegalStateException("device offline")));
  }
}
