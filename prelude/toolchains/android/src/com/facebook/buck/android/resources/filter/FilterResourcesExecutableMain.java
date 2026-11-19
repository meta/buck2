/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.resources.filter;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.io.file.GlobPatternMatcher;
import com.facebook.buck.io.file.PathMatcher;
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.buck.util.json.ObjectMappers;
import com.facebook.infer.annotation.Nullsafe;
import com.fasterxml.jackson.core.type.TypeReference;
import com.google.common.base.Preconditions;
import com.google.common.collect.ImmutableBiMap;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Arrays;
import java.util.Map;
import java.util.Objects;
import java.util.function.Predicate;
import java.util.stream.Collectors;
import org.jetbrains.annotations.Nullable;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;

/** Entry point for filtering resources. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class FilterResourcesExecutableMain {
  private static final ImmutableList<PathMatcher> NON_ASSET_FILENAMES_MATCHERS =
      ImmutableList.of(GlobPatternMatcher.of("**/.DS_Store"));

  @Option(name = "--in-res-dir-to-out-res-dir-map", required = true)
  private String inResDirToOutResDirMapPath;

  @Option(name = "--voltron-in-res-dir-to-out-res-dir-map")
  @Nullable
  private String voltronInResDirToOutResDirMapPath = null;

  @Option(name = "--target-densities")
  @Nullable
  private String targetDensities = null;

  @Option(name = "--enable-string-as-assets-filtering")
  private boolean enableStringsAsAssetsFiltering;

  @Option(name = "--not-filtered-string-dirs")
  @Nullable
  private String notFilteredStringDirsFile = null;

  @Option(name = "--string-files-list-output")
  @Nullable
  private String stringFilesListOutput = null;

  @Option(name = "--locales")
  @Nullable
  private String localesString = null;

  @Option(name = "--packaged-locales")
  @Nullable
  private String packagedLocalesString = null;

  @Option(name = "--post-filter-resources-cmd")
  @Nullable
  private String postFilterResourcesCmd = null;

  @Option(name = "--post-filter-resources-cmd-override-symbols-output")
  @Nullable
  private String postFilterResourcesCmdOverrideSymbols = null;

  @Option(name = "--allowlisted-locales")
  @Nullable
  private String allowlistedLocalesFile = null;

  public static void main(String[] args) throws IOException {
    FilterResourcesExecutableMain main = new FilterResourcesExecutableMain();
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
    AbsPath root = AbsPath.of(Paths.get(".").normalize().toAbsolutePath());
    // readValue only yields null for a literal JSON `null` document.
    Map<String, ImmutableBiMap<Path, Path>> rawMap =
        Objects.requireNonNull(
            ObjectMappers.READER.readValue(
                ObjectMappers.createParser(Paths.get(inResDirToOutResDirMapPath)),
                new TypeReference<Map<String, ImmutableBiMap<Path, Path>>>() {}));
    ImmutableBiMap<Path, Path> inResDirToOutResDirMap =
        Objects.requireNonNull(rawMap.get("res_dir_map"));
    ImmutableSet<ResourceFilters.Density> targetDensitiesSet =
        targetDensities != null
            ? Arrays.stream(targetDensities.split(","))
                .map(ResourceFilters.Density::from)
                .collect(ImmutableSet.toImmutableSet())
            : ImmutableSet.of();
    ImmutableSet<String> locales =
        localesString != null ? ImmutableSet.copyOf(localesString.split(",")) : ImmutableSet.of();
    ImmutableSet<String> packagedLocales =
        packagedLocalesString != null
            ? ImmutableSet.copyOf(packagedLocalesString.split(","))
            : ImmutableSet.of();
    ImmutableSet<Path> notFilteredStringDirs =
        notFilteredStringDirsFile != null
            ? Files.readAllLines(Paths.get(notFilteredStringDirsFile)).stream()
                .map(Paths::get)
                .collect(ImmutableSet.toImmutableSet())
            : ImmutableSet.of();

    ImmutableMap<Path, ImmutableSet<String>> allowlistedLocalesDirs =
        allowlistedLocalesFile != null
            ? Objects.requireNonNull(
                ObjectMappers.READER.readValue(
                    ObjectMappers.createParser(Paths.get(allowlistedLocalesFile)),
                    new TypeReference<ImmutableMap<Path, ImmutableSet<String>>>() {}))
            : ImmutableMap.of();

    Predicate<Path> filteringPredicate =
        FilteringPredicate.getFilteringPredicate(
            root,
            ProjectFilesystemUtils.getIgnoreFilter(root, true, NON_ASSET_FILENAMES_MATCHERS),
            inResDirToOutResDirMap,
            !targetDensitiesSet.isEmpty(),
            targetDensitiesSet,
            locales,
            packagedLocales,
            enableStringsAsAssetsFiltering,
            notFilteredStringDirs,
            allowlistedLocalesDirs);

    timed(
        "Copying & filtering resources",
        () -> {
          FilteredDirectoryCopier.copyDirsParallel(
              root,
              ProjectFilesystemUtils.getIgnoreFilter(root, true, NON_ASSET_FILENAMES_MATCHERS),
              inResDirToOutResDirMap,
              filteringPredicate);
        });

    String voltronMapPath = voltronInResDirToOutResDirMapPath;
    if (voltronMapPath != null) {
      timed(
          "Copying & filtering voltron resources",
          () -> {
            Map<String, ImmutableBiMap<Path, Path>> rawVoltronMap =
                Objects.requireNonNull(
                    ObjectMappers.READER.readValue(
                        ObjectMappers.createParser(Paths.get(voltronMapPath)),
                        new TypeReference<Map<String, ImmutableBiMap<Path, Path>>>() {}));
            ImmutableBiMap<Path, Path> voltronInResDirToOutResDirMap =
                Objects.requireNonNull(rawVoltronMap.get("res_dir_map"));

            FilteredDirectoryCopier.copyDirsParallel(
                root,
                ProjectFilesystemUtils.getEmptyIgnoreFilter(),
                voltronInResDirToOutResDirMap,
                FilteringPredicate.getVoltronLanguagePackPredicate());
          });
    }

    String postFilterCmd = postFilterResourcesCmd;
    if (postFilterCmd != null) {
      timed(
          "Running post-filter-resources command",
          () -> {
            Preconditions.checkState(
                postFilterResourcesCmdOverrideSymbols != null,
                "Must specify an override symbols file if a post-filter-resources-cmd is"
                    + " specified!");
            ImmutableList.Builder<String> postFilterResourcesCmdList = ImmutableList.builder();
            postFilterResourcesCmdList
                .addAll(Arrays.stream(postFilterCmd.split("\\s+")).collect(Collectors.toList()))
                .add(inResDirToOutResDirMapPath)
                .add(postFilterResourcesCmdOverrideSymbols);
            Process postFilterResourcesProcess =
                new ProcessBuilder().command(postFilterResourcesCmdList.build()).start();
            try {
              int exitCode = postFilterResourcesProcess.waitFor();
              if (exitCode != 0) {
                String error =
                    new String(
                        Objects.requireNonNull(postFilterResourcesProcess.getErrorStream())
                            .readAllBytes());
                throw new RuntimeException("post_filter_resources_cmd failed with error: " + error);
              }
            } catch (InterruptedException e) {
              throw new RuntimeException(e);
            }
          });
    }

    // We need to output a list of all the string files if and only if we are doing
    // strings-as-assets filtering.
    String stringFilesList = stringFilesListOutput;
    Preconditions.checkState(enableStringsAsAssetsFiltering == (stringFilesList != null));
    if (stringFilesList != null) {
      timed(
          "Writing string files list",
          () -> {
            Files.write(
                Paths.get(stringFilesList),
                (Iterable<String>)
                    GetStringsFiles.getFilesAsStream(
                                root,
                                ProjectFilesystemUtils.getEmptyIgnoreFilter(),
                                inResDirToOutResDirMap.keySet().asList())
                            .map(Path::toString)
                        ::iterator);
          });
    }

    System.exit(0);
  }

  private static <E extends Exception> void timed(String name, ThrowingRunnable<E> runnable)
      throws E {
    long start = System.currentTimeMillis();
    try {
      runnable.run();
    } finally {
      long end = System.currentTimeMillis();
      System.err.println(name + " took " + (end - start) + "ms");
    }
  }
}
