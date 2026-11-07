/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.apk;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;

import com.facebook.buck.android.apk.sdk.ApkJarBuilder;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Random;
import java.util.zip.CRC32;
import java.util.zip.Deflater;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import java.util.zip.ZipOutputStream;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class ApkJarBuilderTest {
  private static final int LOCAL_FILE_HEADER_SIGNATURE = 0x04034b50;
  private static final int CENTRAL_DIRECTORY_SIGNATURE = 0x02014b50;
  private static final int DATA_DESCRIPTOR_FLAG = 0x08;

  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  private static final class Entry {
    final String name;
    final int method;
    final byte[] content;

    Entry(String name, int method, byte[] content) {
      this.name = name;
      this.method = method;
      this.content = content;
    }
  }

  @Test
  public void writesEveryEntryWithItsSizesInItsLocalHeader() throws Exception {
    byte[] dex = compressible(150_000, 1);
    byte[] library = random(5_001, 2);
    byte[] layout = compressible(9_000, 3);
    byte[] sound = random(777, 4);
    byte[] text = "Données sur l'écran".getBytes(StandardCharsets.UTF_8);
    byte[] asset = compressible(40_000, 5);

    // The input zip is written by java.util.zip, which gives its deflated entries data
    // descriptors.
    Path resources = tmp.getRoot().toPath().resolve("resources.apk");
    try (ZipOutputStream zip = new ZipOutputStream(Files.newOutputStream(resources))) {
      putEntry(zip, "res/layout/main.xml", ZipEntry.DEFLATED, layout);
      putEntry(zip, "res/raw/sound.ogg", ZipEntry.STORED, sound);
      putEntry(zip, "assets/dir/", ZipEntry.DEFLATED, new byte[0]);
      putEntry(zip, "assets/données.txt", ZipEntry.DEFLATED, text);
      putEntry(zip, "assets/empty", ZipEntry.DEFLATED, new byte[0]);
      putEntry(
          zip,
          "META-INF/MANIFEST.MF",
          ZipEntry.DEFLATED,
          "Manifest-Version: 1.0\n".getBytes(StandardCharsets.UTF_8));
    }

    Path apk = tmp.getRoot().toPath().resolve("out.apk");
    try (OutputStream out = Files.newOutputStream(apk)) {
      ApkJarBuilder builder = new ApkJarBuilder(out, false);
      builder.writeFile(write("classes.dex", dex), "classes.dex");
      builder.writeStoredFile(write("libfoo.so", library), "lib/arm64-v8a/libfoo.so");
      try (InputStream in = Files.newInputStream(resources)) {
        builder.writeZip(in, null);
      }
      builder.writeFile(write("table.bin", asset), "assets/table.bin");
      builder.close();
    }

    List<Entry> expected =
        List.of(
            new Entry("classes.dex", ZipEntry.DEFLATED, dex),
            new Entry("lib/arm64-v8a/libfoo.so", ZipEntry.STORED, library),
            new Entry("res/layout/main.xml", ZipEntry.DEFLATED, layout),
            new Entry("res/raw/sound.ogg", ZipEntry.STORED, sound),
            new Entry("assets/données.txt", ZipEntry.DEFLATED, text),
            new Entry("assets/empty", ZipEntry.DEFLATED, new byte[0]),
            new Entry("assets/table.bin", ZipEntry.DEFLATED, asset));
    assertEquals(expected.size(), localFileRecordsWithoutDataDescriptors(apk));
    List<Entry> actual = readEntries(apk);
    assertEquals(names(expected), names(actual));
    for (int i = 0; i < expected.size(); i++) {
      assertEquals(expected.get(i).name, expected.get(i).method, actual.get(i).method);
      assertArrayEquals(expected.get(i).name, expected.get(i).content, actual.get(i).content);
    }
  }

  @Test
  public void deflatesAtLevelFour() throws Exception {
    byte[] dex = compressible(200_000, 6);
    Path apk = tmp.getRoot().toPath().resolve("out.apk");
    try (OutputStream out = Files.newOutputStream(apk)) {
      ApkJarBuilder builder = new ApkJarBuilder(out, false);
      builder.writeFile(write("classes.dex", dex), "classes.dex");
      builder.close();
    }

    try (ZipFile zip = new ZipFile(apk.toFile())) {
      ZipEntry entry = zip.getEntry("classes.dex");
      assertEquals(deflatedLength(dex, 4), entry.getCompressedSize());
      CRC32 crc = new CRC32();
      crc.update(dex);
      assertEquals(crc.getValue(), entry.getCrc());
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

  private static List<Entry> readEntries(Path zip) throws IOException {
    List<Entry> entries = new ArrayList<>();
    try (ZipFile file = new ZipFile(zip.toFile())) {
      for (ZipEntry entry : Collections.list(file.entries())) {
        try (InputStream in = file.getInputStream(entry)) {
          entries.add(new Entry(entry.getName(), entry.getMethod(), in.readAllBytes()));
        }
      }
    }
    return entries;
  }

  private static List<String> names(List<Entry> entries) {
    List<String> names = new ArrayList<>();
    for (Entry entry : entries) {
      names.add(entry.name);
    }
    return names;
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

  private File write(String name, byte[] content) throws IOException {
    Path path = tmp.getRoot().toPath().resolve(name);
    Files.write(path, content);
    return path.toFile();
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
