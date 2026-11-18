/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:JvmName("PreviousOutputs")

package com.facebook.buck.jvm.kotlin.cd.workertool.postexecutors

import java.io.IOException
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.nio.file.attribute.FileTime
import kotlin.streams.toList

/**
 * The directory, under the incremental state, that holds [reclaim]'s copies while it makes them.
 * kotlincd owns it: nothing else writes there, so its leftovers can be removed by path.
 */
const val RECLAIM_DIRECTORY: String = "reclaim_tmp"

/**
 * A file kotlincd keeps in the incremental state so that it can tell whether Buck2 sealed the
 * outputs of the last run.
 */
const val RECLAIM_SENTINEL: String = "reclaim_sentinel"

/**
 * Whether the outputs of an incremental action may be read-only: its [sentinel] is missing or
 * read-only.
 *
 * Under `buck2.readonly_outputs`, Buck2 makes every output of a run read-only, a failed run's
 * included, and makes them writable again only before a run that starts from a recorded success. A
 * run that starts from a failed run's outputs finds them read-only. The sentinel is sealed with the
 * other outputs, so one check tells the two cases apart; a missing sentinel (the first round, or
 * state from before it existed) makes [reclaim] look.
 */
fun needsReclaim(sentinel: Path): Boolean =
    !Files.isRegularFile(sentinel, LinkOption.NOFOLLOW_LINKS) || !Files.isWritable(sentinel)

/**
 * Makes every regular file under [roots] writable by the run about to start, replacing each
 * read-only one with a writable copy of itself that keeps its content and modification time.
 * Symlinks are neither followed nor replaced.
 *
 * Buck2's store may hold a sealed output by hardlink, so a file is replaced, never written in
 * place. Each copy is made in [temporaryDir], which must be on the same file system as the roots
 * and belong to kotlincd alone: the walk skips it, and it is emptied before and removed after, so
 * an interrupted run neither leaves a copy among the outputs nor trips the next run.
 *
 * @param last the file [hasSealedPreviousOutputs] checks, replaced after everything else so that an
 *   interrupted run is detected again by the next one
 */
@Throws(IOException::class)
fun reclaim(roots: List<Path>, sentinel: Path, temporaryDir: Path) {
  val skipped = temporaryDir.toAbsolutePath().normalize()
  val deferred = sentinel.toAbsolutePath().normalize()
  deleteRecursively(temporaryDir)
  Files.createDirectories(temporaryDir)
  try {
    for (root in roots) {
      if (!Files.exists(root, LinkOption.NOFOLLOW_LINKS)) {
        continue
      }
      val candidates =
          Files.walk(root).use { paths ->
            paths
                .filter { !it.toAbsolutePath().normalize().startsWith(skipped) }
                .filter { it.toAbsolutePath().normalize() != deferred }
                .filter { isReadOnlyRegularFile(it) }
                .toList()
          }
      for (file in candidates) {
        // An earlier entry's replacement, or a concurrent cleanup, may have changed this one.
        if (isReadOnlyRegularFile(file)) {
          replaceWithWritableCopy(file, temporaryDir)
        }
      }
    }
    if (isReadOnlyRegularFile(sentinel)) {
      replaceWithWritableCopy(sentinel, temporaryDir)
    } else if (!Files.exists(sentinel, LinkOption.NOFOLLOW_LINKS)) {
      Files.createDirectories(sentinel.parent)
      Files.createFile(sentinel)
    }
  } finally {
    deleteRecursively(temporaryDir)
  }
}

private fun isReadOnlyRegularFile(path: Path): Boolean =
    Files.isRegularFile(path, LinkOption.NOFOLLOW_LINKS) && !Files.isWritable(path)

private fun replaceWithWritableCopy(file: Path, temporaryDir: Path) {
  val modified: FileTime = Files.getLastModifiedTime(file, LinkOption.NOFOLLOW_LINKS)
  val copy = Files.createTempFile(temporaryDir, "copy", ".tmp")
  try {
    Files.copy(
        file,
        copy,
        StandardCopyOption.REPLACE_EXISTING,
        StandardCopyOption.COPY_ATTRIBUTES,
        LinkOption.NOFOLLOW_LINKS,
    )
    if (!copy.toFile().setWritable(true, /* ownerOnly= */ true)) {
      throw IOException("Could not make a writable copy of $file")
    }
    // The attribute copy keeps microseconds only.
    Files.setLastModifiedTime(copy, modified)
    Files.move(copy, file, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
  } finally {
    Files.deleteIfExists(copy)
  }
}

private fun deleteRecursively(directory: Path) {
  if (!Files.exists(directory, LinkOption.NOFOLLOW_LINKS)) return
  Files.walk(directory).use { paths ->
    paths.sorted(Comparator.reverseOrder()).forEach { Files.deleteIfExists(it) }
  }
}
