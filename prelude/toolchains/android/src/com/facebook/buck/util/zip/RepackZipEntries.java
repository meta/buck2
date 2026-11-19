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
import com.google.common.collect.ImmutableSet;
import com.google.common.hash.Hashing;
import com.google.common.io.ByteStreams;
import java.io.BufferedInputStream;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;

/** Class for repacking ZIP archives. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class RepackZipEntries {
  /**
   * A command that creates a copy of a ZIP archive, making sure that certain user-specified entries
   * are packed with a certain compression level.
   *
   * <p>Can be used, for instance, to force the resources.arsc file in an Android .apk to be
   * compressed.
   */
  public static void repack(
      Path inputFile,
      Path outputFile,
      ImmutableSet<String> entries,
      ZipCompressionLevel compressionLevel)
      throws IOException {
    try (ZipInputStream in =
            new ZipInputStream(new BufferedInputStream(Files.newInputStream(inputFile)));
        CustomZipOutputStream out = ZipOutputStreams.newOutputStream(outputFile)) {
      for (ZipEntry entry = in.getNextEntry(); entry != null; entry = in.getNextEntry()) {
        CustomZipEntry customEntry = new CustomZipEntry(entry);
        if (entries.contains(customEntry.getName())) {
          customEntry.setCompressionLevel(compressionLevel.getValue());
        }

        // If we're using STORED files, we must pre-calculate the CRC.
        if (customEntry.getMethod() == ZipEntry.STORED) {
          byte[] bytes = ByteStreams.toByteArray(in);
          customEntry.setCrc(Hashing.crc32().hashBytes(bytes).padToLong());
          customEntry.setSize(bytes.length);
          customEntry.setCompressedSize(bytes.length);
          out.putNextEntry(customEntry);
          out.write(bytes);
          out.closeEntry();
        } else {
          DeflatedZipEntries.write(out, customEntry, in);
        }
      }
    }
  }
}
