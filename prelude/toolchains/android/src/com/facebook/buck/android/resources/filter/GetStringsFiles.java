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
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.buck.io.pathformat.PathFormatter;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.collect.ImmutableList;
import java.io.IOException;
import java.nio.file.DirectoryStream;
import java.nio.file.Path;
import java.util.function.Predicate;
import java.util.regex.Pattern;
import java.util.stream.Stream;

/**
 * Generates a list of strings.xml files
 *
 * <p>The ordering of strings files is consistent with the order of the input resource directories
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class GetStringsFiles {
  @VisibleForTesting
  public static final Pattern STRINGS_FILE_PATH =
      Pattern.compile("(\\b|.*/)res/values(-.+)*/strings.xml", Pattern.CASE_INSENSITIVE);

  public static ImmutableList<Path> getFiles(
      AbsPath root, DirectoryStream.Filter<? super Path> ignoreFilter, ImmutableList<Path> resDirs)
      throws IOException {
    Predicate<Path> isStringsFile =
        pathRelativeToProjectRoot -> {
          String filePath = PathFormatter.pathWithUnixSeparators(pathRelativeToProjectRoot);
          return STRINGS_FILE_PATH.matcher(filePath).matches();
        };

    ImmutableList.Builder<Path> stringFilesBuilder = ImmutableList.builder();
    for (Path resDir : resDirs) {
      stringFilesBuilder.addAll(
          ProjectFilesystemUtils.getFilesUnderPath(
              root,
              resDir,
              isStringsFile,
              ProjectFilesystemUtils.getDefaultVisitOptions(),
              ignoreFilter));
    }

    return stringFilesBuilder.build();
  }

  public static Stream<Path> getFilesAsStream(
      AbsPath root, DirectoryStream.Filter<? super Path> ignoreFilter, ImmutableList<Path> resDirs)
      throws IOException {
    Predicate<Path> isStringsFile =
        pathRelativeToProjectRoot -> {
          String filePath = PathFormatter.pathWithUnixSeparators(pathRelativeToProjectRoot);
          return STRINGS_FILE_PATH.matcher(filePath).matches();
        };

    return resDirs.stream()
        .flatMap(resDir -> getFilesUnderPathExcWrapper(root, resDir, isStringsFile, ignoreFilter));
  }

  private static Stream<Path> getFilesUnderPathExcWrapper(
      AbsPath root,
      Path resDir,
      Predicate<Path> isStringsFile,
      DirectoryStream.Filter<? super Path> ignoreFilter) {
    try {
      return ProjectFilesystemUtils.getFilesUnderPath(
          root,
          resDir,
          isStringsFile,
          ProjectFilesystemUtils.getDefaultVisitOptions(),
          ignoreFilter)
          .stream();
    } catch (IOException e) {
      throw new RuntimeException(e);
    }
  }
}
