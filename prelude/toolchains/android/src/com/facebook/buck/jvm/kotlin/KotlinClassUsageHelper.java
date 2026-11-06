/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin;

import static com.facebook.buck.jvm.java.CompilerOutputPaths.getKAPTDepFilePath;
import static com.facebook.buck.jvm.java.CompilerOutputPaths.getKotlinTempDepFilePath;
import static com.facebook.buck.jvm.java.CompilerOutputPaths.getKspDepFilePath;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.java.ClassUsageURIParser;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.collect.ImmutableMap;
import java.io.IOException;
import java.net.URI;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

/**
 * Java used-class optimization directly inject FileManager into javac process. However, that is no
 * longer possible for Kotlin. We have to modify KAPT, KSP and compiler plugin to output
 * dependencies into separate report files, then parse and merge them to get a complete map of class
 * usages.
 *
 * <p>This class includes utility functions for parsing and merging kotlin temp dep files.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class KotlinClassUsageHelper {

  /**
   * Returns a map of all the files used by the Kotlin compiler and the its annotation processors.
   */
  static ImmutableMap<Path, Set<Path>> getClassUsageData(
      RelPath reportDirPath, AbsPath ruleCellRoot) throws IOException {
    // read compiler plugin generated class usage report file
    Path kotlinGeneralClassUsageReportPath =
        ruleCellRoot.resolve(getKotlinTempDepFilePath(reportDirPath)).getPath();
    ImmutableMap<Path, Set<Path>> classUsages =
        readUriBasedClassUsageFile(kotlinGeneralClassUsageReportPath);

    // merge kapt generated class usage report file if it exist
    Path kaptClassUsageReportPath =
        ruleCellRoot.resolve(getKAPTDepFilePath(reportDirPath)).getPath();
    if (kaptClassUsageReportPath.toFile().exists()) {
      classUsages = merge(classUsages, readUriBasedClassUsageFile(kaptClassUsageReportPath));
    }

    // merge ksp generated class usage report file if it exist
    Path kspClassUsageReportPath = ruleCellRoot.resolve(getKspDepFilePath(reportDirPath)).getPath();
    if (kspClassUsageReportPath.toFile().exists()) {
      classUsages = merge(classUsages, readUriBasedClassUsageFile(kspClassUsageReportPath));
    }

    return classUsages;
  }

  /** Read a class usage report that is in the raw format one URI per line */
  @VisibleForTesting
  static ImmutableMap<Path, Set<Path>> readUriBasedClassUsageFile(Path path) throws IOException {
    if (!Files.exists(path)) {
      return ImmutableMap.of();
    }

    final ClassUsageURIParser parser = new ClassUsageURIParser();
    Files.readAllLines(path).forEach(line -> parser.parseAndRecordURI(URI.create(line)));
    return parser.getClassUsageMap();
  }

  /** Merge two class usage map */
  @VisibleForTesting
  static ImmutableMap<Path, Set<Path>> merge(Map<Path, Set<Path>> mapA, Map<Path, Set<Path>> mapB) {
    // Merge map A to resultBuilder, and ensure both inner and outer map is mutable
    HashMap<Path, Set<Path>> resultBuilder = new HashMap<>();
    for (Map.Entry<Path, Set<Path>> jarEntry : mapA.entrySet()) {
      resultBuilder.put(jarEntry.getKey(), new HashSet<>(jarEntry.getValue()));
    }
    // Merge map B to result builder. Increment count if same entry exist.
    for (Map.Entry<Path, Set<Path>> jarEntry : mapB.entrySet()) {
      Set<Path> resultClassPaths =
          resultBuilder.computeIfAbsent(jarEntry.getKey(), _path -> new HashSet<>());
      resultClassPaths.addAll(jarEntry.getValue());
    }
    return ImmutableMap.copyOf(resultBuilder);
  }
}
