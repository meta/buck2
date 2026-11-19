/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.exopackage;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.base.Joiner;
import com.google.common.collect.ImmutableMap;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.stream.Stream;

/** Installs resources for exo. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ResourcesExoHelper implements ExoHelper {

  @VisibleForTesting public static final Path RESOURCES_DIR = Paths.get("resources");

  private final AbsPath rootPath;
  private final IsolatedExopackageInfo.IsolatedResourcesInfo resourcesInfo;

  ResourcesExoHelper(AbsPath rootPath, IsolatedExopackageInfo.IsolatedResourcesInfo resourcesInfo) {
    this.rootPath = rootPath;
    this.resourcesInfo = resourcesInfo;
  }

  private static ImmutableMap<Path, Path> getFilesToInstall(
      ImmutableMap<String, Path> filesByHash) {
    return ExopackageUtil.applyFilenameFormat(filesByHash, RESOURCES_DIR, "%s.apk");
  }

  /** Returns a map of hash to path for resource files. */
  private static ImmutableMap<String, Path> getResourceFilesByHash(
      AbsPath rootPath,
      Stream<IsolatedExopackageInfo.IsolatedExopackagePathAndHash> resourcesPaths) {
    return resourcesPaths.collect(
        ImmutableMap.toImmutableMap(
            pathAndHash -> readHash(rootPath, pathAndHash),
            pathAndHash -> pathAndHash.getFile().getPath()));
  }

  /**
   * A resource's hash is what names it on the device, so a resource whose hash cannot be read
   * cannot be installed. Fail loudly rather than dropping it: this map is used both to decide what
   * to push and to decide what is still wanted, so a silently missing entry means the previous copy
   * is collected, the new one is never pushed, and the install reports success anyway.
   */
  private static String readHash(
      AbsPath rootPath, IsolatedExopackageInfo.IsolatedExopackagePathAndHash pathAndHash) {
    Path hashFile = pathAndHash.getHashFile().getPath();
    return ProjectFilesystemUtils.readFileIfItExists(rootPath, hashFile)
        .orElseThrow(
            () ->
                new IllegalStateException(
                    String.format(
                        "Missing exopackage resource hash file %s for resource %s",
                        hashFile, pathAndHash.getFile().getPath())))
        .stripTrailing();
  }

  @Override
  public String getType() {
    return "resources";
  }

  @Override
  public ImmutableMap<Path, Path> getFilesToInstall() {
    return getFilesToInstall(getResourceFilesByHash());
  }

  @Override
  public ImmutableMap<Path, String> getMetadataToInstall() {
    return ImmutableMap.of(
        RESOURCES_DIR.resolve("metadata.txt"),
        getResourceMetadataContents(getResourceFilesByHash()));
  }

  private ImmutableMap<String, Path> getResourceFilesByHash() {
    return getResourceFilesByHash(rootPath, resourcesInfo.getResourcesPaths().stream());
  }

  private static String getResourceMetadataContents(ImmutableMap<String, Path> filesByHash) {
    return Joiner.on("\n")
        .join(filesByHash.keySet().stream().map(h -> "resources " + h).iterator());
  }
}
