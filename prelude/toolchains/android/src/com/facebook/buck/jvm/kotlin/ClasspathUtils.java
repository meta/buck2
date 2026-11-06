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

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.io.filesystem.CopySourceMode;
import com.facebook.buck.jvm.core.BuildTargetValueExtraParams;
import com.facebook.buck.jvm.java.CompilerParameters;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.CopyIsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.MakeCleanDirectoryIsolatedStep;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableCollection;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSortedSet;
import com.google.common.collect.Ordering;
import java.util.Comparator;
import java.util.HashMap;
import java.util.Map;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class ClasspathUtils {
  private final AbsPath buildCellRootPath;
  private final ImmutableSortedSet<AbsPath> resolvedFriendPaths;
  private final ImmutableList<RelPath> classpaths;
  private final ImmutableList<AbsPath> extraClasspaths;
  private final BuildTargetValueExtraParams buildTargetValueExtraParams;

  private Map<AbsPath, AbsPath> remappedClasspathEntries = new HashMap<>();

  public ClasspathUtils(
      AbsPath buildCellRootPath,
      ImmutableSortedSet<AbsPath> resolvedFriendPaths,
      ImmutableList<RelPath> classpaths,
      ImmutableList<AbsPath> extraClasspaths,
      BuildTargetValueExtraParams buildTargetValueExtraParams) {
    this.buildCellRootPath = buildCellRootPath;
    this.resolvedFriendPaths = resolvedFriendPaths;
    this.classpaths = classpaths;
    this.extraClasspaths = extraClasspaths;
    this.buildTargetValueExtraParams = buildTargetValueExtraParams;
  }

  public String getFriendPathArgs(ImmutableList.Builder<IsolatedStep> steps) {
    ImmutableCollection<AbsPath> friendPathsSourcePaths = getFriendAbsPaths(steps);
    if (friendPathsSourcePaths.isEmpty()) {
      return "";
    }

    // https://youtrack.jetbrains.com/issue/KT-29933
    ImmutableSortedSet<String> absoluteFriendPaths =
        friendPathsSourcePaths.stream()
            .map(AbsPath::toString)
            .collect(ImmutableSortedSet.toImmutableSortedSet(Ordering.natural()));

    return "-Xfriend-paths=" + String.join(",", absoluteFriendPaths);
  }

  private ImmutableSortedSet<AbsPath> getFriendAbsPaths(ImmutableList.Builder<IsolatedStep> steps) {
    ImmutableSortedSet.Builder<AbsPath> friendAbsPathsBuilder =
        ImmutableSortedSet.orderedBy(Comparator.comparing(AbsPath::getPath));

    // Currently, kotlinc can't handle commas (`,`) in paths when passed to the `-Xfriend-paths`
    // flag, so if we see a comma, we copy the JAR to a new path w/o one.
    RelPath friendPathScratchDir =
        buildTargetValueExtraParams.getScratchPath("__%s_friend_path_jars__");
    friendPathScratchDir =
        friendPathScratchDir
            .getParent()
            .resolveRel(friendPathScratchDir.getFileName().toString().replace(",", "__"));

    for (AbsPath friendPath : resolvedFriendPaths) {
      // If this path has a comma, copy to a new location that doesn't have one.
      if (friendPath.getPath().toString().contains(",")) {
        if (remappedClasspathEntries.isEmpty()) {
          steps.addAll(MakeCleanDirectoryIsolatedStep.of(friendPathScratchDir));
        }
        AbsPath dest =
            buildCellRootPath.resolve(friendPathScratchDir.resolve(friendPath.getFileName()));
        steps.add(CopyIsolatedStep.of(friendPath.getPath(), dest.getPath(), CopySourceMode.FILE));
        remappedClasspathEntries.put(friendPath, dest);
        friendPath = dest;
      }
      friendAbsPathsBuilder.add(friendPath);
    }
    ImmutableSortedSet<AbsPath> friendAbsPaths = friendAbsPathsBuilder.build();
    return friendAbsPaths;
  }

  public static boolean assertValidClasspathsPattern(AbsPath absPath) {
    String path = absPath.toString();
    if (!(path.endsWith(".jar") || path.endsWith(".zip") || path.contains("buck-out/"))) {
      throw new AssertionError(
          String.format(
              "classpath %s is not supported, only jar, zip file or a directory inside `buck-out/`"
                  + " are allowed",
              path));
    }
    return true;
  }

  public ImmutableList<AbsPath> getAllClasspaths(ImmutableList.Builder<IsolatedStep> steps) {
    ImmutableList.Builder<AbsPath> classpathBuilder =
        ImmutableList.<AbsPath>builder()
            .addAll(
                classpaths.stream()
                    .map(buildCellRootPath::resolve)
                    .map(AbsPath::normalize)
                    .map(p -> remappedClasspathEntries.getOrDefault(p, p))
                    .filter(path -> path != null && assertValidClasspathsPattern(path))
                    .iterator())
            .addAll(
                extraClasspaths.stream()
                    .map(p -> remappedClasspathEntries.getOrDefault(p, p))
                    .filter(path -> path != null && assertValidClasspathsPattern(path))
                    .iterator());
    ImmutableList<AbsPath> allClasspaths = classpathBuilder.build();
    return allClasspaths;
  }

  /**
   * Returns a list of classpath snapshots for all `.jar`s in the classpath - uses snapshot for
   * `.jar` if provided - adds a step to generate snapshot for a `.jar` if not provided
   *
   * @param parameters Compiler parameters including output paths and provided classpath snapshots.
   * @param steps A step builder to which new steps might be added to generate snapshots.
   * @param rootPath The root path of the project.
   * @param allClasspaths A list of all classpaths to consider for snapshot generation.
   * @return An immutable list of paths to the classpath snapshots.
   */
  public static ImmutableList<AbsPath> getClasspathSnapshots(
      CompilerParameters parameters,
      ImmutableList.Builder<IsolatedStep> steps,
      AbsPath rootPath,
      ImmutableList<AbsPath> allClasspaths,
      ImmutableList<AbsPath> extraClassPathSnapshots) {
    ImmutableList.Builder<AbsPath> classpathSnapshotsBuilder = ImmutableList.builder();
    classpathSnapshotsBuilder.addAll(extraClassPathSnapshots);
    parameters.getClasspathSnapshots().stream()
        .map(rootPath::resolve)
        .map(AbsPath::normalize)
        .forEach(classpathSnapshotsBuilder::add);
    ImmutableList<AbsPath> classpathSnapshots = classpathSnapshotsBuilder.build();

    if (classpathSnapshots.size() < allClasspaths.size()) {
      throw new IllegalStateException(
          String.format(
              "Classpath snapshots size (%d) does not match classpath size (%d). "
                  + "When compiling incrementally, there must be a classpath snapshot "
                  + "for every classpath item. Please ensure all classpath entries have "
                  + "corresponding snapshots.",
              classpathSnapshots.size(), allClasspaths.size()));
    }

    return classpathSnapshotsBuilder.build();
  }
}
