/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.zip;

import static com.google.common.base.Suppliers.memoize;
import static java.util.Comparator.comparing;
import static java.util.stream.Collectors.toList;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.io.file.MorePaths;
import com.facebook.buck.io.pathformat.PathFormatter;
import com.facebook.buck.util.function.ThrowingSupplier;
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.io.InputStream;
import java.io.UncheckedIOException;
import java.nio.file.FileVisitOption;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Objects;
import java.util.function.Supplier;
import java.util.jar.Manifest;
import java.util.stream.Stream;
import javax.annotation.Nullable;

/**
 * Provides all files and directories recursively contained in a given directory as entries to be
 * added to a jar.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class DirectoryJarEntryContainer implements JarEntryContainer {

  private final Path directory;

  @Nullable private Supplier<List<FileJarEntry>> entriesSupplier;

  DirectoryJarEntryContainer(AbsPath absPath) {
    this(absPath.getPath());
  }

  DirectoryJarEntryContainer(Path directory) {
    this.directory = directory;
  }

  @Nullable
  @Override
  public Manifest getManifest() throws IOException {
    Path manifestPath = directory.resolve("META-INF").resolve("MANIFEST.MF");

    if (!Files.isRegularFile(manifestPath)) {
      return null;
    }

    try (InputStream manifestStream = Files.newInputStream(manifestPath)) {
      return new Manifest(manifestStream);
    }
  }

  public List<FileJarEntry> getEntries() {
    return getEntriesSupplier().get();
  }

  @Override
  public Stream<JarEntrySupplier> stream() {
    return getEntries().stream().map(JarEntrySupplier.class::cast);
  }

  @Override
  public void close() {
    entriesSupplier = null;
  }

  private Supplier<List<FileJarEntry>> getEntriesSupplier() {
    Supplier<List<FileJarEntry>> localSupplier = entriesSupplier;
    if (localSupplier == null) {
      entriesSupplier =
          localSupplier =
              memoize(
                  () -> {
                    try (Stream<Path> directoryStream =
                        Files.walk(directory, FileVisitOption.FOLLOW_LINKS)) {
                      return directoryStream
                          .map(this::createFileJarEntry)
                          .filter(Objects::nonNull)
                          .sorted(comparing(FileJarEntry::getName))
                          .collect(toList());
                    } catch (IOException e) {
                      throw new UncheckedIOException(e);
                    }
                  });
    }
    return localSupplier;
  }

  @Nullable
  private FileJarEntry createFileJarEntry(final Path path) {
    final String relativePath =
        PathFormatter.pathWithUnixSeparators(MorePaths.relativize(directory, path));
    if (relativePath.isEmpty()) {
      return null;
    }
    return FileJarEntry.of(relativePath, path);
  }

  /**
   * Represents a JAR entry that originates from a file. The entry name is relative to its
   * container.
   */
  static class FileJarEntry extends JarEntrySupplier {

    static FileJarEntry of(final String entryName, final Path file) {
      try {
        final boolean directory = Files.isDirectory(file);
        final long fileSize = directory ? 0 : Files.size(file);
        return new FileJarEntry(entryName, file, fileSize, directory);
      } catch (IOException e) {
        throw new UncheckedIOException(e);
      }
    }

    private final Path file;
    private final long fileSize;
    private final boolean directory;

    public FileJarEntry(
        final String entryName, final Path file, final long fileSize, final boolean directory) {
      super(new CustomZipEntry(directory ? entryName + '/' : entryName), null);
      this.file = file;
      this.fileSize = fileSize;
      this.directory = directory;
    }

    public long getFileSize() {
      return fileSize;
    }

    public boolean isDirectory() {
      return directory;
    }

    String getName() {
      return getEntry().getName();
    }

    @Override
    public ThrowingSupplier<InputStream, IOException> getInputStreamSupplier() {
      if (file == null || directory) {
        return () -> null;
      }
      return () -> Files.newInputStream(file);
    }

    @Override
    public boolean equals(@Nullable Object o) {
      if (this == o) {
        return true;
      }
      if (o == null || getClass() != o.getClass()) {
        return false;
      }
      final FileJarEntry that = (FileJarEntry) o;
      return Objects.equals(getName(), that.getName());
    }

    @Override
    public int hashCode() {
      return Objects.hash(getName());
    }

    @Override
    public String toString() {
      return getName();
    }
  }
}
