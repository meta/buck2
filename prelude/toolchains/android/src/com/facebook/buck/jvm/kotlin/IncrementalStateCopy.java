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

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import java.io.IOException;
import java.nio.file.FileVisitResult;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.SimpleFileVisitor;
import java.nio.file.StandardCopyOption;
import java.nio.file.attribute.BasicFileAttributes;

/**
 * A copy of the directories of an incremental round's state (the incremental compiler's working
 * directory and the class outputs), taken before a full compile replaces them, that is put back if
 * that compile fails. A failed compile then leaves the state of the last successful round, so the
 * next round can still be incremental.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
final class IncrementalStateCopy implements KotlincStep.StateCopy {
  private final Path copyRoot;
  private final ImmutableList<Path> directories;

  private IncrementalStateCopy(Path copyRoot, ImmutableList<Path> directories) {
    this.copyRoot = copyRoot;
    this.directories = directories;
  }

  /** Copies {@code directories} under {@code copyRoot}, which must lie outside all of them. */
  static IncrementalStateCopy copy(Path copyRoot, ImmutableList<Path> directories)
      throws IOException {
    deleteRecursively(copyRoot);
    Files.createDirectories(copyRoot);
    for (int index = 0; index < directories.size(); index++) {
      Path directory = directories.get(index);
      if (Files.isDirectory(directory)) {
        copyTree(directory, copyRoot.resolve(Integer.toString(index)));
      }
    }
    return new IncrementalStateCopy(copyRoot, directories);
  }

  /**
   * Replaces the directories with their copies. If that fails, deletes them, so that the next round
   * compiles in full instead of starting from a mixed state.
   */
  @Override
  public void restore() throws IOException {
    try {
      for (int index = 0; index < directories.size(); index++) {
        Path directory = directories.get(index);
        Path copy = copyRoot.resolve(Integer.toString(index));
        deleteRecursively(directory);
        if (Files.isDirectory(copy)) {
          Files.move(copy, directory, StandardCopyOption.ATOMIC_MOVE);
        }
      }
    } catch (IOException e) {
      for (Path directory : directories) {
        deleteRecursively(directory);
      }
      throw e;
    } finally {
      deleteRecursively(copyRoot);
    }
  }

  @Override
  public void discard() throws IOException {
    deleteRecursively(copyRoot);
  }

  private static void copyTree(Path source, Path target) throws IOException {
    Files.walkFileTree(
        source,
        new SimpleFileVisitor<Path>() {
          @Override
          public FileVisitResult preVisitDirectory(Path dir, BasicFileAttributes attrs)
              throws IOException {
            Files.createDirectories(target.resolve(source.relativize(dir)));
            return FileVisitResult.CONTINUE;
          }

          @Override
          public FileVisitResult visitFile(Path file, BasicFileAttributes attrs)
              throws IOException {
            Files.copy(
                file,
                target.resolve(source.relativize(file)),
                StandardCopyOption.COPY_ATTRIBUTES,
                java.nio.file.LinkOption.NOFOLLOW_LINKS);
            return FileVisitResult.CONTINUE;
          }
        });
  }

  private static void deleteRecursively(Path path) throws IOException {
    if (!Files.exists(path, java.nio.file.LinkOption.NOFOLLOW_LINKS)) {
      return;
    }
    Files.walkFileTree(
        path,
        new SimpleFileVisitor<Path>() {
          @Override
          public FileVisitResult visitFile(Path file, BasicFileAttributes attrs)
              throws IOException {
            Files.delete(file);
            return FileVisitResult.CONTINUE;
          }

          @Override
          public FileVisitResult postVisitDirectory(Path dir, IOException exception)
              throws IOException {
            if (exception != null) {
              throw exception;
            }
            Files.delete(dir);
            return FileVisitResult.CONTINUE;
          }
        });
  }
}
