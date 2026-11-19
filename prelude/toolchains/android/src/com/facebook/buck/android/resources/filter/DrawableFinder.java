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
import com.google.common.collect.ImmutableSet;
import java.io.IOException;
import java.nio.file.DirectoryStream;
import java.nio.file.FileVisitResult;
import java.nio.file.Path;
import java.nio.file.SimpleFileVisitor;
import java.nio.file.attribute.BasicFileAttributes;
import java.util.Collection;
import java.util.regex.Pattern;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class DrawableFinder {

  private static final Pattern DRAWABLE_PATH_PATTERN =
      Pattern.compile(".*drawable.*/.*(png|jpg|jpeg|gif|webp|xml)", Pattern.CASE_INSENSITIVE);
  // Android doesn't scale these, so we don't need to scale or filter them either.
  private static final Pattern DRAWABLE_EXCLUDE_PATTERN =
      Pattern.compile(".*-nodpi.*", Pattern.CASE_INSENSITIVE);

  /** Utility class: do not instantiate. */
  private DrawableFinder() {}

  public static ImmutableSet<Path> findDrawables(
      AbsPath projectRoot, Collection<Path> dirs, DirectoryStream.Filter<? super Path> ignoreFilter)
      throws IOException {
    ImmutableSet.Builder<Path> drawableBuilder = ImmutableSet.builder();
    for (Path dir : dirs) {
      ProjectFilesystemUtils.walkRelativeFileTree(
          projectRoot,
          dir,
          ProjectFilesystemUtils.getDefaultVisitOptions(),
          new SimpleFileVisitor<>() {
            @Override
            public FileVisitResult visitFile(Path path, BasicFileAttributes attributes) {
              String unixPath = PathFormatter.pathWithUnixSeparators(path);
              if (DRAWABLE_PATH_PATTERN.matcher(unixPath).matches()
                  && !DRAWABLE_EXCLUDE_PATTERN.matcher(unixPath).matches()) {
                // The path is normalized so that the value can be matched against patterns.
                drawableBuilder.add(path);
              }
              return FileVisitResult.CONTINUE;
            }
          },
          ignoreFilter);
    }
    return drawableBuilder.build();
  }
}
