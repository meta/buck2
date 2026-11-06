/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.bundle;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;

import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
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

public class AndroidBundleUtilsTest {

  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  // The bundle builder inflates the merged assets APK and deflates it again, so the deflate level
  // of that APK must not reach the module. A module builder that copied compressed entries as they
  // are would fail these tests.
  @Test
  public void baseModuleDoesNotDependOnTheResourceApkCompressionLevel() throws Exception {
    byte[] model = vertices(30_000);
    byte[] arsc = repeated("resource table\n", 2_000);
    Path fast = writeApk("resources-fast.ap_", Deflater.BEST_SPEED, model, arsc);
    Path best = writeApk("resources-best.ap_", Deflater.BEST_COMPRESSION, model, arsc);
    assertNotEquals(
        compressedSize(best, "assets/model.glb"), compressedSize(fast, "assets/model.glb"));

    assertSameEntries(buildModule(best, /* isBaseModule */ true), buildModule(fast, true));
  }

  @Test
  public void featureModuleDoesNotDependOnTheAssetsApkCompressionLevel() throws Exception {
    byte[] model = vertices(30_000);
    Path fast = writeApk("assets-fast.ap_", Deflater.BEST_SPEED, model, null);
    Path best = writeApk("assets-best.ap_", Deflater.BEST_COMPRESSION, model, null);
    assertNotEquals(
        compressedSize(best, "assets/model.glb"), compressedSize(fast, "assets/model.glb"));

    assertSameEntries(buildModule(best, /* isBaseModule */ false), buildModule(fast, false));
  }

  private Path buildModule(Path apk, boolean isBaseModule) throws Exception {
    Path module = tmp.getRoot().toPath().resolve("module-" + System.nanoTime() + ".zip");
    File fakeResourceApk = tmp.newFile();
    AndroidBundleUtils.addModule(
        new ThrowingDuplicateFileListener(),
        module.toFile(),
        isBaseModule ? fakeResourceApk : apk.toFile(),
        /* packageMetaInfVersionFiles */ false,
        /* verboseStream */ null,
        isBaseModule,
        isBaseModule ? apk : null,
        ImmutableSet.of(),
        ImmutableMap.of(),
        ImmutableSet.of(),
        ImmutableSet.of(),
        ImmutableSet.of(),
        new HashSet<>(),
        new HashSet<>(),
        ImmutableSet.of());
    return module;
  }

  private Path writeApk(String name, int level, byte[] model, byte[] arsc) throws IOException {
    Path path = tmp.getRoot().toPath().resolve(name);
    try (ZipOutputStream out = new ZipOutputStream(Files.newOutputStream(path))) {
      out.setLevel(level);
      if (arsc != null) {
        out.putNextEntry(new ZipEntry("AndroidManifest.xml"));
        out.write("<manifest/>".getBytes(StandardCharsets.UTF_8));
        out.closeEntry();
        ZipEntry arscEntry = new ZipEntry("resources.arsc");
        arscEntry.setMethod(ZipEntry.STORED);
        arscEntry.setSize(arsc.length);
        arscEntry.setCompressedSize(arsc.length);
        CRC32 crc = new CRC32();
        crc.update(arsc);
        arscEntry.setCrc(crc.getValue());
        out.putNextEntry(arscEntry);
        out.write(arsc);
        out.closeEntry();
      }
      out.putNextEntry(new ZipEntry("assets/model.glb"));
      out.write(model);
      out.closeEntry();
    }
    return path;
  }

  private static void assertSameEntries(Path expected, Path actual) throws IOException {
    try (ZipFile expectedZip = new ZipFile(expected.toFile());
        ZipFile actualZip = new ZipFile(actual.toFile())) {
      List<? extends ZipEntry> expectedEntries = Collections.list(expectedZip.entries());
      List<? extends ZipEntry> actualEntries = Collections.list(actualZip.entries());
      assertEquals(names(expectedEntries), names(actualEntries));
      for (int i = 0; i < expectedEntries.size(); i++) {
        ZipEntry expectedEntry = expectedEntries.get(i);
        ZipEntry actualEntry = actualEntries.get(i);
        String name = expectedEntry.getName();
        assertEquals(name, expectedEntry.getMethod(), actualEntry.getMethod());
        assertEquals(name, expectedEntry.getCrc(), actualEntry.getCrc());
        assertEquals(name, expectedEntry.getSize(), actualEntry.getSize());
        assertEquals(name, expectedEntry.getCompressedSize(), actualEntry.getCompressedSize());
        assertArrayEquals(
            name,
            expectedZip.getInputStream(expectedEntry).readAllBytes(),
            actualZip.getInputStream(actualEntry).readAllBytes());
      }
    }
  }

  private static long compressedSize(Path zip, String entry) throws IOException {
    try (ZipFile zipFile = new ZipFile(zip.toFile())) {
      return zipFile.getEntry(entry).getCompressedSize();
    }
  }

  private static List<String> names(List<? extends ZipEntry> entries) {
    List<String> names = new ArrayList<>();
    for (ZipEntry entry : entries) {
      names.add(entry.getName());
    }
    return names;
  }

  private static byte[] vertices(int count) {
    Random random = new Random(42);
    StringBuilder builder = new StringBuilder();
    for (int i = 0; i < count; i++) {
      builder.append("v ").append(random.nextInt(100)).append(' ').append(random.nextInt(100));
      builder.append('\n');
    }
    return builder.toString().getBytes(StandardCharsets.UTF_8);
  }

  private static byte[] repeated(String line, int times) {
    return line.repeat(times).getBytes(StandardCharsets.UTF_8);
  }

  private static class ThrowingDuplicateFileListener
      implements AndroidBundleUtils.DuplicateFileListener {

    @Override
    public void onDuplicateFileAdded(String file) {
      throw new AssertionError("Duplicate file " + file);
    }

    @Override
    public void onDuplicateSourceFileAdded(Path sourceFile) {
      throw new AssertionError("Duplicate source file " + sourceFile);
    }
  }
}
