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

import static com.facebook.buck.util.zip.ByteIo.readInt;
import static com.facebook.buck.util.zip.ByteIo.readLong;
import static com.facebook.buck.util.zip.ByteIo.readShort;
import static com.facebook.buck.util.zip.ByteIo.readUnsignedInt;
import static java.nio.file.StandardOpenOption.READ;

import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.channels.SeekableByteChannel;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.zip.ZipEntry;
import org.jspecify.annotations.Nullable;

/**
 * Load existing Central Directory header information, such as offset position and previous list of
 * files. It's used to support append operations by preserving header of previous files.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class AppendableCentralDirectory extends CentralDirectory {

  /** END Header fields, used to locate the ZIP Central Directory. */
  static class Header {

    private final int count;
    private final long size;
    private final long offset;

    Header(int count, long size, long offset) {
      this.count = count;
      this.size = size;
      this.offset = offset;
    }

    int getCount() {
      return count;
    }

    long getSize() {
      return size;
    }

    long getOffset() {
      return offset;
    }

    boolean isValid() {
      return offset >= 0 && size > 0 && count > 0;
    }

    boolean isZip64() {
      return isZip64MagicVal(offset) || isZip64MagicVal(size) || isZip64MagicCount(count);
    }
  }

  static @Nullable Header readHeader(SeekableByteChannel file) throws IOException {
    final long fileSize = file.size();
    // Central Directory header will be on the last 64kb of the file
    final int headerSize = (int) Math.min(fileSize, ZipConstants.END_MAXLEN);
    final long headerPosition = fileSize - headerSize;
    // not enough data or invalid
    if (fileSize >= headerSize) {
      final ByteBuffer headerBuffer = ByteBuffer.allocate(headerSize);
      // position channel to read the last block of the file.
      file.position(headerPosition);
      file.read(headerBuffer);

      final byte[] buffer = headerBuffer.array();
      final int bufferSize = headerBuffer.rewind().remaining();

      // scan the block backwards for ENDSIG header signature
      for (int pos = bufferSize - ZipEntry.ENDHDR; pos >= 0; pos--) {
        if (readInt(buffer, pos) == ZipEntry.ENDSIG) {
          // Offset of the start of the central directory.
          final Header header =
              new Header(
                  readShort(buffer, pos + ZipEntry.ENDTOT),
                  readUnsignedInt(buffer, pos + ZipEntry.ENDSIZ),
                  readUnsignedInt(buffer, pos + ZipEntry.ENDOFF));
          // if theres no zip64 magic values.
          if (header.isZip64()) {
            int end64Pos = pos - ZipConstants.ZIP64_LOCHDR;
            // check if zip64 signature is available
            if (end64Pos <= 0 || readInt(buffer, end64Pos) != ZipConstants.ZIP64_LOCSIG) {
              return header;
            }
            // read zip64 offset in the file
            long end64Offset = readLong(buffer, end64Pos + ZipConstants.ZIP64_LOCOFF);
            // has to be a position within the tail.
            if (end64Offset < headerPosition || end64Offset > fileSize) {
              return header;
            }
            // recalculate offset inside the buffer
            end64Pos = (int) (end64Offset - headerPosition);
            // if the position is ENDSIG
            if (readInt(buffer, end64Pos) == ZipConstants.ZIP64_ENDSIG) {
              final Header header64 =
                  new Header(
                      (int) readLong(buffer, end64Pos + ZipConstants.ZIP64_ENDTOT),
                      readLong(buffer, end64Pos + ZipConstants.ZIP64_ENDSIZ),
                      readLong(buffer, end64Pos + ZipConstants.ZIP64_ENDOFF));
              if (header64.isValid()) {
                return header64;
              }
            }
          }
          return header;
        }
      }
    }
    return null;
  }

  /**
   * @param src file to load the header information
   * @return Central directory with preloaded files (CENSIG list)
   * @throws IOException In case it fails to read the src information.
   */
  public static AppendableCentralDirectory readCentralDirectory(Path src) throws IOException {
    try (final SeekableByteChannel file = Files.newByteChannel(src, READ)) {
      final Header header = readHeader(file);
      // found valid offset, load header, so it can be appended with new entries.
      if (header != null && header.isValid()) {
        final ByteBuffer buffer = ByteBuffer.allocate((int) header.getSize());
        file.position(header.getOffset());
        file.read(buffer);
        return new AppendableCentralDirectory(header.getCount(), header.getOffset(), buffer);
      }
    }
    return new AppendableCentralDirectory();
  }

  private final int entryCount;
  private final long entryOffset;
  private final ByteBuffer header;

  AppendableCentralDirectory() {
    this(0, 0, ByteBuffer.allocate(0));
  }

  AppendableCentralDirectory(
      final int entryCount, final long entryOffset, final ByteBuffer header) {
    this.entryCount = entryCount;
    this.entryOffset = entryOffset;
    this.header = header;
  }

  /**
   * @return Number of entries current available in the zip central directory.
   */
  public int getEntryCount() {
    return entryCount;
  }

  /** Position of bytes where the files in the zip ended and the header start. */
  public long getEntryOffset() {
    return entryOffset;
  }

  /**
   * Prepend previous list of files in the zip.
   *
   * @return Total bytes prepended.
   */
  @Override
  protected long beforeWrite(OutputStream out) throws IOException {
    int headerSize = header.capacity();
    if (headerSize > 0) {
      out.write(header.array(), 0, headerSize);
    }
    return headerSize;
  }

  /**
   * @return Number of files previous present in the zip.
   */
  @Override
  protected int beforeEntries() {
    return entryCount;
  }
}
