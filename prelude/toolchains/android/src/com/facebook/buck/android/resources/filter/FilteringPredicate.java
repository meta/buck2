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
import com.facebook.buck.io.pathformat.PathFormatter;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.collect.ImmutableBiMap;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import java.io.IOException;
import java.nio.file.DirectoryStream;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.function.Predicate;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class FilteringPredicate {

  /** Utility class: do not instantiate. */
  private FilteringPredicate() {}

  @VisibleForTesting
  public static final Pattern NON_ENGLISH_STRINGS_FILE_PATH =
      Pattern.compile(".*/values-([a-z]{2})(?:-r([A-Z]{2}))*/strings.xml");

  public static Predicate<Path> getFilteringPredicate(
      AbsPath projectRoot,
      DirectoryStream.Filter<? super Path> ignoreFilter,
      ImmutableBiMap<Path, Path> inResDirToOutResDirMap,
      boolean filterByDensity,
      Set<ResourceFilters.Density> targetDensities,
      ImmutableSet<String> locales,
      ImmutableSet<String> packagedLocales,
      boolean enableStringWhitelisting,
      ImmutableSet<Path> whitelistedStringDirs,
      ImmutableMap<Path, ImmutableSet<String>> allowlistedLocalesDirs)
      throws IOException {
    List<Predicate<Path>> pathPredicates = new ArrayList<>();

    if (filterByDensity) {
      Objects.requireNonNull(targetDensities);
      Set<Path> rootResourceDirs = inResDirToOutResDirMap.keySet();

      pathPredicates.add(ResourceFilters.createDensityFilter(projectRoot, targetDensities));

      Set<Path> drawables =
          DrawableFinder.findDrawables(projectRoot, rootResourceDirs, ignoreFilter);
      pathPredicates.add(ResourceFilters.createImageDensityFilter(drawables, targetDensities));
    }

    boolean localeFilterEnabled = !locales.isEmpty();
    if (localeFilterEnabled || enableStringWhitelisting) {
      pathPredicates.add(
          path -> {
            String filePath = PathFormatter.pathWithUnixSeparators(path);
            Matcher matcher = NON_ENGLISH_STRINGS_FILE_PATH.matcher(filePath);
            if (!matcher.matches()) {
              return true;
            }
            String locale = Objects.requireNonNull(matcher.group(1));
            if (matcher.group(2) != null) {
              locale += "_" + matcher.group(2);
            }
            if (enableStringWhitelisting) {
              return isPathWhitelisted(
                  path, locale, packagedLocales, whitelistedStringDirs, allowlistedLocalesDirs);

            } else {
              return locales.contains(locale);
            }
          });
    }
    return pathPredicates.stream().reduce(p -> true, Predicate::and);
  }

  public static Predicate<Path> getVoltronLanguagePackPredicate() {
    return path -> {
      String filePath = PathFormatter.pathWithUnixSeparators(path);
      Matcher matcher = NON_ENGLISH_STRINGS_FILE_PATH.matcher(filePath);
      return matcher.matches();
    };
  }

  private static boolean isPathWhitelisted(
      Path path,
      String locale,
      ImmutableSet<String> packagedLocales,
      ImmutableSet<Path> whitelistedStringDirs,
      ImmutableMap<Path, ImmutableSet<String>> allowlistedLocalesDirs) {
    if (packagedLocales.contains(locale)) {
      return true;
    }
    for (Path whitelistedStringDir : whitelistedStringDirs) {
      if (path.startsWith(whitelistedStringDir)) {
        return true;
      }
    }
    for (Map.Entry<Path, ImmutableSet<String>> entry : allowlistedLocalesDirs.entrySet()) {
      if (path.startsWith(entry.getKey()) && entry.getValue().contains(locale)) {
        return true;
      }
    }

    return false;
  }
}
