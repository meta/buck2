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

import static com.facebook.buck.util.zip.ByteIo.*;
import static java.lang.Math.toIntExact;
import static java.nio.file.StandardOpenOption.READ;

import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.channels.SeekableByteChannel;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.zip.ZipEntry;
import org.jspecify.annotations.Nullable;

/**
 * Utility class designed to read and parse the central directory of a ZIP file. The central
 * directory is a critical component of a ZIP file, containing metadata about the files within the
 * archive. The class parses the central directory header, extracting essential information such as
 * the number of files, total size, and offset.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class CentralDirectoryReader {

  /**
   * @param src file to load the header information
   * @return Central directory with preloaded files (CENSIG list)
   * @throws IOException In case it fails to read the src information.
   * @see #readCentralDirectory(Path, boolean)
   */
  public CentralDirectoryHeader readCentralDirectory(final Path src) throws IOException {
    return readCentralDirectory(src, true);
  }

  /**
   * @param src file to load the header information
   * @param readFiles If it should parse CD in order to load file headers.
   * @return Central directory with preloaded files (CENSIG list)
   * @throws IOException In case it fails to read the src information.
   */
  public CentralDirectoryHeader readCentralDirectory(final Path src, final boolean readFiles)
      throws IOException {
    if (Files.exists(src)) {
      try (final SeekableByteChannel byteChannel = Files.newByteChannel(src, READ)) {

        final long fileSize = byteChannel.size();
        // Central Directory header will be on the last 64kb of the file
        final int headerSize = (int) Math.min(fileSize, ZipConstants.END_MAXLEN);
        final long headerOffset = fileSize - headerSize;

        // not enough data or invalid
        if (fileSize >= headerSize) {

          final ByteBuffer headerBuffer = readBytes(byteChannel, headerOffset, headerSize);
          final CentralDirectoryHeader header =
              readCentralDirectory(src, headerBuffer.rewind(), headerOffset);

          if (header != null && header.isValid()) {
            if (readFiles) {
              final ByteBuffer centralDirectoryBuffer =
                  readBytes(byteChannel, header.getOffset(), header.getSize());
              header.setFiles(readFileHeaders(centralDirectoryBuffer.rewind(), header.getCount()));
            }
            return header;
          }
        }
      }
    }
    return new CentralDirectoryHeader(src);
  }

  /**
   * @param src Path to zip to read its Central Directory
   * @param headerBuffer Buffer containing Central Directory headers.
   * @param bufferOffset position at the file which the buffer is positioned.
   * @return Central Directory Header
   * @throws IOException In case it fails to read the header.
   */
  private @Nullable CentralDirectoryHeader readCentralDirectory(
      final Path src, final ByteBuffer headerBuffer, final long bufferOffset) throws IOException {
    // position channel to read the last block of the file.
    final byte[] buffer = headerBuffer.array();
    final int bufferSize = headerBuffer.remaining();
    final long totalSize = bufferOffset + bufferSize;
    // scan the block backwards for ENDSIG header signature
    for (int pos = bufferSize - ZipEntry.ENDHDR; pos >= 0; pos--) {
      if (readInt(buffer, pos) == ZipEntry.ENDSIG) {
        // Offset of the start of the central directory.
        final CentralDirectoryHeader header =
            new CentralDirectoryHeader(
                src,
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
          if (end64Offset < bufferOffset || end64Offset > totalSize) {
            return header;
          }
          // recalculate offset inside the buffer
          end64Pos = (int) (end64Offset - bufferOffset);
          // if the position is ENDSIG
          if (readInt(buffer, end64Pos) == ZipConstants.ZIP64_ENDSIG) {
            final CentralDirectoryHeader header64 =
                new CentralDirectoryHeader(
                    src,
                    toIntExact(readLong(buffer, end64Pos + ZipConstants.ZIP64_ENDTOT)),
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
    return null;
  }

  /**
   * Read all CDFile headers within the buffer.
   *
   * @param buffer Buffer that contains all Central Directory headers.
   * @param expectedSize Expected number of file headers, obtained from EOCD
   * @return All File headers.
   */
  private List<CentralDirectoryFileHeader> readFileHeaders(
      final ByteBuffer buffer, final int expectedSize) {

    final byte[] bytes = buffer.array();
    final List<CentralDirectoryFileHeader> files = new ArrayList<>(expectedSize);

    // verify is a header CDFile header block is available
    int offset = 0;
    while (files.size() < expectedSize) {
      final CentralDirectoryFileHeader file = new CentralDirectoryFileHeader();
      if (!file.read(bytes, offset)) {
        break;
      }
      files.add(file);
      offset = file.nextOffset();
    }
    return files;
  }

  /**
   * Reads a specified number of bytes from a seek-able channel at a given offset and returns them
   * as a ByteBuffer.
   */
  private ByteBuffer readBytes(SeekableByteChannel byteChannel, long offset, long size)
      throws IOException {
    final ByteBuffer buffer = ByteBuffer.allocate(toIntExact(size));
    byteChannel.position(offset);
    // try to fill the buffer
    while (buffer.hasRemaining()) {
      if (byteChannel.read(buffer) <= 0) {
        break;
      }
    }
    // buffer not fully read
    if (buffer.hasRemaining()) {
      throw new IOException(String.format("failed to read %s bytes at offset %s", size, offset));
    }
    return buffer;
  }
}
