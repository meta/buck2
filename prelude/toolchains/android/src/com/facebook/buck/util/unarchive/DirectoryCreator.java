/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.unarchive;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.nio.file.Path;
import java.util.HashSet;
import java.util.Set;

/**
 * Simple class that creates directories, but keeps track of what it has created to ensure that it
 * does not create directories more than once. This is useful when extracting archives that may or
 * may not have duplicate entries, or to just ensure that a directory always exists before writing
 * out a new file
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class DirectoryCreator {

  private final AbsPath projectRoot;
  private final Set<Path> existing = new HashSet<>();

  /**
   * Creates an instance of {@link DirectoryCreator}
   *
   * @param projectRoot The root path to operate on
   */
  public DirectoryCreator(AbsPath projectRoot) {
    this.projectRoot = projectRoot;
    this.existing.add(projectRoot.getPath());
  }

  /**
   * Get the set of directories that were either created by this class, or manually specified in
   * {@link DirectoryCreator#recordPath(Path)}
   *
   * @return Set of directories
   */
  public Set<Path> recordedDirectories() {
    return existing;
  }

  /**
   * Ensure that we record that a path was created. This can be useful if for some reason a
   * directory is created externally, but the list of created directories is pulled from an instance
   * of {@link DirectoryCreator}
   */
  public void recordPath(Path target) {
    existing.add(target);
  }

  public AbsPath getProjectRoot() {
    return projectRoot;
  }

  /**
   * Create a directory and all of its parents
   *
   * @param target The path where a directory should exist
   * @throws IOException If the directory could not be created
   */
  public void mkdirs(Path target) throws IOException {
    if (existing.contains(target)) {
      return;
    }
    ProjectFilesystemUtils.mkdirs(projectRoot, target);
    while (target != null) {
      existing.add(target);
      target = target.getParent();
    }
  }

  /**
   * Force a directory to be created. This will delete existing files (not directories) if they
   * share the name of the directory, including parent paths
   *
   * @param target The path where a directory should exist
   * @throws IOException The directory could not be created, or files that needed to be removed
   *     could not be removed
   */
  public void forcefullyCreateDirs(Path target) throws IOException {
    if (existing.contains(target)) {
      return;
    }
    if (ProjectFilesystemUtils.exists(projectRoot, target)) {
      if (!ProjectFilesystemUtils.isDirectory(projectRoot, target)) {
        ProjectFilesystemUtils.deleteFileAtPath(projectRoot, target);
        mkdirs(target);
      }
    } else {
      if (target.getParent() != null) {
        forcefullyCreateDirs(target.getParent());
      }
      mkdirs(target);
    }
    existing.add(target);
  }
}
