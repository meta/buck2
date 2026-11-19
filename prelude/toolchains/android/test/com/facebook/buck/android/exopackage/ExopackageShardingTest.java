/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.exopackage;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.core.filesystems.AbsPath;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSortedMap;
import com.google.common.collect.ImmutableSortedSet;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class ExopackageShardingTest {

  private static final Path DATA_ROOT = Paths.get("/data/local/tmp/exopackage/com.example");

  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  private AbsPath root() throws Exception {
    return AbsPath.of(tmp.getRoot().toPath().toRealPath());
  }

  /** A file of exactly {@code bytes}, named by its path relative to the project root. */
  private Path sized(String name, int bytes) throws Exception {
    Files.write(tmp.getRoot().toPath().resolve(name), new byte[bytes]);
    return Paths.get(name);
  }

  private ImmutableList<ExopackageInstaller.PushShard> shard(long targetBytes, String... names)
      throws Exception {
    ImmutableSortedMap.Builder<Path, Path> files = ImmutableSortedMap.naturalOrder();
    for (String name : names) {
      files.put(Paths.get(name), Paths.get(name));
    }
    return ExopackageInstaller.splitIntoShards(
        "native_library", files.build(), root(), DATA_ROOT, targetBytes);
  }

  /** The names each shard carries, in order, for readable assertions. */
  private static List<List<String>> contents(ImmutableList<ExopackageInstaller.PushShard> shards) {
    List<List<String>> byShard = new ArrayList<>();
    for (ExopackageInstaller.PushShard shard : shards) {
      List<String> names = new ArrayList<>();
      shard.installPaths.keySet().forEach(path -> names.add(path.getFileName().toString()));
      byShard.add(names);
    }
    return byShard;
  }

  @Test
  public void aPayloadUnderTheTargetStaysOneShard() throws Exception {
    sized("a.so", 10);
    sized("b.so", 10);

    assertEquals(List.of(List.of("a.so", "b.so")), contents(shard(1_000L, "a.so", "b.so")));
  }

  /** The file that crosses the target closes its own shard rather than opening the next one. */
  @Test
  public void theFileThatCrossesTheTargetEndsThatShard() throws Exception {
    sized("a.so", 60);
    sized("b.so", 60);
    sized("c.so", 10);

    assertEquals(
        List.of(List.of("a.so", "b.so"), List.of("c.so")),
        contents(shard(100L, "a.so", "b.so", "c.so")));
  }

  /** Whatever is left when the files run out still ships. */
  @Test
  public void theTrailingFilesFormAShardOfTheirOwn() throws Exception {
    sized("a.so", 200);
    sized("b.so", 5);

    assertEquals(List.of(List.of("a.so"), List.of("b.so")), contents(shard(100L, "a.so", "b.so")));
  }

  /** A shard is never smaller than one file, so an oversized file is not split. */
  @Test
  public void aFileLargerThanTheTargetIsNotSplit() throws Exception {
    sized("huge.so", 500);

    assertEquals(List.of(List.of("huge.so")), contents(shard(100L, "huge.so")));
  }

  /** The lock file belongs to no payload, so reclaiming must never take it. */
  @Test
  public void theLockFileIsNeverUnwanted() {
    ImmutableSortedSet<Path> present =
        ImmutableSortedSet.of(Paths.get("lock"), Paths.get("resources/old.apk"));

    assertEquals(
        ImmutableSortedSet.of(Paths.get("resources/old.apk")),
        ExopackageInstaller.filesToDelete(present, ImmutableList.of()));
  }

  @Test
  public void whatAPayloadWantsIsNotUnwanted() throws Exception {
    ImmutableSortedSet<Path> present =
        ImmutableSortedSet.of(Paths.get("resources/keep.apk"), Paths.get("resources/drop.apk"));

    assertEquals(
        ImmutableSortedSet.of(Paths.get("resources/drop.apk")),
        ExopackageInstaller.filesToDelete(
            present,
            ImmutableList.of(
                payload(
                    ImmutableMap.of(Paths.get("resources/keep.apk"), Paths.get("src/keep.apk")),
                    ImmutableMap.of()))));
  }

  @Test
  public void anEmptyPayloadProducesNoShards() throws Exception {
    assertEquals(List.of(), contents(shard(100L)));
  }

  /** Destinations are resolved against the device data root, sources against the project root. */
  @Test
  public void pathsAreResolvedAgainstTheirOwnRoots() throws Exception {
    sized("a.so", 10);

    ExopackageInstaller.PushShard only = shard(1_000L, "a.so").get(0);

    assertEquals("native_library", only.filesType);
    assertEquals(DATA_ROOT.resolve("a.so"), only.installPaths.keySet().iterator().next());
    assertEquals(root().resolve("a.so").getPath(), only.installPaths.values().iterator().next());
  }

  /** A payload with the given content and metadata, for the delete-set arithmetic below. */
  private static ExopackageInstaller.ResolvedExoPayload payload(
      ImmutableMap<Path, Path> filesToInstall, ImmutableMap<Path, String> metadataToInstall)
      throws IOException {
    return new ExopackageInstaller.ResolvedExoPayload(
        new ExoHelper() {
          @Override
          public ImmutableMap<Path, Path> getFilesToInstall() {
            return filesToInstall;
          }

          @Override
          public ImmutableMap<Path, String> getMetadataToInstall() {
            return metadataToInstall;
          }

          @Override
          public String getType() {
            return "resources";
          }
        });
  }

  /** Only what the device is missing is sent; what it already holds is left alone. */
  @Test
  public void onlyFilesTheDeviceDoesNotHaveArePushed() {
    ImmutableSortedSet<Path> present = ImmutableSortedSet.of(Paths.get("resources/have.apk"));

    assertEquals(
        ImmutableSortedMap.of(Paths.get("resources/want.apk"), Paths.get("src/want.apk")),
        ExopackageInstaller.filesToPush(
            present,
            ImmutableMap.of(
                Paths.get("resources/have.apk"), Paths.get("src/have.apk"),
                Paths.get("resources/want.apk"), Paths.get("src/want.apk"))));
  }

  /** A payload the device already has in full sends nothing. */
  @Test
  public void aPayloadAlreadyOnTheDevicePushesNothing() {
    ImmutableSortedSet<Path> present = ImmutableSortedSet.of(Paths.get("resources/have.apk"));

    assertTrue(
        ExopackageInstaller.filesToPush(
                present,
                ImmutableMap.of(Paths.get("resources/have.apk"), Paths.get("src/have.apk")))
            .isEmpty());
  }
}
