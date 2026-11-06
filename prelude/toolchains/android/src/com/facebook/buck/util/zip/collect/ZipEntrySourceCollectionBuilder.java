/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.zip.collect;

import com.facebook.buck.io.pathformat.PathFormatter;
import com.facebook.buck.util.PatternsMatcher;
import com.facebook.buck.util.zip.Zip;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.Iterables;
import com.google.common.collect.LinkedHashMultimap;
import com.google.common.collect.Multimap;
import java.io.IOException;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Collections;
import java.util.Comparator;
import java.util.function.Function;
import java.util.regex.Pattern;

/**
 * A builder for {@link ZipEntrySourceCollection} that allows to build a collection using provided
 * source files as well as source zip files.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ZipEntrySourceCollectionBuilder {
  private final PatternsMatcher excludedEntriesMatcher;
  private final OnDuplicateEntry onDuplicateEntryAction;
  private final Function<ZipEntrySource, Void> onDuplicateEntryHandler;
  private final Multimap<String, ZipEntrySource> entryNameToEntry;

  public ZipEntrySourceCollectionBuilder(
      ImmutableSet<Pattern> entriesToExclude, OnDuplicateEntry onDuplicateEntryAction) {
    this.excludedEntriesMatcher = new PatternsMatcher(entriesToExclude);
    this.onDuplicateEntryAction = onDuplicateEntryAction;
    this.onDuplicateEntryHandler = createDuplicateEntryHandler();
    this.entryNameToEntry = LinkedHashMultimap.create();
  }

  /** Add the given file as a source for an entry with the provided name. */
  public void addFile(String entryName, Path sourceFilePath) {
    entryName = PathFormatter.pathWithUnixSeparators(entryName);
    if (excludedEntriesMatcher.matches(entryName)) {
      return;
    }
    ZipEntrySource entrySource = new FileZipEntrySource(sourceFilePath, entryName);
    Collection<ZipEntrySource> oldEntries = entryNameToEntry.get(entrySource.getEntryName());
    if (oldEntries.isEmpty()) {
      entryNameToEntry.put(entrySource.getEntryName(), entrySource);
    } else {
      onDuplicateEntryHandler.apply(entrySource);
    }
  }

  /**
   * Add all entries of a given zip file to the collection (excluding entries that match {@link
   * #excludedEntriesMatcher}.
   */
  public void addZipFile(Path zipFilePath) throws IOException {
    ImmutableList<String> zipEntries = Zip.getAllZipEntries(zipFilePath);
    for (int i = 0; i < zipEntries.size(); ++i) {
      String entryName = zipEntries.get(i);
      if (excludedEntriesMatcher.matches(entryName)) {
        continue;
      }
      ZipEntrySource entrySource = new ZipEntrySourceFromZip(zipFilePath, entryName, i);
      Collection<ZipEntrySource> oldEntries = entryNameToEntry.get(entryName);
      if (oldEntries.isEmpty()) {
        entryNameToEntry.put(entryName, entrySource);
      } else {
        onDuplicateEntryHandler.apply(entrySource);
      }
    }
  }

  public ZipEntrySourceCollection build() {
    ArrayList<ZipEntrySource> allEntries = new ArrayList<ZipEntrySource>(entryNameToEntry.values());
    Collections.sort(
        allEntries,
        new Comparator<ZipEntrySource>() {
          @Override
          public int compare(ZipEntrySource z1, ZipEntrySource z2) {
            return z1.getEntryName().compareTo(z2.getEntryName());
          }
        });
    return new ZipEntrySourceCollection(ImmutableList.copyOf(allEntries));
  }

  private Function<ZipEntrySource, Void> createDuplicateEntryHandler() {
    switch (onDuplicateEntryAction) {
      case FAIL:
        return (entry) -> {
          Collection<ZipEntrySource> oldEntries = entryNameToEntry.get(entry.getEntryName());
          throw new RuntimeException(
              String.format(
                  "Duplicate entry \"%s\" is coming from %s and %s",
                  entry.getEntryName(),
                  Iterables.getOnlyElement(oldEntries).getSourceFilePath(),
                  entry.getSourceFilePath()));
        };
      case APPEND:
        return (entry) -> {
          entryNameToEntry.put(entry.getEntryName(), entry);
          return null;
        };
      case OVERWRITE:
        return (entry) -> {
          entryNameToEntry.removeAll(entry.getEntryName());
          entryNameToEntry.put(entry.getEntryName(), entry);
          return null;
        };
      default:
        throw new IllegalArgumentException("Unknown action: " + onDuplicateEntryAction);
    }
  }
}
