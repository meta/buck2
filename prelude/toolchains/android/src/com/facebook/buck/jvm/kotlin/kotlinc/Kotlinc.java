/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.kotlinc;

import com.facebook.buck.core.build.execution.context.IsolatedExecutionContext;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDLoggingContext;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlincMode;
import com.facebook.buck.jvm.kotlin.util.KotlinUnarchiverKt;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.ImmutableSortedSet;
import java.io.IOException;
import java.nio.file.Path;
import java.util.Optional;
import org.jspecify.annotations.Nullable;

/** Interface for a kotlin compiler. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface Kotlinc {

  int buildWithClasspath(
      IsolatedExecutionContext context,
      BuildTargetValue invokingRule,
      ImmutableList<String> options,
      ImmutableList<AbsPath> kotlinHomeLibraries,
      ImmutableSortedSet<RelPath> kotlinSourceFilePaths,
      Path pathToSrcsList,
      Optional<Path> workingDirectory,
      AbsPath ruleCellRoot,
      KotlincMode mode,
      KotlinCDLoggingContext kotlinCDLoggingContext,
      AbsPath outputDirectory,
      @Nullable Path classToSourceMapPath)
      throws InterruptedException;

  String getDescription(
      ImmutableList<String> options,
      ImmutableSortedSet<RelPath> kotlinSourceFilePaths,
      Path pathToSrcsList);

  String getShortName();

  default ImmutableList<Path> getExpandedSourcePaths(
      AbsPath ruleCellRoot,
      ImmutableSet<RelPath> kotlinSourceFilePaths,
      Optional<Path> workingDirectory)
      throws IOException {

    return KotlinUnarchiverKt.getExpandedSourcePaths(
        ruleCellRoot, kotlinSourceFilePaths, workingDirectory);
  }
}
