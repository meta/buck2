/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.aapt;

import com.android.ide.common.rendering.api.ResourceNamespace;
import com.android.ide.common.resources.MergedResourceWriter;
import com.android.ide.common.resources.MergingException;
import com.android.ide.common.resources.NoOpResourcePreprocessor;
import com.android.ide.common.resources.ResourceMerger;
import com.android.ide.common.resources.ResourcePathEncoding;
import com.android.ide.common.resources.ResourceSet;
import com.android.utils.ILogger;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import java.nio.file.Path;
import java.util.Objects;

/** Merges multiple directories containing Android resource sources into one directory. */
@Nullsafe(Nullsafe.Mode.LOCAL)
class MergeAndroidResourceSourcesUtils {

  public static void mergeResPaths(
      ImmutableList<Path> resPaths, Path outFolderPath, Path tmpFolderPath, ILogger logger)
      throws MergingException {
    ResourceMerger merger = new ResourceMerger(1);
    for (Path resPath : resPaths) {
      ResourceSet set =
          new ResourceSet(
              resPath.toString(),
              Objects.requireNonNull(ResourceNamespace.RES_AUTO),
              /* libraryName */ null,
              /* validateEnabled */ true,
              /* aaptEnv */ null);
      set.setDontNormalizeQualifiers(true);
      set.addSource(resPath.toFile());
      set.loadFromFiles(logger);
      merger.addDataSet(set);
    }
    MergedResourceWriter writer =
        MergedResourceWriter.createWriterWithoutPngCruncher(
            outFolderPath.toFile(),
            null /*publicFile*/,
            null /*blameLogFolder*/,
            Objects.requireNonNull(NoOpResourcePreprocessor.INSTANCE),
            tmpFolderPath.toFile(),
            "unused_package_name",
            new ResourcePathEncoding.AbsoluteNotRelocatable("Android Gradle Plugin only feature"));
    merger.mergeData(writer, /* cleanUp */ false);
  }
}
