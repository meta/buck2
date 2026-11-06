/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.util.unarchive.Unzip;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSet;
import java.io.IOException;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Enumeration;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

/** Utilities for handling paths to java source files. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class JavaPaths {

  public static final String SRC_ZIP = ".src.zip";
  public static final String SRC_JAR = "-sources.jar";

  /**
   * Processes a list of java source files, extracting and SRC_ZIP or SRC_JAR to the working
   * directory and returns a list of all the resulting .java files.
   */
  static ImmutableList<RelPath> extractArchivesAndGetPaths(
      AbsPath ruleCellPathRoot, ImmutableSet<RelPath> javaSourceFilePaths, Path workingDirectory)
      throws IOException {

    // Add sources file or sources list to command
    ImmutableList.Builder<RelPath> sources = ImmutableList.builder();
    for (RelPath path : javaSourceFilePaths) {
      String pathString = path.toString();
      if (pathString.endsWith(".java")) {
        sources.add(path);
      } else if (pathString.endsWith(SRC_ZIP) || pathString.endsWith(SRC_JAR)) {
        // For a Zip of .java files, create a JavaFileObject for each .java entry.
        ImmutableList<Path> zipPaths =
            Unzip.extractArchive(
                ruleCellPathRoot,
                ruleCellPathRoot.resolve(path).getPath(),
                ruleCellPathRoot.resolve(workingDirectory).getPath());
        sources.addAll(
            zipPaths.stream()
                .filter(input -> input.toString().endsWith(".java"))
                .map(ruleCellPathRoot::relativize)
                .iterator());
      }
    }
    return sources.build();
  }

  /**
   * Whether the given source paths expand to at least one {@code .java} compilation unit. A
   * SRC_ZIP/SRC_JAR staged by an annotation processor may hold no Java sources at all, so the
   * archives are inspected rather than counted.
   *
   * <p>A path that is neither a Java source nor an archive counts as a compilation unit, leaving it
   * to javac to reject.
   */
  static boolean hasJavaCompilationUnits(Iterable<Path> javaSourceFilePaths) throws IOException {
    for (Path path : javaSourceFilePaths) {
      String pathString = path.toString();
      if (!pathString.endsWith(SRC_ZIP) && !pathString.endsWith(SRC_JAR)) {
        return true;
      }
      try (ZipFile zipFile = new ZipFile(path.toFile())) {
        Enumeration<? extends ZipEntry> entries = zipFile.entries();
        while (entries.hasMoreElements()) {
          if (entries.nextElement().getName().endsWith(".java")) {
            return true;
          }
        }
      }
    }
    return false;
  }

  /**
   * Traverses a list of java inputs return the list of all found files (for SRC_ZIP/SRC_JAR, it
   * returns the paths from within the archive).
   */
  static ImmutableList<Path> getExpandedSourcePaths(Iterable<Path> javaSourceFilePaths)
      throws IOException {
    // Add sources file or sources list to command
    ImmutableList.Builder<Path> sources = ImmutableList.builder();
    for (Path path : javaSourceFilePaths) {
      String pathString = path.toString();
      if (pathString.endsWith(SRC_ZIP) || pathString.endsWith(SRC_JAR)) {
        try (ZipFile zipFile = new ZipFile(path.toFile())) {
          Enumeration<? extends ZipEntry> entries = zipFile.entries();
          while (entries.hasMoreElements()) {
            sources.add(Paths.get(entries.nextElement().getName()));
          }
        }
      } else {
        sources.add(path);
      }
    }
    return sources.build();
  }
}
