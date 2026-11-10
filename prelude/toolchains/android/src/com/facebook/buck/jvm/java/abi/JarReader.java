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
import com.google.common.base.Preconditions;
import com.google.common.collect.Iterables;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.FileSystem;
import java.nio.file.FileSystems;
import java.nio.file.Path;
import java.util.List;
import javax.annotation.Nullable;
import org.objectweb.asm.ClassVisitor;

/** A {@link LibraryReader} that reads from a jar file. */
@Nullsafe(Nullsafe.Mode.LOCAL)
class JarReader implements LibraryReader {
  private final Path jarPath;
  @Nullable FileSystem fileSystem;
  @Nullable private DirectoryReader inner;
  private boolean closed = false;

  JarReader(Path jarPath) {
    this.jarPath = jarPath;
  }

  @Override
  public List<Path> getRelativePaths() throws IOException {
    return getInner().getRelativePaths();
  }

  @Override
  public InputStream openResourceFile(Path relativePath) throws IOException {
    return getInner().openResourceFile(relativePath);
  }

  @Override
  public InputStream openInputStream(Path relativePath) throws IOException {
    return getInner().openInputStream(relativePath);
  }

  @Override
  public void visitClass(Path relativePath, ClassVisitor cv, boolean skipCode) throws IOException {
    getInner().visitClass(relativePath, cv, skipCode);
  }

  @Override
  public void close() throws IOException {
    if (fileSystem != null) {
      fileSystem.close();
      fileSystem = null;
      inner = null;
      closed = true;
    }
  }

  private DirectoryReader getInner() throws IOException {
    // Actually had a file descriptor leak because something was reopening this after it was closed,
    // so let's make sure we crash and detect it if it happens again.
    Preconditions.checkState(!closed);
    if (inner == null) {
      fileSystem = FileSystems.newFileSystem(jarPath, (ClassLoader) null);
      inner = new DirectoryReader(Iterables.getOnlyElement(fileSystem.getRootDirectories()));
    }

    return inner;
  }
}
