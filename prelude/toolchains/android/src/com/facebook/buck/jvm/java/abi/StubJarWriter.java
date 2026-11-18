/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi;

import com.facebook.buck.util.function.ThrowingSupplier;
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Path;

/** An interface for writing to stub jars. */
@Nullsafe(Nullsafe.Mode.LOCAL)
interface StubJarWriter extends AutoCloseable {
  void writeEntry(Path relativePath, ThrowingSupplier<InputStream, IOException> streamSupplier);

  @Override
  void close() throws IOException;
}
