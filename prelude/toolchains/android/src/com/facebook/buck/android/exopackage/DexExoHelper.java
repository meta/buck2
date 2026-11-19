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

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableMultimap;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Map;

/** Installs secondary dexes for exo. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class DexExoHelper implements ExoHelper {
  @VisibleForTesting public static final Path SECONDARY_DEX_DIR = Paths.get("secondary-dex");

  private final AbsPath rootPath;
  private final IsolatedExopackageInfo.IsolatedDexInfo dexInfo;

  private static enum DexCompressionType {
    RAW,
    JAR,
    UNKNOWN
  };

  DexExoHelper(AbsPath rootPath, IsolatedExopackageInfo.IsolatedDexInfo dexInfo) {
    this.rootPath = rootPath;
    this.dexInfo = dexInfo;
  }

  @Override
  public String getType() {
    return "secondary_dex";
  }

  @Override
  public ImmutableMap<Path, Path> getFilesToInstall() throws IOException {
    ImmutableMap<String, Path> dexFiles = getRequiredDexFiles();
    if (dexFiles.isEmpty()) {
      return ImmutableMap.of();
    }
    Path anyDexPath = dexFiles.entrySet().iterator().next().getValue();
    switch (getDexCompressionType(anyDexPath.toString())) {
      case JAR:
        {
          return ExopackageUtil.applyFilenameFormat(
              dexFiles, SECONDARY_DEX_DIR, "secondary-%s.dex.jar");
        }
      case RAW:
        {
          return ExopackageUtil.applyFilenameFormat(
              dexFiles, SECONDARY_DEX_DIR, "secondary-%s.dex");
        }
      case UNKNOWN:
      default:
        {
          throw new IllegalStateException("Unknown dex compression type");
        }
    }
  }

  @Override
  public ImmutableMap<Path, String> getMetadataToInstall() throws IOException {
    return ImmutableMap.of(
        SECONDARY_DEX_DIR.resolve("metadata.txt"), getSecondaryDexMetadataContents());
  }

  private DexCompressionType getDexCompressionType(String metadataPattern) {
    return metadataPattern.contains("dex.jar") ? DexCompressionType.JAR : DexCompressionType.RAW;
  }

  private String getSecondaryDexMetadataContents() throws IOException {
    // This is a bit gross.  It was a late addition.  Ideally, we could eliminate this, but
    // it wouldn't be terrible if we don't.  We store the dexed jars on the device
    // with the full SHA-1 hashes in their names.  This is the format that the loader uses
    // internally, so ideally we would just load them in place.  However, the code currently
    // expects to be able to copy the jars from a directory that matches the name in the
    // metadata file, like "secondary-1.dex.jar".  We don't want to give up putting the
    // hashes in the file names (because we use that to skip re-uploads), so just hack
    // the metadata file to have hash-like names.
    String originalMetadata =
        com.google.common.io.Files.toString(dexInfo.getMetadata().toFile(), StandardCharsets.UTF_8);
    switch (getDexCompressionType(originalMetadata)) {
      case JAR:
        {
          return com.google.common.io.Files.toString(
                  dexInfo.getMetadata().toFile(), StandardCharsets.UTF_8)
              .replaceAll(
                  "secondary-([\\d_]+)\\.dex\\.jar (\\p{XDigit}{40}) ", "secondary-$2.dex.jar $2 ");
        }
      case RAW:
        {
          return com.google.common.io.Files.toString(
                  dexInfo.getMetadata().toFile(), StandardCharsets.UTF_8)
              .replaceAll("secondary-([\\d_]+)\\.dex\\ (\\p{XDigit}{40}) ", "secondary-$2.dex $2 ");
        }
      case UNKNOWN:
      default:
        {
          throw new IllegalStateException("Unknown dex compression type");
        }
    }
  }

  private ImmutableMap<String, Path> getRequiredDexFiles() throws IOException {
    ImmutableMultimap<String, Path> multimap =
        ExopackageInstaller.parseExopackageInfoMetadata(
            dexInfo.getMetadata().getPath(), dexInfo.getDirectory().getPath(), rootPath);
    // Convert multimap to a map, because every key should have only one value.
    ImmutableMap.Builder<String, Path> builder = ImmutableMap.builder();
    for (Map.Entry<String, Path> entry : multimap.entries()) {
      builder.put(entry);
    }
    return builder.build();
  }
}
