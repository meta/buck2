/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.features.zip.rules.utils;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.util.zip.collect.OnDuplicateEntry;
import com.facebook.buck.util.zip.collect.ZipEntrySourceCollection;
import com.facebook.buck.util.zip.collect.ZipEntrySourceCollectionBuilder;
import com.facebook.buck.util.zip.collect.ZipEntrySourceCollectionWriter;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import java.io.IOException;
import java.nio.file.Path;
import java.util.Map;
import java.util.regex.Pattern;

/** Utilities method related to zip_file rule */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ZipUtils {

  /** Creates zip file */
  public static void createZipFile(
      AbsPath ruleCellRoot,
      ImmutableMap<RelPath, RelPath> entryMap,
      ImmutableList<RelPath> zipSources,
      ImmutableSet<Pattern> entriesToExclude,
      OnDuplicateEntry onDuplicateEntry,
      RelPath outputPath,
      boolean requiresDeterministicOutput)
      throws IOException {
    ZipEntrySourceCollection zipEntrySourceCollection =
        buildCollection(
            ruleCellRoot,
            entryMap,
            zipSources,
            entriesToExclude,
            onDuplicateEntry,
            requiresDeterministicOutput);
    new ZipEntrySourceCollectionWriter(ruleCellRoot, requiresDeterministicOutput)
        .copyToZip(zipEntrySourceCollection, outputPath.getPath());
  }

  private static ZipEntrySourceCollection buildCollection(
      AbsPath ruleCellRoot,
      ImmutableMap<RelPath, RelPath> entryMap,
      ImmutableList<RelPath> zipSources,
      ImmutableSet<Pattern> entriesToExclude,
      OnDuplicateEntry onDuplicateEntry,
      boolean requiresDeterministicOutput) {
    ZipEntrySourceCollectionBuilder builder =
        new ZipEntrySourceCollectionBuilder(entriesToExclude, onDuplicateEntry);
    for (RelPath zipPath : zipSources) {
      AbsPath zipSourceAbsPath = ruleCellRoot.resolve(zipPath);
      try {
        builder.addZipFile(zipSourceAbsPath.getPath());
      } catch (IOException e) {
        throw new RuntimeException(
            String.format(
                "Error while reading archive entries from %s: %s",
                zipSourceAbsPath, e.getMessage()),
            e);
      }
    }
    for (Map.Entry<RelPath, RelPath> pathEntry : entryMap.entrySet()) {
      String entryName = pathEntry.getKey().toString();
      AbsPath entryAbsPath = ruleCellRoot.resolve(pathEntry.getValue());
      builder.addFile(entryName, entryAbsPath.getPath());
    }
    return builder.build();
  }

  /**
   * Returns a map where given {@link AbsPath} instances are resolved relatively to the given root
   * path.
   */
  public static ImmutableMap<RelPath, RelPath> toRelPathEntryMap(
      ImmutableMap<Path, AbsPath> entryPathToAbsolutePathMap, AbsPath rootPath) {
    return entryPathToAbsolutePathMap.entrySet().stream()
        .collect(
            ImmutableMap.toImmutableMap(
                e -> RelPath.of(e.getKey()), e -> rootPath.relativize(e.getValue())));
  }
}
