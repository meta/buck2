/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.features.zip.rules.utils;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.util.relativepathmap.RelativePathMapUtils;
import com.facebook.buck.util.zip.collect.OnDuplicateEntry;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.regex.Pattern;
import org.jspecify.annotations.Nullable;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;
import org.kohsuke.args4j.spi.StringArrayOptionHandler;

/** Main entry point for generating Zip files */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ZipMain {

  private static final AbsPath CURRENT_DIRECTORY =
      AbsPath.of(Paths.get(".").normalize().toAbsolutePath());

  @Option(name = "--output_path", required = true)
  private String outputPath;

  @Option(name = "--on_duplicate_entry", required = true)
  private OnDuplicateEntry onDuplicateEntry;

  @Option(name = "--zip_sources", handler = StringArrayOptionHandler.class)
  private @Nullable List<String> zipSources;

  @Option(name = "--entries_to_exclude", handler = StringArrayOptionHandler.class)
  private @Nullable List<String> entriesToExclude;

  @Option(name = "--entries_file")
  private @Nullable File entriesFile;

  @Option(name = "--hardcode_permissions_for_deterministic_output")
  private boolean hardcodePermissionsForDeterministicOutput;

  public static void main(String[] args) throws IOException {
    ZipMain main = new ZipMain();
    CmdLineParser parser = new CmdLineParser(main);
    try {
      parser.parseArgument(args);
      main.run();
      System.exit(0);
    } catch (CmdLineException e) {
      System.err.println(e.getMessage());
      parser.printUsage(System.err);
      System.exit(1);
    }
  }

  /** Main entry point into ZipFile binary */
  private void run() throws IOException {
    RelPath outputPath = RelPath.get(this.outputPath);
    ImmutableList.Builder<RelPath> zipSourcesBuilder = ImmutableList.builder();
    if (zipSources != null) {
      for (String zipSource : zipSources) {
        zipSourcesBuilder.add(RelPath.get(zipSource));
      }
    }

    ImmutableSet.Builder<Pattern> entriesToExcludeBuilder = ImmutableSet.builder();
    if (entriesToExclude != null) {
      for (String entryToExclude : entriesToExclude) {
        entriesToExcludeBuilder.add(Pattern.compile(entryToExclude));
      }
    }

    Map<Path, AbsPath> map = new LinkedHashMap<>();
    if (entriesFile != null) {
      AbsPath entriesFilePath = CURRENT_DIRECTORY.resolve(entriesFile.toPath());
      List<String> entriesLines = Files.readAllLines(entriesFilePath.getPath());

      for (int lineIndex = 0; lineIndex < entriesLines.size(); lineIndex += 3) {
        String entry = entriesLines.get(lineIndex);
        AbsPath zipEntryAbsPath = CURRENT_DIRECTORY.resolve(entry);
        RelPath shortPath = RelPath.get(entriesLines.get(lineIndex + 1));
        boolean isSourceFlag = Boolean.parseBoolean(entriesLines.get(lineIndex + 2));

        if (isSourceFlag) {
          RelativePathMapUtils.addPathToRelativePathMap(
              CURRENT_DIRECTORY,
              map,
              CURRENT_DIRECTORY.getPath(),
              zipEntryAbsPath,
              shortPath.getPath());
        } else {
          RelativePathMapUtils.addPathToRelativePathMap(
              CURRENT_DIRECTORY,
              map,
              Objects.requireNonNull(zipEntryAbsPath.getPath().getParent()),
              zipEntryAbsPath,
              zipEntryAbsPath.getFileName());
        }
      }
    }
    ImmutableMap<RelPath, RelPath> entryMap =
        ZipUtils.toRelPathEntryMap(ImmutableMap.copyOf(map), CURRENT_DIRECTORY);

    ZipUtils.createZipFile(
        CURRENT_DIRECTORY,
        entryMap,
        zipSourcesBuilder.build(),
        entriesToExcludeBuilder.build(),
        onDuplicateEntry,
        outputPath,
        hardcodePermissionsForDeterministicOutput);

    System.exit(0);
  }
}
