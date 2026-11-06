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
import java.io.ByteArrayOutputStream;
import java.io.PrintStream;
import java.io.UnsupportedEncodingException;
import java.nio.charset.Charset;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class CapturingPrintStream extends PrintStream {

  private final ByteArrayOutputStream byteArrayOutputStream;

  public CapturingPrintStream() {
    this(new ByteArrayOutputStream());
  }

  private CapturingPrintStream(ByteArrayOutputStream byteArrayOutputStream) {
    super(byteArrayOutputStream);
    this.byteArrayOutputStream = byteArrayOutputStream;
  }

  public String getContentsAsString(Charset charset) {
    try {
      return byteArrayOutputStream.toString(charset.name());
    } catch (UnsupportedEncodingException e) {
      throw new RuntimeException(e);
    }
  }
}
