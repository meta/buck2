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
import java.io.BufferedInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.stream.Collectors;
import java.util.stream.Stream;
import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;

/** A {@link LibraryReader} that reads from a directory (recursively). */
@Nullsafe(Nullsafe.Mode.LOCAL)
class DirectoryReader implements LibraryReader {
  private final Path root;

  public DirectoryReader(Path root) {
    this.root = root;
  }

  @Override
  public List<Path> getRelativePaths() throws IOException {
    try (Stream<Path> paths = Files.walk(root)) {
      return paths
          .filter(path -> !Files.isDirectory(path))
          .map(root::relativize)
          .collect(Collectors.toList());
    }
  }

  @Override
  public InputStream openResourceFile(Path relativePath) throws IOException {
    if (!isResource(relativePath)) {
      throw new IllegalArgumentException();
    }
    return openInputStream(relativePath);
  }

  @Override
  public void visitClass(Path relativePath, ClassVisitor cv, boolean skipCode) throws IOException {
    if (!isClass(relativePath)) {
      throw new IllegalArgumentException();
    }

    int parsingOptions = ClassReader.SKIP_FRAMES;
    if (skipCode) {
      parsingOptions |= ClassReader.SKIP_DEBUG | ClassReader.SKIP_CODE;
    }

    try (InputStream inputStream = openInputStream(relativePath)) {
      ClassReader reader = new ClassReader(inputStream);
      reader.accept(cv, parsingOptions);
    }
  }

  @Override
  public void close() {
    // Nothing in particular needed
  }

  @Override
  public InputStream openInputStream(Path relativePath) throws IOException {
    return new BufferedInputStream(Files.newInputStream(root.resolve(relativePath)));
  }
}
