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

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.util.function.ThrowingSupplier;
import com.facebook.infer.annotation.Nullsafe;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Path;
import java.util.Objects;

/*
 * Class to write stub jar entries as class files
 * */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class StubClassFileWriter implements StubJarWriter {

  private final AbsPath outputPath;
  private boolean closed = false;

  public StubClassFileWriter(AbsPath outputPath) {
    this.outputPath = outputPath;
  }

  @Override
  public void writeEntry(
      Path relativePath, ThrowingSupplier<InputStream, IOException> streamSupplier) {

    if (!outputPath.toFile().isDirectory()) {
      throw new IllegalArgumentException("Output Path should be a directory");
    }

    try {
      // Ensure parent directory exists, throw if creation fails
      File parentDir =
          new File(
              outputPath.toFile(), Objects.requireNonNull(relativePath.getParent()).toString());
      if (!parentDir.exists() && !parentDir.mkdirs()) {
        throw new IOException("Failed to create directory: " + parentDir);
      }

      File classFile = new File(parentDir, relativePath.getFileName().toString());

      // Use try-with-resources for automatic resource management
      try (InputStream inputStream = streamSupplier.get();
          FileOutputStream fos = new FileOutputStream(classFile)) {

        byte[] buffer = new byte[8192];
        int bytesRead;
        while ((bytesRead = inputStream.read(buffer)) != -1) {
          fos.write(buffer, 0, bytesRead);
        }
      }
    } catch (IOException e) {
      throw new RuntimeException(e);
    }
  }

  @Override
  public void close() throws IOException {}
}
