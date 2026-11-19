/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.resources.strings;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.io.file.PathMatcher;
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableCollection;
import com.google.common.collect.ImmutableList;
import java.io.IOException;
import java.nio.file.Path;

/**
 * Copy filtered string resources (values/strings.xml) files to output directory. These will be used
 * by i18n to map resource_id to fbt_hash with resource_name as the intermediary
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class StringResourcesUtils {
  private static final String VALUES = "values";
  private static final String STRINGS_XML = "strings.xml";
  private static final String NEW_RES_DIR_FORMAT = "%04x";

  // "4 digit hex" => 65536 files

  /*
   * Copy all strings.xml under resDir/values to output Path
   */
  public static void copyResources(
      AbsPath projectRoot, ImmutableList<Path> resDirs, Path outputDirPath) throws IOException {
    int i = 0;
    for (Path resDir : resDirs) {
      Path stringsFilePath = resDir.resolve(VALUES).resolve(STRINGS_XML);
      if (ProjectFilesystemUtils.exists(projectRoot, stringsFilePath)) {
        // create <output_dir>/<new_res_dir>/values
        Path newStringsFileDir =
            outputDirPath.resolve(String.format(NEW_RES_DIR_FORMAT, i++)).resolve(VALUES);
        ProjectFilesystemUtils.mkdirs(projectRoot, newStringsFileDir);
        // copy <res_dir>/values/strings.xml ->
        // <output_dir>/<new_res_dir>/values/strings.xml
        ProjectFilesystemUtils.copyFile(
            projectRoot, stringsFilePath, newStringsFileDir.resolve(STRINGS_XML));
      }
    }
  }

  /**
   * Copy all strings.xml under resDir/values-xx to output Path
   *
   * @param resDirs List of directories that contains values-xx/strings.
   */
  public static void copyVoltronStringResources(
      AbsPath projectRoot,
      ImmutableList<Path> resDirs,
      ImmutableCollection<PathMatcher> ignores,
      Path outputDirPath)
      throws IOException {
    int i = 0;
    for (Path resDir : resDirs) {
      if (ProjectFilesystemUtils.exists(projectRoot, resDir)) {
        ImmutableCollection<Path> dirs =
            ProjectFilesystemUtils.getDirectoryContents(projectRoot, ignores, resDir);
        boolean containNonEnglishStrings = false;
        for (Path dir : dirs) {
          String filename = dir.getFileName().toString();
          if (filename.startsWith(VALUES) && !filename.equals(VALUES)) {
            Path stringsFilePath = resDir.resolve(filename).resolve(STRINGS_XML);
            if (ProjectFilesystemUtils.exists(projectRoot, stringsFilePath)) {
              // create <output_dir>/<new_res_dir>/values-xx
              Path newStringsFileDir =
                  outputDirPath.resolve(String.format(NEW_RES_DIR_FORMAT, i)).resolve(filename);
              ProjectFilesystemUtils.mkdirs(projectRoot, newStringsFileDir);
              // copy <res_dir>/values-es/strings.xml ->
              // <output_dir>/<new_res_dir>/values-es/strings.xml
              ProjectFilesystemUtils.copyFile(
                  projectRoot, stringsFilePath, newStringsFileDir.resolve(STRINGS_XML));
              containNonEnglishStrings = true;
            }
          }
        }
        if (containNonEnglishStrings) {
          ++i;
        }
      }
    }
  }
}
