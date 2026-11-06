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

import static java.nio.file.StandardOpenOption.CREATE;
import static java.nio.file.StandardOpenOption.READ;
import static java.nio.file.StandardOpenOption.WRITE;

import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.channels.Channels;
import java.nio.channels.FileChannel;
import java.nio.file.Path;
import javax.annotation.Nonnull;

/**
 * Alternative implementation to dependency packing, instead of copying several jars which is slow,
 * make use of a previous packed jar with immutable dependencies and append on top of it.
 *
 * @see AppendableCentralDirectory
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class IncrementalJarOutputStreamImpl extends AppendingZipOutputStreamImpl {

  /**
   * Create a copy of a jar copying previous existing files and loading Central Directory to execute
   * append operation.
   *
   * @see AppendableCentralDirectory
   */
  public static IncrementalJarOutputStreamImpl createIncrementalJar(Path src, Path dst)
      throws IOException {

    final AppendableCentralDirectory centralDirectory =
        AppendableCentralDirectory.readCentralDirectory(src);

    try (final FileChannel srcChannel = FileChannel.open(src, READ)) {
      final FileChannel dstChannel = FileChannel.open(dst, CREATE, WRITE);

      // copy the block of files that exists in the input jar
      // transferFrom may not transfer all requested bytes in a single call,
      // so we need to loop until all bytes are transferred
      long bytesToTransfer = centralDirectory.getEntryOffset();
      long position = 0;
      while (position < bytesToTransfer) {
        long transferred =
            dstChannel.transferFrom(srcChannel, position, bytesToTransfer - position);
        if (transferred == 0) {
          throw new IOException(
              "Failed to transfer bytes from source jar: transferred 0 bytes at position "
                  + position
                  + " of "
                  + bytesToTransfer);
        }
        position += transferred;
      }
      dstChannel.position(centralDirectory.getEntryOffset());

      return new IncrementalJarOutputStreamImpl(
          Channels.newOutputStream(dstChannel), centralDirectory);
    }
  }

  private final AppendableCentralDirectory appendableDirectory;

  IncrementalJarOutputStreamImpl(OutputStream stream) {
    this(stream, new AppendableCentralDirectory());
  }

  public IncrementalJarOutputStreamImpl(
      final OutputStream outputStream, final AppendableCentralDirectory appendableDirectory) {
    super(outputStream, false);
    this.appendableDirectory = appendableDirectory;
    setCurrentOffset(appendableDirectory.getEntryOffset());
  }

  @Nonnull
  @Override
  protected CentralDirectory getCentralDirectory() {
    return appendableDirectory;
  }
}
