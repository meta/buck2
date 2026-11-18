/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.zip;

import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.io.OutputStream;
import java.util.zip.GZIPOutputStream;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class BestCompressionGZIPOutputStream extends GZIPOutputStream {

  public BestCompressionGZIPOutputStream(OutputStream out, boolean syncFlush) throws IOException {
    super(out, syncFlush);
    def.setLevel(9);
  }
}
