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
import com.facebook.buck.core.util.log.Logger;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlinSourceChanges;
import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.Path;
import java.util.List;
import java.util.stream.Collectors;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class KotlinSourceChangesFactory {

  private static final Logger LOG = Logger.get(KotlinSourceChangesFactory.class);

  public static KotlinSourceChanges create(
      final AbsPath rootProjectDir, final SourceFilesActionMetadata actionMetadata) {
    List<Path> modifiedFiles = actionMetadata.calculateAddedAndModifiedSourceFiles();
    List<Path> removedFiles = actionMetadata.calculateRemovedFiles();

    LOG.debug(
        "Source file changes: \n" + "modified files: [%s], \n" + "removed files: [%s]",
        modifiedFiles, removedFiles);

    return new KotlinSourceChanges.Known(
        modifiedFiles.stream().map(rootProjectDir::resolve).collect(Collectors.toList()),
        removedFiles.stream().map(rootProjectDir::resolve).collect(Collectors.toList()));
  }
}
