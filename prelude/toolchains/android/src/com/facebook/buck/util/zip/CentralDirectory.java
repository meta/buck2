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

import static com.facebook.buck.util.zip.ByteIo.writeInt;
import static com.facebook.buck.util.zip.ByteIo.writeLong;
import static com.facebook.buck.util.zip.ByteIo.writeShort;

import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.zip.ZipEntry;

/**
 * Each zip file has a "central directory" at the end of the archive, which provides the indexes
 * required for fast random access to the contents of the zip. This class models that.
 *
 * <p>The central directory consists of a series of "file headers", describing each entry in the
 * zip, and a "end of central directory" signature containing book keeping information.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class CentralDirectory {

  /** Execute before the central directory is created and return number of bytes prepended. */
  protected long beforeWrite(OutputStream out) throws IOException {
    return 0;
  }

  /**
   * Execute before the central directory is created and return number of entries added. Will be
   * used as offset number of entries.
   */
  protected int beforeEntries() {
    return 0;
  }

  /**
   * Write the entire central directory, including the file headers and the end of central directory
   * signature.
   *
   * @param out The stream to output to.
   * @param startOffset The number of bytes offset within the zip file that this starts at.
   * @param entries The entries that are contained within the zip.
   * @throws IOException Should something go awry.
   */
  public void writeCentralDirectory(
      final OutputStream out, final long startOffset, final Iterable<EntryAccounting> entries)
      throws IOException {

    long size = beforeWrite(out);
    int entryCount = beforeEntries();

    for (EntryAccounting entry : entries) {
      entryCount++;
      size += writeCentralDirectoryFileHeader(out, entry);
    }

    boolean useZip64 = false;

    useZip64 =
        isZip64MagicVal(startOffset) || isZip64MagicVal(size) || isZip64MagicCount(entryCount);

    if (useZip64) {
      // Zip64 end of central directory record.
      writeInt(out, ZipConstants.ZIP64_ENDSIG);
      writeLong(out, ZipConstants.ZIP64_ENDHDR - 12);
      // Version made by.
      writeShort(out, 45);
      // Version needed to extract.
      writeShort(out, 45);
      // Number of this disk.
      writeInt(out, 0);
      // Number of the disk with the start of the central directory.
      writeInt(out, 0);
      // Total number of entries in the central directory on this disk.
      writeLong(out, entryCount);
      // Total number of entries in the central directory.
      writeLong(out, entryCount);
      // Size of the central directory.
      writeLong(out, size);
      // Offset of start of central directory.
      writeLong(out, startOffset);

      // Zip64 end of central directory locator.
      writeInt(out, ZipConstants.ZIP64_LOCSIG);
      // number of the disk with the start of the zip64 end of central directory.
      writeInt(out, 0);
      // relative offset of the zip64 end of central directory record.
      writeLong(out, startOffset + size);
      // total number of disks.
      writeInt(out, 1);
    }

    // End of central directory record.
    writeInt(out, ZipEntry.ENDSIG);
    // Number of this disk (with end of central directory)
    writeShort(out, 0);
    // Number of disk on which central directory starts.
    writeShort(out, 0);
    // Number of central directory entries in this disk.
    writeShort(out, zip64MagicCount(entryCount));
    // Number of central directory entries.
    writeShort(out, zip64MagicCount(entryCount));
    // Size of the central directory in bytes.
    writeInt(out, zip64MagicVal(size));
    // Offset of the start of the central directory.
    writeInt(out, zip64MagicVal(startOffset));
    // Size of the comment (we don't have one)
    writeShort(out, 0);
  }

  /** Each entry requires a description of that entry to be contained in the central directory. */
  private long writeCentralDirectoryFileHeader(OutputStream out, EntryAccounting entry)
      throws IOException {
    int zip64DataLength = 0;
    if (isZip64MagicVal(entry.getSize())) {
      zip64DataLength += 8;
    }
    if (isZip64MagicVal(entry.getCompressedSize())) {
      zip64DataLength += 8;
    }
    if (isZip64MagicVal(entry.getOffset())) {
      zip64DataLength += 8;
    }

    int extraDataLength = 0;
    boolean useZip64 = zip64DataLength > 0;
    if (useZip64) {
      extraDataLength = zip64DataLength + 4;
    } else {
      extraDataLength = 0;
    }

    long size = 0;
    size += writeInt(out, ZipEntry.CENSIG);
    // version made by.
    size += writeShort(out, entry.getRequiredExtractVersion(useZip64));
    // version to extract with.
    size += writeShort(out, entry.getRequiredExtractVersion(useZip64));
    size += writeShort(out, entry.getFlags());
    // Compression.
    size += writeShort(out, entry.getCompressionMethod());
    // Modification time.
    size += writeInt(out, entry.getTime());
    size += writeInt(out, entry.getCrc());
    size += writeInt(out, zip64MagicVal(entry.getCompressedSize()));
    size += writeInt(out, zip64MagicVal(entry.getSize()));

    byte[] nameBytes = entry.getName().getBytes(StandardCharsets.UTF_8);
    long externalAttributes = entry.getExternalAttributes();
    // Length of name.
    size += writeShort(out, nameBytes.length);
    // Length of extra data.
    size += writeShort(out, extraDataLength);
    // Length of file comment.
    size += writeShort(out, 0);
    // Disk on which file starts.
    size += writeShort(out, 0);
    // internal file attributes (unknown)
    size += writeShort(out, 0);
    // external file attributes
    size += writeInt(out, externalAttributes);
    // Offset of local file header.
    size += writeInt(out, zip64MagicVal(entry.getOffset()));
    out.write(nameBytes);
    size += nameBytes.length;

    if (useZip64) {
      size += writeShort(out, ZipConstants.ZIP64_EXTID);
      size += writeShort(out, zip64DataLength);
      if (isZip64MagicVal(entry.getSize())) {
        size += writeLong(out, entry.getSize());
      }
      if (isZip64MagicVal(entry.getCompressedSize())) {
        size += writeLong(out, entry.getCompressedSize());
      }
      if (isZip64MagicVal(entry.getOffset())) {
        size += writeLong(out, entry.getOffset());
      }
    }
    return size;
  }

  static boolean isZip64MagicVal(long value) {
    return value >= ZipConstants.ZIP64_MAGICVAL;
  }

  static long zip64MagicVal(long value) {
    return Math.min(value, ZipConstants.ZIP64_MAGICVAL);
  }

  static boolean isZip64MagicCount(int value) {
    return value >= ZipConstants.ZIP64_MAGICCOUNT;
  }

  static int zip64MagicCount(int value) {
    return Math.min(value, ZipConstants.ZIP64_MAGICCOUNT);
  }
}
