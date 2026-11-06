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
import java.util.LinkedList;
import java.util.List;
import java.util.Objects;
import java.util.zip.ZipEntry;
import javax.annotation.Nullable;

/**
 * An implementation of {@link CustomZipOutputStream.Impl} that doesn't perform any checks around
 * duplicate entries.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class SimpleZipOutputStreamImpl implements CustomZipOutputStream.Impl {

  private final OutputStream delegate;
  private long currentOffset = 0;
  private List<EntryAccounting> entries = new LinkedList<>();
  @Nullable private EntryAccounting currentEntry = null;

  public SimpleZipOutputStreamImpl(OutputStream stream) {
    this.delegate = stream;
  }

  @Override
  public void actuallyWrite(byte[] b, int off, int len) throws IOException {
    Objects.requireNonNull(currentEntry).write(delegate, b, off, len);
  }

  @Override
  public void actuallyPutNextEntry(ZipEntry entry) throws IOException {
    currentEntry = new EntryAccounting(entry, currentOffset);
    entries.add(currentEntry);
    currentOffset += currentEntry.writeLocalFileHeader(delegate);
  }

  @Override
  public void actuallyCloseEntry() throws IOException {
    if (currentEntry == null) {
      return; // no-op
    }
    currentOffset += currentEntry.finish(delegate);
    currentEntry = null;
  }

  @Override
  public void actuallyClose() throws IOException {
    new CentralDirectory().writeCentralDirectory(delegate, currentOffset, entries);
    delegate.close();
  }
}
