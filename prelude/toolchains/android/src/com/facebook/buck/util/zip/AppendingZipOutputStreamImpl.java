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
import java.util.HashSet;
import java.util.LinkedList;
import java.util.List;
import java.util.Objects;
import java.util.Set;
import java.util.zip.ZipEntry;
import java.util.zip.ZipException;
import org.jspecify.annotations.Nullable;

/**
 * A drop-in replacement for (@link java.util.zip.ZipOutStream} that supports the ability to set a
 * compression level and allows multiple entries with the same name.
 *
 * <p><a href="https://users.cs.jmu.edu/buchhofp/forensics/formats/pkzip.html">
 * https://users.cs.jmu.edu/buchhofp/forensics/formats/pkzip.html </a> <a
 * href="http://www.pkware.com/documents/casestudies/APPNOTE.TXT">
 * http://www.pkware.com/documents/casestudies/APPNOTE.TXT </a>
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class AppendingZipOutputStreamImpl implements CustomZipOutputStream.Impl {

  private final OutputStream delegate;
  private final boolean throwExceptionsOnDuplicate;
  private final List<EntryAccounting> entries = new LinkedList<>();
  private final Set<String> seenNames = new HashSet<>();

  private long currentOffset;

  @Nullable private EntryAccounting currentEntry = null;

  public AppendingZipOutputStreamImpl(OutputStream stream, boolean throwExceptionsOnDuplicate) {
    this.delegate = stream;
    this.throwExceptionsOnDuplicate = throwExceptionsOnDuplicate;
  }

  @Override
  public void actuallyWrite(byte[] b, int off, int len) throws IOException {
    Objects.requireNonNull(currentEntry);
    currentEntry.write(delegate, b, off, len);
  }

  @Override
  public void actuallyPutNextEntry(ZipEntry entry) throws IOException {
    if (throwExceptionsOnDuplicate && hasEntry(entry.getName())) {
      // Same exception as ZipOutputStream.
      throw new ZipException("duplicate entry: " + entry.getName());
    }
    currentEntry = new EntryAccounting(entry, currentOffset);
    currentOffset += currentEntry.writeLocalFileHeader(delegate);
    addEntry(currentEntry);
  }

  protected boolean hasEntry(String name) {
    return seenNames.contains(name);
  }

  protected void addEntry(EntryAccounting entry) {
    seenNames.add(entry.getName());
    entries.add(entry);
  }

  protected long getCurrentOffset() {
    return currentOffset;
  }

  protected void setCurrentOffset(long currentOffset) {
    this.currentOffset = currentOffset;
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
    getCentralDirectory().writeCentralDirectory(delegate, currentOffset, entries);
    delegate.close();
  }

  protected CentralDirectory getCentralDirectory() {
    return new CentralDirectory();
  }

  protected OutputStream getDelegate() {
    return delegate;
  }
}
