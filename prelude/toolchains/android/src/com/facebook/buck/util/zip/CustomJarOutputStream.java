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
import com.google.common.hash.HashFunction;
import com.google.common.hash.Hasher;
import com.google.common.hash.Hashing;
import java.io.IOException;
import java.io.OutputStream;
import java.util.Objects;
import java.util.jar.JarFile;
import java.util.zip.ZipEntry;
import org.jspecify.annotations.Nullable;

/** Extension of {@link CustomZipOutputStream} with jar-specific functionality. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class CustomJarOutputStream extends CustomZipOutputStream {
  public static final String DIGEST_ATTRIBUTE_NAME = "Murmur3-128-Digest";
  private final HashingImpl impl;

  public CustomJarOutputStream(Impl impl) {
    this(new HashingImpl(impl));
  }

  private CustomJarOutputStream(HashingImpl impl) {
    super(impl);
    this.impl = impl;
  }

  public DeterministicManifest getManifest() {
    return impl.getManifest();
  }

  public void setEntryHashingEnabled(boolean shouldHashEntries) {
    impl.setEntryHashingEnabled(shouldHashEntries);
  }

  public void writeManifest() throws IOException {
    impl.writeManifest();
  }

  private static class HashingImpl extends OutputStream implements Impl {
    private static final HashFunction HASH_FUNCTION = Hashing.murmur3_128();

    private final Impl inner;
    private final DeterministicManifest manifest = new DeterministicManifest();
    private boolean shouldHashEntries = false;
    private boolean manifestWritten = false;

    @Nullable private ZipEntry currentEntry;
    @Nullable private Hasher hasher;

    HashingImpl(Impl inner) {
      this.inner = inner;
    }

    public DeterministicManifest getManifest() {
      return manifest;
    }

    public void setEntryHashingEnabled(boolean shouldHashEntries) {
      this.shouldHashEntries = shouldHashEntries;
    }

    @Override
    public void actuallyPutNextEntry(ZipEntry entry) throws IOException {
      inner.actuallyPutNextEntry(entry);

      if (shouldHashEntries && !entry.isDirectory() && hasher == null) {
        hasher = HASH_FUNCTION.newHasher();
      }

      currentEntry = entry;
    }

    @Override
    public void actuallyWrite(byte[] b, int off, int len) throws IOException {
      inner.actuallyWrite(b, off, len);

      if (hasher != null) {
        hasher.putBytes(b, off, len);
      }
    }

    @Override
    public void actuallyCloseEntry() throws IOException {
      inner.actuallyCloseEntry();

      if (hasher != null) {
        if (manifestWritten) {
          throw new IllegalStateException(
              "Attempted to write an entry with hashing enabled after the manifest was written.");
        }
        manifest.setEntryAttribute(
            Objects.requireNonNull(currentEntry).getName(),
            DIGEST_ATTRIBUTE_NAME,
            hasher.hash().toString());
        hasher = null;
      }

      currentEntry = null;
    }

    @Override
    public void actuallyClose() throws IOException {
      shouldHashEntries = false;
      writeManifest();

      inner.actuallyClose();
    }

    protected void writeManifest() throws IOException {
      if (shouldHashEntries || manifestWritten) {
        return;
      }

      inner.actuallyPutNextEntry(new CustomZipEntry(JarFile.MANIFEST_NAME));
      try {
        manifest.write(this);
      } finally {
        inner.actuallyCloseEntry();
      }

      manifestWritten = true;
    }

    @Override
    public void write(byte[] b, int off, int len) throws IOException {
      actuallyWrite(b, off, len);
    }

    @Override
    public void write(int b) {
      throw new UnsupportedOperationException();
    }
  }
}
