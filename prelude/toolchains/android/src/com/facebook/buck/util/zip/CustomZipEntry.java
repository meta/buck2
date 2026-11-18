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

import static java.util.zip.Deflater.BEST_COMPRESSION;
import static java.util.zip.Deflater.NO_COMPRESSION;

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Preconditions;
import java.io.File;
import java.nio.file.Path;
import java.util.zip.Deflater;
import java.util.zip.ZipEntry;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class CustomZipEntry extends ZipEntry {

  private int compressionLevel = Deflater.DEFAULT_COMPRESSION;
  private long externalAttributes = 0;
  private boolean rawCopy = false;

  public CustomZipEntry(ZipEntry other) {
    super(other);
    setDefaultMethodAndTimeIfUnset();
  }

  public CustomZipEntry(String name) {
    super(name);
    setDefaultMethodAndTimeIfUnset();
  }

  public CustomZipEntry(Path path, boolean isDirectory) {
    this(path.toString().replace(File.separatorChar, '/') + (isDirectory ? "/" : ""));
  }

  public CustomZipEntry(Path path) {
    this(path, false);
  }

  private void setDefaultMethodAndTimeIfUnset() {
    if (getMethod() == -1) {
      setMethod(DEFLATED);
    }
    if (getTime() == -1) {
      setFakeTime();
    }
  }

  public void setFakeTime() {
    setTime(ZipConstants.getFakeTime());
  }

  public void setCompressionLevel(int compressionLevel) {
    Preconditions.checkArgument(
        compressionLevel >= NO_COMPRESSION && compressionLevel <= BEST_COMPRESSION);
    this.compressionLevel = compressionLevel;

    // We need to update the underlying method declared
    setMethod(compressionLevel == NO_COMPRESSION ? STORED : DEFLATED);

    // Reset the various fields that need to be updated.
    setCrc(0);
    setSize(0);
    setCompressedSize(0);
  }

  public int getCompressionLevel() {
    return compressionLevel;
  }

  public long getExternalAttributes() {
    return externalAttributes;
  }

  public void setExternalAttributes(long externalAttributes) {
    this.externalAttributes = externalAttributes;
  }

  /**
   * Marks this entry for raw copy mode. When set, the data written to the zip output stream is
   * treated as pre-compressed bytes — no CRC computation or deflation is performed. The caller must
   * set CRC, size, and compressedSize on this entry before writing.
   */
  public void setRawCopy(boolean rawCopy) {
    this.rawCopy = rawCopy;
  }

  public boolean isRawCopy() {
    return rawCopy;
  }
}
