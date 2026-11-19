/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.aar;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.util.relativepathmap.RelativePathMapUtils;
import com.facebook.buck.util.zip.collect.OnDuplicateEntry;
import com.facebook.buck.util.zip.collect.ZipEntrySourceCollectionBuilder;
import com.facebook.buck.util.zip.collect.ZipEntrySourceCollectionWriter;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableSet;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Objects;
import org.jetbrains.annotations.Nullable;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class AarBuilderExecutableMain {
  private static final AbsPath CURRENT_DIRECTORY =
      AbsPath.of(Paths.get(".").normalize().toAbsolutePath());

  @Option(name = "--output_path", required = true)
  private Path outputPath;

  @Option(name = "--on_duplicate_entry", required = true)
  private OnDuplicateEntry onDuplicateEntry;

  @Option(name = "--entries_file", required = true)
  private Path entriesFile;

  @Option(name = "--hardcode_permissions_for_deterministic_output")
  private boolean hardcodePermissionsForDeterministicOutput;

  @Option(name = "--native_libs_file", required = true)
  private Path nativeLibsFile;

  @Option(name = "--native_libs_assets_file", required = true)
  private Path nativeLibsAssetsFile;

  @Option(name = "--proguard_config_file", required = false)
  @Nullable
  private Path proguardConfigFile;

  public static void main(String[] args) throws IOException {
    AarBuilderExecutableMain main = new AarBuilderExecutableMain();
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
    ZipEntrySourceCollectionBuilder builder =
        new ZipEntrySourceCollectionBuilder(ImmutableSet.of(), onDuplicateEntry);

    Map<Path, AbsPath> entriesMap = new LinkedHashMap<>();
    for (String entry : Files.readAllLines(entriesFile)) {
      AbsPath zipEntryAbsPath = CURRENT_DIRECTORY.resolve(entry);
      RelativePathMapUtils.addPathToRelativePathMap(
          CURRENT_DIRECTORY,
          entriesMap,
          Objects.requireNonNull(zipEntryAbsPath.getPath().getParent()),
          zipEntryAbsPath,
          zipEntryAbsPath.getFileName());
    }

    for (Map.Entry<Path, AbsPath> pathEntry : entriesMap.entrySet()) {
      String entryName = pathEntry.getKey().toString();
      builder.addFile(entryName, pathEntry.getValue().getPath());
    }

    // We resolve native libs relative to themselves, and then put them into the
    // "jni" folder.
    Map<Path, AbsPath> nativeLibsMap = new LinkedHashMap<>();
    for (String nativeLibsEntry : Files.readAllLines(nativeLibsFile)) {
      AbsPath zipEntryAbsPath = CURRENT_DIRECTORY.resolve(nativeLibsEntry);
      RelativePathMapUtils.addPathToRelativePathMap(
          CURRENT_DIRECTORY,
          nativeLibsMap,
          zipEntryAbsPath.getPath(),
          zipEntryAbsPath,
          zipEntryAbsPath.getFileName());
    }

    Path jniPath = Paths.get("jni");
    for (Map.Entry<Path, AbsPath> pathEntry : nativeLibsMap.entrySet()) {
      String entryName = jniPath.resolve(pathEntry.getKey()).toString();
      builder.addFile(entryName, pathEntry.getValue().getPath());
    }

    // Native lib assets are resolved relative to themselves, and we throw away
    // the metadata.txt files.
    Map<Path, AbsPath> nativeLibsAssetsEntriesMap = new LinkedHashMap<>();
    for (String entry : Files.readAllLines(nativeLibsAssetsFile)) {
      AbsPath zipEntryAbsPath = CURRENT_DIRECTORY.resolve(entry);
      RelativePathMapUtils.addPathToRelativePathMap(
          CURRENT_DIRECTORY,
          nativeLibsAssetsEntriesMap,
          zipEntryAbsPath.getPath(),
          zipEntryAbsPath,
          zipEntryAbsPath.getFileName());
    }

    for (Map.Entry<Path, AbsPath> pathEntry : nativeLibsAssetsEntriesMap.entrySet()) {
      if (Objects.requireNonNull(pathEntry.getKey().getFileName())
          .toString()
          .equals("metadata.txt")) {
        continue;
      }

      String entryName = pathEntry.getKey().toString();
      builder.addFile(entryName, pathEntry.getValue().getPath());
    }

    if (proguardConfigFile != null) {
      builder.addFile("proguard.txt", proguardConfigFile);
    }

    new ZipEntrySourceCollectionWriter(CURRENT_DIRECTORY, hardcodePermissionsForDeterministicOutput)
        .copyToZip(builder.build(), outputPath);
  }
}
