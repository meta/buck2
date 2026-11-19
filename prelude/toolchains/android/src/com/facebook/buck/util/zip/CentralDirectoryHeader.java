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

import static java.util.Collections.emptyList;
import static java.util.Optional.ofNullable;

import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.Path;
import java.util.Collections;
import java.util.List;

/**
 * END Header fields, used to locate the ZIP Central Directory. Load existing Central Directory
 * header information, such as offset position and previous list of files. It's used to support
 * append operations by preserving header of previous files.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class CentralDirectoryHeader extends CentralDirectory {

  private final Path path;
  private final int count;
  private final long size;
  private final long offset;

  private List<CentralDirectoryFileHeader> files;

  public CentralDirectoryHeader(Path path) {
    this(path, 0, 0, 0);
  }

  public CentralDirectoryHeader(Path path, int count, long size, long offset) {
    this.path = path;
    this.count = count;
    this.size = size;
    this.offset = offset;
    this.files = emptyList();
  }

  public Path getPath() {
    return path;
  }

  public List<CentralDirectoryFileHeader> getFiles() {
    return files;
  }

  public int getCount() {
    return count;
  }

  public long getSize() {
    return size;
  }

  public long getOffset() {
    return offset;
  }

  void setFiles(List<CentralDirectoryFileHeader> files) {
    this.files = ofNullable(files).orElseGet(Collections::emptyList);
  }

  boolean isValid() {
    return offset >= 0 && size > 0 && count > 0;
  }

  boolean isZip64() {
    return isZip64MagicVal(offset) || isZip64MagicVal(size) || isZip64MagicCount(count);
  }

  @Override
  public String toString() {
    return "CentralDirectoryHeader{"
        + "path="
        + path
        + ", count="
        + count
        + ", size="
        + size
        + ", offset="
        + offset
        + ", files="
        + files.size()
        + '}';
  }
}
