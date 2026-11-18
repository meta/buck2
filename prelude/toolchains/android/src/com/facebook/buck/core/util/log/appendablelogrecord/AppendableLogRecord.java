/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.core.util.log.appendablelogrecord;

import com.facebook.infer.annotation.Nullsafe;
import java.util.logging.Level;
import java.util.logging.LogRecord;

/**
 * Subclass of LogRecord that only accepts preformatted strings. LogFormatter downcasts if it
 * receives AppendableLogRecord instances, allowing us to avoid string allocations.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class AppendableLogRecord extends LogRecord {
  public AppendableLogRecord(Level level, String msg) {
    super(level, msg);
  }

  public void appendFormattedMessage(StringBuilder sb) {
    sb.append(getMessage());
  }
}
