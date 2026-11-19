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

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.util.json.ObjectMappers;
import com.facebook.infer.annotation.Nullsafe;
import com.fasterxml.jackson.core.type.TypeReference;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import com.google.common.hash.Hashing;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.zip.Deflater;
import org.jetbrains.annotations.Nullable;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;
import org.kohsuke.args4j.spi.StringArrayOptionHandler;

/** Main entry point for merging assets. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class MergeAssetsExecutableMain {

  @Option(name = "--output-apk", required = true, usage = "path to output APK")
  private String outputApk;

  @Option(name = "--module-assets-apks-dir", usage = "path to module assets APKs")
  private @Nullable String moduleAssetsApksDir = null;

  @Option(name = "--base-apk", usage = "path to existing APK containing resources")
  private @Nullable String baseApk = null;

  @Option(name = "--assets-dirs", required = true, usage = "directory containing assets")
  private String assetsDirs;

  @Option(
      name = "--output-apk-hash",
      usage = "output path of a file containing the hash of output APK")
  private @Nullable String outputApkHash = null;

  @Option(
      name = "--extra-no-compress-asset-extensions",
      handler = StringArrayOptionHandler.class,
      usage = "list of asset extensions that should not be compressed")
  private List<String> extraNoCompressAssetExtensions = new ArrayList<>();

  @Option(
      name = "--extra_no_compress_asset_regex",
      usage = "regex pattern to match asset paths that should not be compressed")
  private @Nullable String extraNoCompressRegex = null;

  @Option(name = "--binary-type", usage = "either 'apk' or 'aab'")
  private @Nullable String binaryType = null;

  @Option(
      name = "--compression-level",
      usage = "deflate level, from 1 to 9, of the entries that are compressed (default: 9)")
  private int compressionLevel = Deflater.BEST_COMPRESSION;

  public static void main(String[] args) throws IOException {
    MergeAssetsExecutableMain main = new MergeAssetsExecutableMain();
    CmdLineParser parser = new CmdLineParser(main);
    try {
      parser.parseArgument(args);
      main.run();
      System.exit(0);
    } catch (CmdLineException e) {
      System.err.println(e.toString());
      parser.printUsage(System.err);
      System.exit(1);
    }
  }

  private void run() throws IOException {
    // readValue only yields null for a literal JSON `null` document.
    ImmutableMap<String, ImmutableSet<Path>> rawDirs =
        Objects.requireNonNull(
            ObjectMappers.READER.readValue(
                ObjectMappers.createParser(Paths.get(assetsDirs)),
                new TypeReference<ImmutableMap<String, ImmutableSet<Path>>>() {}));

    ImmutableMap<String, ImmutableSet<RelPath>> dirs =
        rawDirs.entrySet().stream()
            .collect(
                ImmutableMap.toImmutableMap(
                    Map.Entry::getKey,
                    x ->
                        x.getValue().stream()
                            .map(RelPath::of)
                            .collect(ImmutableSet.toImmutableSet())));

    Path outputApkPath = Paths.get(outputApk);
    MergeAssetsUtils.mergeAssets(
        outputApkPath,
        Optional.ofNullable(moduleAssetsApksDir).map(Paths::get),
        Optional.ofNullable(baseApk).map(Paths::get),
        AbsPath.of(Paths.get(".").normalize().toAbsolutePath()),
        dirs,
        ImmutableSet.copyOf(extraNoCompressAssetExtensions),
        Optional.ofNullable(extraNoCompressRegex),
        MergeAssetsUtils.BinaryType.valueOf(Objects.requireNonNull(binaryType).toUpperCase()),
        compressionLevel);

    if (outputApkHash != null) {
      Files.writeString(
          Paths.get(outputApkHash),
          com.google.common.io.Files.hash(outputApkPath.toFile(), Hashing.sha1()).toString());
    }
  }
}
