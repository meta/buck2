/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.util

import com.facebook.buck.core.filesystems.AbsPath
import com.facebook.buck.core.filesystems.RelPath
import com.facebook.buck.jvm.java.JavaPaths
import com.facebook.buck.util.unarchive.Unzip
import com.google.common.collect.ImmutableList
import com.google.common.collect.ImmutableSet
import java.io.IOException
import java.nio.file.Path
import java.util.Optional

@Throws(IOException::class)
fun getExpandedSourcePaths(
    ruleCellRoot: AbsPath,
    kotlinSourceFilePaths: ImmutableSet<RelPath>,
    workingDirectory: Optional<Path>,
): ImmutableList<Path> {
  // Add sources file or sources list to command

  val sources = ImmutableList.builder<Path>()
  for (path in kotlinSourceFilePaths) {
    val pathString = path.toString()
    if (pathString.endsWith(".kt") || pathString.endsWith(".kts") || pathString.endsWith(".java")) {
      sources.add(path.path)
    } else if (pathString.endsWith(JavaPaths.SRC_ZIP) || pathString.endsWith(JavaPaths.SRC_JAR)) {
      // For a Zip of .java files, create a JavaFileObject for each .java entry.
      val zipPaths: ImmutableList<Path> =
          Unzip.extractArchive(
              ruleCellRoot,
              ruleCellRoot.resolve(path).path,
              ruleCellRoot.resolve(workingDirectory.orElse(path.path)).path,
          )
      sources.addAll(
          zipPaths
              .stream()
              .filter { input: Path ->
                (input.toString().endsWith(".kt") ||
                    input.toString().endsWith(".kts") ||
                    input.toString().endsWith(".java"))
              }
              .iterator(),
      )
    }
  }
  return sources.build()
}
