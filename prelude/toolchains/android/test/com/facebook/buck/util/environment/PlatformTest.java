/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.environment;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import org.junit.Test;

public class PlatformTest {
  /**
   * Check that Platform detects this OS. Note: if you're running on a reasonably mainstream OS and
   * this test fails, or if Buck's CI is failing on this, you should report a bug / add a test for
   * this in Platform. We shouldn't get {@code UNKNOWN} here.
   */
  @Test
  public void detectTest() {
    assertNotEquals(Platform.UNKNOWN, Platform.detect());
  }

  /**
   * Unix-like OSes only: try to get the "null" special filesystem entry. Almost certain to be
   * {@code /dev/null}. But let's verify our basic assumptions about the OS.
   */
  @Test
  public void nullFileTest() throws IOException {
    Platform platform = Platform.detect();

    File nullFile = platform.getNullDevicePath().toFile();

    if (platform.getType().isUnix()) {
      // Unix: `/dev/null`, though a device node, is a legit filesystem entry nonetheless.
      assertTrue(nullFile.exists());
    }

    // According to POSIX and Windows literature, the null file can be opened; all writes are
    // accepted (and their data discarded); and all reads result in EOF.  Try all of these, except
    // for writing.
    try (FileInputStream nullInputStream = new FileInputStream(nullFile)) {
      assertEquals(-1, nullInputStream.read());
    }
  }
}
