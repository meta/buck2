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
import static com.facebook.buck.util.zip.CentralDirectory.isZip64MagicVal;
import static java.nio.charset.StandardCharsets.UTF_8;
import static java.util.zip.ZipEntry.CENCOM;
import static java.util.zip.ZipEntry.CENEXT;
import static java.util.zip.ZipEntry.CENHDR;
import static java.util.zip.ZipEntry.CENLEN;
import static java.util.zip.ZipEntry.CENNAM;
import static java.util.zip.ZipEntry.CENOFF;
import static java.util.zip.ZipEntry.CENSIG;
import static java.util.zip.ZipEntry.CENSIZ;

import com.facebook.infer.annotation.Nullsafe;
import java.util.Objects;
import org.jspecify.annotations.Nullable;

/**
 * Class that represents a Central Directory File Header in a ZIP archive. It provides methods to
 * read and parse the header data from a byte array. Reading Header Data: Parsing Header Fields: The
 * class parses the header fields, including the file name, compressed size, uncompressed size, and
 * offset. Providing Accessors: The class provides accessor methods for the parsed header fields,
 * allowing users to retrieve the values.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class CentralDirectoryFileHeader {

  private int headerOffset;
  private String name = "";
  private long compressedSize;
  private long uncompressedSize;
  private long offset;

  private int nameSize;
  private int extraSize;
  private int commSize;

  /**
   * Read a CENHDR header from a byte array in a given offset.
   *
   * @param buffer Byte array which contains the header.
   * @param headerOffset Offset of the location of the header in the buffer.
   * @return true if the buffer contains a header and it could be read, otherwise false.
   */
  boolean read(final byte[] buffer, int headerOffset) {

    final int headerLimit = headerOffset + CENHDR;

    if (headerLimit >= buffer.length || readInt(buffer, headerOffset) != CENSIG) {
      // no more CD Files
      return false;
    }

    this.headerOffset = headerOffset;
    this.compressedSize = readUnsignedInt(buffer, headerOffset + CENSIZ);
    this.uncompressedSize = readUnsignedInt(buffer, headerOffset + CENLEN);
    this.offset = readUnsignedInt(buffer, headerOffset + CENOFF);
    this.nameSize = readShort(buffer, headerOffset + CENNAM);
    this.extraSize = readShort(buffer, headerOffset + CENEXT);
    this.commSize = readShort(buffer, headerOffset + CENCOM);

    if (headerLimit + this.nameSize <= buffer.length) {
      this.name = new String(buffer, headerLimit, this.nameSize, UTF_8);
    }

    // zip64
    if (isZip64()) {

      int fieldsOffset = headerLimit + this.nameSize;
      int fieldsLimit = fieldsOffset + this.extraSize;

      while (fieldsOffset + 4 < fieldsLimit) {

        int extraFieldsTag = readShort(buffer, fieldsOffset);
        int extraFieldsSize = readShort(buffer, fieldsOffset + 2);

        fieldsOffset += 4;

        if (fieldsOffset + extraFieldsSize > fieldsLimit) {
          // invalid extra fields
          return false;
        }

        if (extraFieldsTag == ZipConstants.ZIP64_EXTID) {
          if (isZip64MagicVal(this.uncompressedSize)) {
            if (extraFieldsSize < 8 || fieldsOffset + 8 > fieldsLimit) {
              return false;
            }
            this.uncompressedSize = readLong(buffer, fieldsOffset);
            extraFieldsSize -= 8;
            fieldsOffset += 8;
          }
          if (isZip64MagicVal(this.compressedSize)) {
            if (extraFieldsSize < 8 || fieldsOffset + 8 > fieldsLimit) {
              return false;
            }
            this.compressedSize = readLong(buffer, fieldsOffset);
            extraFieldsSize -= 8;
            fieldsOffset += 8;
          }
          if (isZip64MagicVal(this.offset)) {
            if (extraFieldsSize < 8 || fieldsOffset + 8 > fieldsLimit) {
              return false;
            }
            this.offset = readLong(buffer, fieldsOffset);
          }
          break;
        } else {
          // skip to next extra field group
          fieldsOffset += extraFieldsSize;
        }
      }
    }
    return true;
  }

  boolean isZip64() {
    return isZip64MagicVal(compressedSize)
        || isZip64MagicVal(uncompressedSize)
        || isZip64MagicVal(offset);
  }

  int headerSize() {
    return CENHDR + nameSize + extraSize + commSize;
  }

  int nextOffset() {
    return headerOffset + headerSize();
  }

  /**
   * @return File name entry.
   */
  public String getName() {
    return name;
  }

  /**
   * @return Offset of which LOC + Data is located.
   */
  public long getOffset() {
    return offset;
  }

  /**
   * @return compressed size in bytes.
   */
  public long getCompressedSize() {
    return compressedSize;
  }

  /**
   * @return Uncompressed size in bytes.
   */
  public long getUncompressedSize() {
    return uncompressedSize;
  }

  @Override
  public boolean equals(@Nullable Object o) {
    if (this == o) {
      return true;
    }
    if (o == null || getClass() != o.getClass()) {
      return false;
    }
    final CentralDirectoryFileHeader that = (CentralDirectoryFileHeader) o;
    return headerOffset == that.headerOffset
        && compressedSize == that.compressedSize
        && uncompressedSize == that.uncompressedSize
        && offset == that.offset
        && nameSize == that.nameSize
        && extraSize == that.extraSize
        && commSize == that.commSize
        && Objects.equals(name, that.name);
  }

  @Override
  public int hashCode() {
    return Objects.hash(
        headerOffset,
        name,
        compressedSize,
        uncompressedSize,
        offset,
        nameSize,
        extraSize,
        commSize);
  }

  @Override
  public String toString() {
    return "CentralDirectoryFileHeader{"
        + "headerOffset="
        + headerOffset
        + ", name='"
        + name
        + '\''
        + ", compressedSize="
        + compressedSize
        + ", uncompressedSize="
        + uncompressedSize
        + ", offset="
        + offset
        + ", nameSize="
        + nameSize
        + ", extraSize="
        + extraSize
        + ", commSize="
        + commSize
        + '}';
  }
}
