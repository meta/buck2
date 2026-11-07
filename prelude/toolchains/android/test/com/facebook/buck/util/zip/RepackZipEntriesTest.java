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

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;

import com.google.common.collect.ImmutableSet;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Random;
import java.util.zip.CRC32;
import java.util.zip.Deflater;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import java.util.zip.ZipOutputStream;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class RepackZipEntriesTest {
  private static final int LOCAL_FILE_HEADER_SIGNATURE = 0x04034b50;
  private static final int CENTRAL_DIRECTORY_SIGNATURE = 0x02014b50;
  private static final int DATA_DESCRIPTOR_FLAG = 0x08;

  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  private final byte[] arsc = compressible(60_000, 1);
  private final byte[] dex = compressible(120_000, 2);
  private final byte[] library = random(4_099, 3);
  private final byte[] text = "Données sur l'écran".getBytes(StandardCharsets.UTF_8);

  @Test
  public void writesEveryEntryWithItsSizesInItsLocalHeader() throws Exception {
    Path output = repack(ImmutableSet.of("resources.arsc"), ZipCompressionLevel.MAX);

    assertEquals(5, localFileRecordsWithoutDataDescriptors(output));
    Map<String, Integer> methods = new LinkedHashMap<>();
    methods.put("resources.arsc", ZipEntry.DEFLATED);
    methods.put("classes.dex", ZipEntry.DEFLATED);
    methods.put("lib/arm64-v8a/libfoo.so", ZipEntry.STORED);
    methods.put("assets/données.txt", ZipEntry.DEFLATED);
    methods.put("assets/empty", ZipEntry.DEFLATED);
    assertEntries(output, methods);
    try (ZipFile zip = new ZipFile(output.toFile())) {
      assertEquals(
          deflatedLength(arsc, Deflater.BEST_COMPRESSION),
          zip.getEntry("resources.arsc").getCompressedSize());
    }
  }

  @Test
  public void storesTheEntriesRepackedWithoutCompression() throws Exception {
    Path output = repack(ImmutableSet.of("classes.dex"), ZipCompressionLevel.NONE);

    assertEquals(5, localFileRecordsWithoutDataDescriptors(output));
    Map<String, Integer> methods = new LinkedHashMap<>();
    methods.put("resources.arsc", ZipEntry.DEFLATED);
    methods.put("classes.dex", ZipEntry.STORED);
    methods.put("lib/arm64-v8a/libfoo.so", ZipEntry.STORED);
    methods.put("assets/données.txt", ZipEntry.DEFLATED);
    methods.put("assets/empty", ZipEntry.DEFLATED);
    assertEntries(output, methods);
  }

  /** Repacks an input written by java.util.zip, whose deflated entries have data descriptors. */
  private Path repack(ImmutableSet<String> entries, ZipCompressionLevel level) throws IOException {
    Path input = tmp.getRoot().toPath().resolve("input.apk");
    try (ZipOutputStream zip = new ZipOutputStream(Files.newOutputStream(input))) {
      putEntry(zip, "resources.arsc", ZipEntry.DEFLATED, arsc);
      putEntry(zip, "classes.dex", ZipEntry.DEFLATED, dex);
      putEntry(zip, "lib/arm64-v8a/libfoo.so", ZipEntry.STORED, library);
      putEntry(zip, "assets/données.txt", ZipEntry.DEFLATED, text);
      putEntry(zip, "assets/empty", ZipEntry.DEFLATED, new byte[0]);
    }
    Path output = tmp.getRoot().toPath().resolve("output.apk");
    RepackZipEntries.repack(input, output, entries, level);
    return output;
  }

  private void assertEntries(Path zip, Map<String, Integer> methods) throws IOException {
    Map<String, byte[]> contents = new LinkedHashMap<>();
    contents.put("resources.arsc", arsc);
    contents.put("classes.dex", dex);
    contents.put("lib/arm64-v8a/libfoo.so", library);
    contents.put("assets/données.txt", text);
    contents.put("assets/empty", new byte[0]);
    try (ZipFile file = new ZipFile(zip.toFile())) {
      List<String> names = new ArrayList<>();
      for (ZipEntry entry : Collections.list(file.entries())) {
        names.add(entry.getName());
        assertEquals(entry.getName(), (int) methods.get(entry.getName()), entry.getMethod());
        try (InputStream in = file.getInputStream(entry)) {
          assertArrayEquals(entry.getName(), contents.get(entry.getName()), in.readAllBytes());
        }
      }
      assertEquals(new ArrayList<>(methods.keySet()), names);
    }
  }

  /**
   * Walks the local file records from the start of the archive, each one directly after the data of
   * the previous one, and returns their number. A data descriptor after any entry's data stops the
   * walk short of the central directory.
   */
  private static int localFileRecordsWithoutDataDescriptors(Path zip) throws IOException {
    ByteBuffer buffer = ByteBuffer.wrap(Files.readAllBytes(zip)).order(ByteOrder.LITTLE_ENDIAN);
    int offset = 0;
    int records = 0;
    while (buffer.getInt(offset) == LOCAL_FILE_HEADER_SIGNATURE) {
      int flags = Short.toUnsignedInt(buffer.getShort(offset + 6));
      assertEquals("data descriptor flag of record " + records, 0, flags & DATA_DESCRIPTOR_FLAG);
      long compressedSize = Integer.toUnsignedLong(buffer.getInt(offset + 18));
      int nameLength = Short.toUnsignedInt(buffer.getShort(offset + 26));
      int extraLength = Short.toUnsignedInt(buffer.getShort(offset + 28));
      offset += Math.toIntExact(30 + nameLength + extraLength + compressedSize);
      records++;
    }
    assertEquals(
        "signature after local file record " + records,
        CENTRAL_DIRECTORY_SIGNATURE,
        buffer.getInt(offset));
    return records;
  }

  private static void putEntry(ZipOutputStream zip, String name, int method, byte[] content)
      throws IOException {
    ZipEntry entry = new ZipEntry(name);
    entry.setMethod(method);
    if (method == ZipEntry.STORED) {
      CRC32 crc = new CRC32();
      crc.update(content);
      entry.setSize(content.length);
      entry.setCompressedSize(content.length);
      entry.setCrc(crc.getValue());
    }
    zip.putNextEntry(entry);
    zip.write(content);
    zip.closeEntry();
  }

  private static long deflatedLength(byte[] content, int level) {
    Deflater deflater = new Deflater(level, true);
    try {
      deflater.setInput(content);
      deflater.finish();
      ByteArrayOutputStream out = new ByteArrayOutputStream();
      byte[] chunk = new byte[8192];
      while (!deflater.finished()) {
        out.write(chunk, 0, deflater.deflate(chunk));
      }
      return out.size();
    } finally {
      deflater.end();
    }
  }

  /** Text-like bytes: words from a small vocabulary, so that deflate shrinks them. */
  private static byte[] compressible(int length, long seed) {
    String[] words = {"layout ", "view ", "android ", "meta ", "helix ", "stage ", "\n"};
    Random random = new Random(seed);
    StringBuilder text = new StringBuilder(length);
    while (text.length() < length) {
      text.append(words[random.nextInt(words.length)]);
    }
    return text.substring(0, length).getBytes(StandardCharsets.UTF_8);
  }

  private static byte[] random(int length, long seed) {
    byte[] bytes = new byte[length];
    new Random(seed).nextBytes(bytes);
    return bytes;
  }
}
