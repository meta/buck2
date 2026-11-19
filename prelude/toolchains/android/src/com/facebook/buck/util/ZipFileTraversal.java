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
import java.io.IOException;
import java.nio.file.Path;
import java.util.Enumeration;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

@Nullsafe(Nullsafe.Mode.LOCAL)
public abstract class ZipFileTraversal {
  private final Path file;

  public ZipFileTraversal(Path file) {
    this.file = file;
  }

  public abstract void visit(ZipFile zipFile, ZipEntry zipEntry) throws IOException;

  public final void traverse() throws IOException {
    try (ZipFile zipFile = new ZipFile(file.toFile())) {
      Enumeration<? extends ZipEntry> entries = zipFile.entries();
      while (entries.hasMoreElements()) {
        ZipEntry entry = entries.nextElement();
        visit(zipFile, entry);
      }
    } catch (IllegalArgumentException e) { // help debugging a "MALFORMED" error
      throw new IllegalArgumentException("zipfile traverse exception on file:" + file, e);
    }
  }
}
