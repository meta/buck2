/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.resources;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Optional;
import java.util.Random;
import java.util.regex.Pattern;
import java.util.zip.CRC32;
import java.util.zip.Deflater;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import java.util.zip.ZipOutputStream;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class MergeAssetsUtilsTest {

  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  // Extensions found in a codehub search for uses of extra_no_compress_asset_extensions
  private static ImmutableSet<String> extraExtensions =
      ImmutableSet.of("zip", "dict", "spo", "srt", "txt");

  // ==================== Tests for Simple Extensions (.jpg, .wav) ====================

  @Test
  public void testNormalExtensions() {

    // Combine with DEFAULT_NO_COMPRESS_EXTENSIONS
    ImmutableSet<String> allExtensions =
        new ImmutableSet.Builder<String>()
            .addAll(MergeAssetsUtils.DEFAULT_NO_COMPRESS_EXTENSIONS)
            .addAll(extraExtensions)
            .build();

    // Test each of DEFAULT_NO_COMPRESS_EXTENSIONS
    for (String ext : MergeAssetsUtils.DEFAULT_NO_COMPRESS_EXTENSIONS) {
      testExtension(ext, allExtensions);
    }

    // Test each of extraExtensions
    for (String ext : extraExtensions) {
      testExtension(ext, allExtensions);
    }
  }

  private void testExtension(String ext, ImmutableSet<String> allExtensions) {
    // Match in assets folder
    assertTrue(
        MergeAssetsUtils.isNoCompress("assets/file." + ext, allExtensions, Optional.empty()));
    // Match in nested folder
    assertTrue(
        MergeAssetsUtils.isNoCompress(
            "assets/foo/bar/file." + ext, allExtensions, Optional.empty()));
    // Match in nested folder with . in the directory path
    assertTrue(
        MergeAssetsUtils.isNoCompress(
            "assets/foo/v.20/file." + ext, allExtensions, Optional.empty()));
    // No match on folders with ext name
    assertFalse(
        MergeAssetsUtils.isNoCompress(
            "assets/" + ext + "/foo.bin", allExtensions, Optional.empty()));
    // Match files starting with a different extension
    assertTrue(
        MergeAssetsUtils.isNoCompress("assets/file.bin." + ext, allExtensions, Optional.empty()));
    // No match on files ending with a different extension
    assertFalse(
        MergeAssetsUtils.isNoCompress(
            "assets/file." + ext + ".bin", allExtensions, Optional.empty()));
    // No match on similar extensions that start with the extension
    assertFalse(
        MergeAssetsUtils.isNoCompress("assets/file." + ext + "s", allExtensions, Optional.empty()));
    // No match on similar extensions that end with the extension
    assertFalse(
        MergeAssetsUtils.isNoCompress("assets/file.x" + ext, allExtensions, Optional.empty()));
    // No match if file name and ext aren't at the end of the path
    assertFalse(
        MergeAssetsUtils.isNoCompress(
            "assets/file." + ext + "/foo", allExtensions, Optional.empty()));
    // No match if file name doesn't have a dot
    assertFalse(MergeAssetsUtils.isNoCompress("assets/" + ext, allExtensions, Optional.empty()));
    // Match is case-sensitive (.JPG shouldn't match for .jpg)
    String allcapExt = ext.toUpperCase();
    if (!allcapExt.equals(ext)) {
      assertFalse(
          MergeAssetsUtils.isNoCompress(
              "assets/file." + allcapExt, allExtensions, Optional.empty()));
    }
  }

  // ==================== Tests for Regex ====================

  @Test
  public void testRegex() {
    String regex =
        "worldtemplatemain\\.bank/bank$|cybersectormain\\.bank/bank$|fpstestworldfirepit\\.bank/bank$";
    Optional<Pattern> pattern = Optional.of(Pattern.compile(regex));

    assertTrue(
        MergeAssetsUtils.isNoCompress(
            "assets/meta/audio/worldtemplatemain.bank/bank", ImmutableSet.of(), pattern));
    assertFalse(
        MergeAssetsUtils.isNoCompress(
            "assets/meta/audio/worldtemplatemain.strings.bank/bank", ImmutableSet.of(), pattern));
    assertTrue(
        MergeAssetsUtils.isNoCompress(
            "assets/meta/cyber_sector/usd/assets/cybersectormain.bank/bank",
            ImmutableSet.of(),
            pattern));
    assertFalse(
        MergeAssetsUtils.isNoCompress(
            "assets/meta/cyber_sector/usd/assets/cybersectormain.strings.bank/bank",
            ImmutableSet.of(),
            pattern));
    assertTrue(
        MergeAssetsUtils.isNoCompress(
            "assets/meta/fps_test_world/audio/fpstestworldfirepit.bank/bank",
            ImmutableSet.of(),
            pattern));
    assertFalse(
        MergeAssetsUtils.isNoCompress(
            "assets/meta/fps_test_world/audio/fpstestworldmain.strings.bank/bank",
            ImmutableSet.of(),
            pattern));
  }

  // ==================== Tests for the compression level ====================

  @Test
  public void compressionLevelOnlyChangesTheCompressedBytes() throws IOException {
    Path root = tmp.getRoot().toPath();
    Path baseApk = root.resolve("base.ap_");
    try (ZipOutputStream out = new ZipOutputStream(Files.newOutputStream(baseApk))) {
      out.putNextEntry(new ZipEntry("AndroidManifest.xml"));
      out.write(words(2_000, 1));
      out.closeEntry();
      ZipEntry arsc = new ZipEntry("resources.arsc");
      byte[] arscBytes = words(100, 2);
      arsc.setMethod(ZipEntry.STORED);
      arsc.setSize(arscBytes.length);
      arsc.setCompressedSize(arscBytes.length);
      CRC32 crc = new CRC32();
      crc.update(arscBytes);
      arsc.setCrc(crc.getValue());
      out.putNextEntry(arsc);
      out.write(arscBytes);
      out.closeEntry();
    }
    Path assets = root.resolve("assets_dir");
    Files.createDirectories(assets.resolve("models"));
    Files.write(assets.resolve("models/model.glb"), words(50_000, 3));
    Files.write(assets.resolve("strings.json"), words(5_000, 4));
    Files.write(assets.resolve("image.png"), words(1_000, 5));

    Path fast = merge(root, baseApk, assets, "fast.ap_", Deflater.BEST_SPEED);
    Path best = merge(root, baseApk, assets, "best.ap_", Deflater.BEST_COMPRESSION);

    try (ZipFile fastZip = new ZipFile(fast.toFile());
        ZipFile bestZip = new ZipFile(best.toFile())) {
      List<ZipEntry> fastEntries = new ArrayList<>(Collections.list(fastZip.entries()));
      List<ZipEntry> bestEntries = new ArrayList<>(Collections.list(bestZip.entries()));
      assertEquals(names(bestEntries), names(fastEntries));
      assertEquals(
          List.of(
              "AndroidManifest.xml",
              "assets/image.png",
              "assets/models/model.glb",
              "assets/strings.json",
              "resources.arsc"),
          sorted(names(fastEntries)));
      for (int i = 0; i < fastEntries.size(); i++) {
        ZipEntry fastEntry = fastEntries.get(i);
        ZipEntry bestEntry = bestEntries.get(i);
        String name = fastEntry.getName();
        assertEquals(name, bestEntry.getMethod(), fastEntry.getMethod());
        assertEquals(name, bestEntry.getCrc(), fastEntry.getCrc());
        assertEquals(name, bestEntry.getSize(), fastEntry.getSize());
        assertArrayEquals(
            name,
            bestZip.getInputStream(bestEntry).readAllBytes(),
            fastZip.getInputStream(fastEntry).readAllBytes());
      }
      assertEquals(ZipEntry.STORED, fastZip.getEntry("resources.arsc").getMethod());
      assertEquals(ZipEntry.STORED, fastZip.getEntry("assets/image.png").getMethod());
      ZipEntry fastModel = fastZip.getEntry("assets/models/model.glb");
      assertEquals(ZipEntry.DEFLATED, fastModel.getMethod());
      assertNotEquals(
          bestZip.getEntry("assets/models/model.glb").getCompressedSize(),
          fastModel.getCompressedSize());
    }
  }

  @Test(expected = IllegalArgumentException.class)
  public void rejectsStoredAsTheCompressionLevel() throws IOException {
    Path root = tmp.getRoot().toPath();
    Path assets = root.resolve("assets_dir");
    Files.createDirectories(assets);
    MergeAssetsUtils.mergeAssets(
        root.resolve("out.ap_"),
        Optional.empty(),
        Optional.empty(),
        AbsPath.of(root),
        ImmutableMap.of("dex", ImmutableSet.of(RelPath.get("assets_dir"))),
        ImmutableSet.of(),
        Optional.empty(),
        MergeAssetsUtils.BinaryType.APK,
        Deflater.NO_COMPRESSION);
  }

  private static Path merge(Path root, Path baseApk, Path assets, String name, int level)
      throws IOException {
    Path output = root.resolve(name);
    MergeAssetsUtils.mergeAssets(
        output,
        Optional.empty(),
        Optional.of(baseApk),
        AbsPath.of(root),
        ImmutableMap.of("dex", ImmutableSet.of(RelPath.of(root.relativize(assets)))),
        ImmutableSet.of(),
        Optional.empty(),
        MergeAssetsUtils.BinaryType.APK,
        level);
    return output;
  }

  private static byte[] words(int count, long seed) {
    Random random = new Random(seed);
    String[] vocabulary = {"vertex", "normal", "uv", "index", "mesh", "node", "skin", "joint"};
    StringBuilder builder = new StringBuilder();
    for (int i = 0; i < count; i++) {
      builder.append(vocabulary[random.nextInt(vocabulary.length)]).append(random.nextInt(1000));
      builder.append(i % 16 == 15 ? '\n' : ' ');
    }
    return builder.toString().getBytes(StandardCharsets.UTF_8);
  }

  private static List<String> names(List<ZipEntry> entries) {
    List<String> names = new ArrayList<>();
    for (ZipEntry entry : entries) {
      names.add(entry.getName());
    }
    return names;
  }

  private static List<String> sorted(List<String> names) {
    List<String> copy = new ArrayList<>(names);
    Collections.sort(copy);
    return copy;
  }
}
