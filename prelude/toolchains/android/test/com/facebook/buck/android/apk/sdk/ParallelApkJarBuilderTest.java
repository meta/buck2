/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.apk.sdk;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Random;
import java.util.zip.CRC32;
import java.util.zip.ZipEntry;
import java.util.zip.ZipException;
import java.util.zip.ZipOutputStream;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class ParallelApkJarBuilderTest {
  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  private File resources;
  private File[] files;

  @Before
  public void makeInputs() throws Exception {
    Random random = new Random(42);
    resources = tmp.newFile("resources.zip");
    try (ZipOutputStream out = new ZipOutputStream(Files.newOutputStream(resources.toPath()))) {
      for (int i = 0; i < 40; i++) {
        byte[] data = i % 3 == 0 ? noise(random, 1000 * i) : text(random, 3000 * i);
        ZipEntry e = new ZipEntry("res/entry" + i);
        if (i % 4 == 0) {
          e.setMethod(ZipEntry.STORED);
          e.setSize(data.length);
          CRC32 crc = new CRC32();
          crc.update(data);
          e.setCrc(crc.getValue());
        }
        out.putNextEntry(e);
        out.write(data);
        out.closeEntry();
      }
      // Larger than the 300,000-byte buffering limit below, so these take the streaming path.
      byte[] big = text(random, 400_000);
      out.putNextEntry(new ZipEntry("res/big_deflated"));
      out.write(big);
      out.closeEntry();
      ZipEntry stored = new ZipEntry("res/big_stored");
      stored.setMethod(ZipEntry.STORED);
      stored.setSize(big.length);
      CRC32 crc = new CRC32();
      crc.update(big);
      stored.setCrc(crc.getValue());
      out.putNextEntry(stored);
      out.write(big);
      out.closeEntry();
    }
    int[] sizes = {0, 1, 777, 300_000, 3_000_000, 5_000_000};
    files = new File[sizes.length];
    for (int i = 0; i < sizes.length; i++) {
      files[i] = tmp.newFile("file" + i);
      Files.write(files[i].toPath(), i == 5 ? noise(random, sizes[i]) : text(random, sizes[i]));
    }
  }

  private static byte[] noise(Random random, int n) {
    byte[] b = new byte[n];
    random.nextBytes(b);
    return b;
  }

  private static byte[] text(Random random, int n) {
    String[] words = {"apk ", "dex ", "zip ", "deflate ", "resource ", "native ", "\n"};
    StringBuilder sb = new StringBuilder();
    while (sb.length() < n) {
      sb.append(words[random.nextInt(words.length)]);
    }
    return sb.substring(0, n).getBytes(StandardCharsets.UTF_8);
  }

  private byte[] build(int threads, long budget, int maxBufferedEntry) throws Exception {
    ByteArrayOutputStream out = new ByteArrayOutputStream();
    ApkJarBuilder builder = new ApkJarBuilder(out, false, threads, budget, maxBufferedEntry);
    try (FileInputStream in = new FileInputStream(resources)) {
      builder.writeZip(in, null);
    }
    for (int i = 0; i < files.length; i++) {
      builder.writeFile(files[i], "lib/x86_64/lib" + i + ".so");
      if (i == 2) {
        // A stored entry between deflated ones keeps its place.
        builder.writeStoredFile(files[3], "lib/arm64-v8a/libstored.so");
      }
    }
    builder.close();
    return out.toByteArray();
  }

  @Test
  public void parallelArchiveIsByteIdenticalToTheSerialOne() throws Exception {
    byte[] serial = build(0, Long.MAX_VALUE, Integer.MAX_VALUE);
    assertArrayEquals(serial, build(1, Long.MAX_VALUE, Integer.MAX_VALUE));
    assertArrayEquals(serial, build(4, Long.MAX_VALUE, Integer.MAX_VALUE));
    assertArrayEquals(serial, build(8, 1, Integer.MAX_VALUE));
  }

  @Test
  public void entriesTooLargeToBufferAreStreamedIdentically() throws Exception {
    byte[] serial = build(0, Long.MAX_VALUE, Integer.MAX_VALUE);
    // 300,000 bytes: the two largest files and the two big resources take the streaming path.
    assertArrayEquals(serial, build(4, Long.MAX_VALUE, 300_000));
    assertArrayEquals(serial, build(4, Long.MAX_VALUE, 299_999));
  }

  private byte[] buildEmptyLast(int threads, int maxBufferedEntry) throws Exception {
    ByteArrayOutputStream out = new ByteArrayOutputStream();
    ApkJarBuilder builder =
        new ApkJarBuilder(out, false, threads, Long.MAX_VALUE, maxBufferedEntry);
    builder.writeFile(files[3], "a");
    builder.writeFile(files[0], "empty_before_big");
    builder.writeFile(files[4], "big");
    builder.writeFile(files[0], "empty_last");
    builder.close();
    return out.toByteArray();
  }

  @Test
  public void emptyEntriesKeepTheirPlaceAndTheLastIsWritten() throws Exception {
    byte[] serial = buildEmptyLast(0, Integer.MAX_VALUE);
    assertArrayEquals(serial, buildEmptyLast(4, Integer.MAX_VALUE));
    // "big" is too large to buffer, so it is streamed after the queue is drained.
    assertArrayEquals(serial, buildEmptyLast(4, 300_000));
  }

  @Test
  public void archiveIsReadableAndKeepsStoredEntriesStored() throws Exception {
    File apk = tmp.newFile("out.apk");
    Files.write(apk.toPath(), build(4, Long.MAX_VALUE, Integer.MAX_VALUE));
    try (java.util.zip.ZipFile zip = new java.util.zip.ZipFile(apk)) {
      assertEquals(42 + files.length + 1, zip.size());
      assertEquals(ZipEntry.STORED, zip.getEntry("lib/arm64-v8a/libstored.so").getMethod());
      assertEquals(ZipEntry.STORED, zip.getEntry("res/entry0").getMethod());
      assertEquals(ZipEntry.DEFLATED, zip.getEntry("res/entry1").getMethod());
      assertArrayEquals(
          Files.readAllBytes(files[4].toPath()),
          zip.getInputStream(zip.getEntry("lib/x86_64/lib4.so")).readAllBytes());
    }
  }

  @Test
  public void aRepeatedNameFailsWhenItIsWritten() throws Exception {
    for (int threads : new int[] {0, 4}) {
      ApkJarBuilder builder =
          new ApkJarBuilder(
              new ByteArrayOutputStream(), false, threads, Long.MAX_VALUE, Integer.MAX_VALUE);
      builder.writeFile(files[2], "same");
      try {
        builder.writeFile(files[3], "same");
        fail("expected a duplicate entry error with " + threads + " threads");
      } catch (ZipException e) {
        assertTrue(e.getMessage(), e.getMessage().contains("duplicate entry: same"));
      } finally {
        builder.cleanUp();
      }
    }
  }
}
