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

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.io.file.MorePosixFilePermissions;
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.buck.io.pathformat.PathFormatter;
import com.facebook.buck.util.zip.CustomZipEntry;
import com.facebook.buck.util.zip.CustomZipOutputStream;
import com.facebook.buck.util.zip.ZipOutputStreams;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.HashMultimap;
import com.google.common.collect.Multimap;
import com.google.common.io.ByteStreams;
import java.io.BufferedInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.attribute.PosixFilePermission;
import java.util.ArrayDeque;
import java.util.Deque;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;

/** Writes a {@link ZipEntrySourceCollection} to a zip file. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ZipEntrySourceCollectionWriter {

  private final AbsPath rootPath;
  private final boolean hardcodePermissions;

  public ZipEntrySourceCollectionWriter(AbsPath rootPath, boolean hardcodePermissions) {
    this.rootPath = rootPath;
    this.hardcodePermissions = hardcodePermissions;
  }

  /** Creates a zip archive in a given file by copying all entries listed in the collection. */
  public void copyToZip(ZipEntrySourceCollection collection, Path outputFile) throws IOException {
    // For every source archive contains list of entries and their positions to include when
    // copying.
    Map<Path, Multimap<String, Integer>> sourceArchiveEntries = new HashMap<>();
    collection.getSources().stream()
        .filter(ZipEntrySourceFromZip.class::isInstance)
        .map(ZipEntrySourceFromZip.class::cast)
        .forEach(
            entry ->
                Objects.requireNonNull(
                        sourceArchiveEntries.compute(
                            entry.getSourceFilePath(),
                            (k, v) -> (v == null ? HashMultimap.create() : v)))
                    .put(entry.getEntryName(), entry.getEntryPosition()));

    Set<Path> seenFiles = new HashSet<>();
    try (OutputStream baseOut = ProjectFilesystemUtils.newFileOutputStream(rootPath, outputFile);
        CustomZipOutputStream zip = ZipOutputStreams.newSimpleOutputStream(baseOut)) {
      for (ZipEntrySource entrySource : collection.getSources()) {
        if (!seenFiles.add(entrySource.getSourceFilePath())) {
          continue;
        }
        Path sourceFilePath = entrySource.getSourceFilePath();
        String entryName = entrySource.getEntryName();
        if (entrySource instanceof FileZipEntrySource) {
          addDirectoryEntries(zip, seenFiles, entryName);
          copyFile(zip, entryName, sourceFilePath);
        } else if (entrySource instanceof ZipEntrySourceFromZip) {
          copyZip(
              zip,
              sourceFilePath,
              seenFiles,
              sourceArchiveEntries.getOrDefault(sourceFilePath, HashMultimap.create()));
        }
      }
    }
  }

  /**
   * For a given entry name adds entries for all parent directories unless they are already added.
   */
  private void addDirectoryEntries(CustomZipOutputStream out, Set<Path> seenFiles, String entryName)
      throws IOException {
    Path entryPath = ProjectFilesystemUtils.getPath(rootPath, entryName).getParent();
    if (entryPath == null) {
      return;
    }
    Deque<Path> directoriesToAdd = new ArrayDeque<>(entryPath.getNameCount());
    while (entryPath != null && seenFiles.add(entryPath)) {
      directoriesToAdd.push(entryPath);
      entryPath = entryPath.getParent();
    }
    while (!directoriesToAdd.isEmpty()) {
      Path currentPath = directoriesToAdd.pop();
      CustomZipEntry entry =
          new CustomZipEntry(PathFormatter.pathWithUnixSeparators(currentPath) + "/");
      entry.setFakeTime();
      out.putNextEntry(entry);
      out.closeEntry();
    }
  }

  private void copyFile(CustomZipOutputStream out, String entryName, Path from) throws IOException {
    CustomZipEntry entry = new CustomZipEntry(entryName);
    entry.setFakeTime();
    entry.setExternalAttributes(getFileAttributesForZipEntry(from));

    out.putNextEntry(entry);
    try (InputStream input = ProjectFilesystemUtils.newFileInputStream(rootPath, from)) {
      ByteStreams.copy(input, out);
    }
    out.closeEntry();
  }

  public long getFileAttributesForZipEntry(Path path) throws IOException {
    long permissions;
    if (this.hardcodePermissions) {
      /*
       * Goal:
       *   We need consistent unix permissions for all zip entries, because we want the zip file to be a byte-for-byte match across different execution platforms.
       * Problem #1:
       *   On Windows, there are no unix permissions to be retrieved, all we can do is guess.
       * Problem #2:
       *   Even across flavors of Unix, RE makes all inputs read-only, so permissions can vary.
       * Solution:
       *   The only truly reliable way to get consistent permissions is to force the same one for all zip entries.
       */
      permissions =
          MorePosixFilePermissions.toMode(
              Set.of(
                  PosixFilePermission.OWNER_WRITE,
                  PosixFilePermission.OWNER_READ,
                  PosixFilePermission.GROUP_READ,
                  PosixFilePermission.OTHERS_READ)); // i.e. 0644
      return permissions << 16;
    } else {
      permissions = ProjectFilesystemUtils.getPosixFileModes(rootPath, path);
    }
    return permissions << 16;
  }

  private static void copyZip(
      CustomZipOutputStream out,
      Path from,
      Set<Path> seenFiles,
      Multimap<String, Integer> allowedEntries)
      throws IOException {
    try (ZipInputStream in =
        new ZipInputStream(new BufferedInputStream(Files.newInputStream(from)))) {
      int position = 0;
      for (ZipEntry entry = in.getNextEntry();
          entry != null;
          entry = in.getNextEntry(), position++) {
        if (!allowedEntries.containsKey(entry.getName())) {
          continue;
        }
        if (!allowedEntries.get(entry.getName()).contains(position)) {
          continue;
        }
        if (entry.isDirectory()) {
          seenFiles.add(Paths.get(entry.getName()));
        }
        CustomZipEntry customEntry = new CustomZipEntry(entry);
        customEntry.setFakeTime();
        out.putNextEntry(customEntry);
        ByteStreams.copy(in, out);
        out.closeEntry();
      }
    }
  }
}
