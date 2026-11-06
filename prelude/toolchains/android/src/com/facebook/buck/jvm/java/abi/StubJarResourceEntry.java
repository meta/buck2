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

import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.Path;
import java.util.Collections;
import java.util.List;

@Nullsafe(Nullsafe.Mode.LOCAL)
class StubJarResourceEntry extends StubJarEntry {
  private final LibraryReader input;
  private final Path path;

  public static StubJarResourceEntry of(LibraryReader input, Path path) {
    return new StubJarResourceEntry(input, path);
  }

  private StubJarResourceEntry(LibraryReader input, Path path) {
    this.input = input;
    this.path = path;
  }

  @Override
  public void write(StubJarWriter writer) {
    writer.writeEntry(path, () -> input.openResourceFile(path));
  }

  @Override
  public List<String> getInlineFunctions() {
    return Collections.emptyList();
  }

  @Override
  public boolean extendsInlineFunctionScope() {
    return false;
  }
}
