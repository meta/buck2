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
import static org.junit.Assert.assertNotEquals;

import com.google.common.collect.ImmutableSet;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Random;
import java.util.function.Predicate;
import java.util.regex.Pattern;
import java.util.zip.CRC32;
import java.util.zip.Deflater;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import java.util.zip.ZipOutputStream;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class ApkBuilderUtilsTest {
  private static final String NATIVE_LIB = "lib/arm64-v8a/libfoo.so";
  private static final String OTHER_NATIVE_LIB = "lib/x86_64/libbar.so";
  private static final String ASSET = "assets/data/table.bin";

  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  private Path resourceApk;
  private Path dexFile;
  private Path nativeLibs;
  private Path assets;
  private byte[] nativeLibBytes;
  private byte[] otherNativeLibBytes;
  private byte[] assetBytes;

  @Before
  public void setUp() throws IOException {
    resourceApk = tmp.getRoot().toPath().resolve("resources.ap_");
    try (ZipOutputStream out = new ZipOutputStream(Files.newOutputStream(resourceApk))) {
      out.putNextEntry(new ZipEntry("AndroidManifest.xml"));
      out.write("<manifest/>".getBytes(StandardCharsets.UTF_8));
      out.closeEntry();
    }

    dexFile = write("classes.dex", repeated("dex\n", 3_000));
    nativeLibBytes = repeated("native code of libfoo\n", 20_000);
    otherNativeLibBytes = repeated("native code of libbar\n", 7_000);
    assetBytes = repeated("asset row\n", 5_000);

    nativeLibs = tmp.newFolder("libs").toPath();
    Files.createDirectories(nativeLibs.resolve("arm64-v8a"));
    Files.createDirectories(nativeLibs.resolve("x86_64"));
    Files.write(nativeLibs.resolve("arm64-v8a/libfoo.so"), nativeLibBytes);
    Files.write(nativeLibs.resolve("x86_64/libbar.so"), otherNativeLibBytes);

    assets = tmp.newFolder("assets_root").toPath();
    Files.createDirectories(assets.resolve("assets/data"));
    Files.write(assets.resolve(ASSET), assetBytes);
  }

  @Test
  public void storesMatchingFilesWithoutCompressingThem() throws Exception {
    Path apk = buildApk(path -> Pattern.compile("^lib/.*\\.so$").matcher(path).matches());

    try (ZipFile zip = new ZipFile(apk.toFile())) {
      assertStored(zip, NATIVE_LIB, nativeLibBytes);
      assertStored(zip, OTHER_NATIVE_LIB, otherNativeLibBytes);
      assertDeflated(zip, ASSET, assetBytes);
      assertDeflated(zip, "classes.dex", Files.readAllBytes(dexFile));
    }
  }

  @Test
  public void compressesEverythingWhenNothingMatches() throws Exception {
    Path apk = buildApk(path -> false);

    try (ZipFile zip = new ZipFile(apk.toFile())) {
      assertDeflated(zip, NATIVE_LIB, nativeLibBytes);
      assertDeflated(zip, OTHER_NATIVE_LIB, otherNativeLibBytes);
      assertDeflated(zip, ASSET, assetBytes);
    }
  }

  @Test
  public void storesTheDexFileWhenItMatches() throws Exception {
    Path apk = buildApk(path -> path.equals("classes.dex"));

    try (ZipFile zip = new ZipFile(apk.toFile())) {
      assertStored(zip, "classes.dex", Files.readAllBytes(dexFile));
      assertDeflated(zip, NATIVE_LIB, nativeLibBytes);
    }
  }

  @Test
  public void findsOnlyMatchingEntriesThatAreStillCompressed() throws Exception {
    Path apk = buildApk(path -> path.equals(NATIVE_LIB));

    ImmutableSet<String> compressed =
        ApkBuilderExecutableMain.findCompressedEntries(
            apk, ImmutableSet.of(Pattern.compile("^lib/.*\\.so$"), Pattern.compile("^assets/.*")));

    assertEquals(ImmutableSet.of(OTHER_NATIVE_LIB, ASSET), compressed);
  }

  @Test
  public void resourceApkCompressionLevelDoesNotChangeTheApk() throws Exception {
    byte[] model = vertices(30_000);
    byte[] arsc = repeated("resource table\n", 2_000);
    resourceApk = writeResourceApk("resources-fast.ap_", Deflater.BEST_SPEED, model, arsc);
    byte[] fromFastResources = Files.readAllBytes(buildApk(path -> false));
    resourceApk = writeResourceApk("resources-best.ap_", Deflater.BEST_COMPRESSION, model, arsc);
    byte[] fromBestResources = Files.readAllBytes(buildApk(path -> false));

    assertArrayEquals(fromBestResources, fromFastResources);
    try (ZipFile fast = new ZipFile(tmp.getRoot().toPath().resolve("resources-fast.ap_").toFile());
        ZipFile best = new ZipFile(tmp.getRoot().toPath().resolve("resources-best.ap_").toFile())) {
      assertDeflated(fast, "assets/model.glb", model);
      assertNotEquals(
          best.getEntry("assets/model.glb").getCompressedSize(),
          fast.getEntry("assets/model.glb").getCompressedSize());
    }
  }

  private Path writeResourceApk(String name, int level, byte[] model, byte[] arsc)
      throws IOException {
    Path path = tmp.getRoot().toPath().resolve(name);
    try (ZipOutputStream out = new ZipOutputStream(Files.newOutputStream(path))) {
      out.setLevel(level);
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
      out.putNextEntry(new ZipEntry("assets/model.glb"));
      out.write(model);
      out.closeEntry();
    }
    return path;
  }

  private Path buildApk(Predicate<String> storedPaths) throws Exception {
    Path apk = tmp.getRoot().toPath().resolve("out-" + System.nanoTime() + ".apk");
    ApkBuilderUtils.buildApk(
        resourceApk,
        apk,
        dexFile,
        ImmutableSet.of(assets),
        ImmutableSet.of(nativeLibs),
        ImmutableSet.of(),
        ImmutableSet.of(),
        tmp.getRoot().toPath().resolve("unused.keystore"),
        new KeystoreProperties(
            tmp.getRoot().toPath().resolve("unused.keystore"), "storepass", "keypass", "alias"),
        false,
        null,
        ImmutableSet.of(),
        storedPaths);
    return apk;
  }

  private Path write(String name, byte[] bytes) throws IOException {
    Path path = tmp.getRoot().toPath().resolve(name);
    try (OutputStream out = Files.newOutputStream(path)) {
      out.write(bytes);
    }
    return path;
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

  private static void assertStored(ZipFile zip, String name, byte[] expected) throws IOException {
    ZipEntry entry = zip.getEntry(name);
    assertEquals(name, ZipEntry.STORED, entry.getMethod());
    assertEquals(name, expected.length, entry.getCompressedSize());
    CRC32 crc = new CRC32();
    crc.update(expected);
    assertEquals(name, crc.getValue(), entry.getCrc());
    assertArrayEquals(name, expected, zip.getInputStream(entry).readAllBytes());
  }

  private static void assertDeflated(ZipFile zip, String name, byte[] expected) throws IOException {
    ZipEntry entry = zip.getEntry(name);
    assertEquals(name, ZipEntry.DEFLATED, entry.getMethod());
    assertArrayEquals(name, expected, zip.getInputStream(entry).readAllBytes());
  }
}
