/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.cd.workertool.postexecutors

import com.facebook.buck.testutil.TemporaryPaths
import java.nio.file.Files
import java.nio.file.Path
import kotlin.streams.toList
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

internal class PreviousOutputsTest {

  @JvmField @Rule val temporaryFolder: TemporaryPaths = TemporaryPaths(false)

  @Test
  fun `read-only outputs become writable and keep their content`() {
    val root = temporaryFolder.newFolder("incremental_state").path
    val output = writeReadOnly(root.resolve("caches/lookups.tab"), "previous")

    reclaimAll(root)

    assertTrue(Files.isWritable(output))
    assertEquals("previous", Files.readString(output))
    Files.writeString(output, "next")
    assertEquals("next", Files.readString(output))
  }

  @Test
  fun `a sealed hardlinked output is replaced and its peer is left untouched`() {
    val root = temporaryFolder.newFolder("jar").path
    val output = writeReadOnly(root.resolve("lib.jar"), "previous")
    val peer = temporaryFolder.newFolder("store").path.resolve("blob")
    Files.createLink(peer, output)
    val inode = Files.getAttribute(peer, "unix:ino")
    val modified = Files.getLastModifiedTime(output)
    assertEquals(2, Files.getAttribute(peer, "unix:nlink"))

    reclaimAll(root)

    assertTrue(Files.isWritable(output))
    assertEquals("previous", Files.readString(output))
    assertEquals(modified, Files.getLastModifiedTime(output))
    assertNotEquals(inode, Files.getAttribute(output, "unix:ino"))
    // The peer is the original inode: same content and mode. Its link count drops by one, the
    // output's name, which now names the copy.
    assertEquals(inode, Files.getAttribute(peer, "unix:ino"))
    assertEquals("previous", Files.readString(peer))
    assertFalse(Files.isWritable(peer))
    assertEquals(1, Files.getAttribute(peer, "unix:nlink"))
    Files.writeString(output, "next")
    assertEquals("previous", Files.readString(peer))
  }

  @Test
  fun `a symlink out of the output tree is not followed`() {
    val root = temporaryFolder.newFolder("incremental_state").path
    val outside = writeReadOnly(temporaryFolder.newFolder("outside").path.resolve("blob"), "keep")
    val link = Files.createSymbolicLink(root.resolve("link"), outside)
    val inode = Files.getAttribute(outside, "unix:ino")

    reclaimAll(root)

    assertTrue(Files.isSymbolicLink(link))
    assertEquals(inode, Files.getAttribute(outside, "unix:ino"))
    assertFalse(Files.isWritable(outside))
  }

  @Test
  fun `a writable tree and missing roots are left alone`() {
    val root = temporaryFolder.newFolder("outputs").path
    val writable = root.resolve("dep-file.txt")
    Files.writeString(writable, "deps")
    val inode = Files.getAttribute(writable, "unix:ino")

    reclaimAll(root, root.resolve("missing"))

    assertEquals(inode, Files.getAttribute(writable, "unix:ino"))
  }

  @Test
  fun `outputs named like a temporary copy are left alone`() {
    val root = temporaryFolder.newFolder("jar").path
    val output = writeReadOnly(root.resolve("a"), "original")
    val neighbour = root.resolve("a.writable")
    Files.writeString(neighbour, "neighbour")
    val prefixed = root.resolve(".kotlincd-reclaim-user-output.txt")
    Files.writeString(prefixed, "user")

    reclaimAll(root)

    assertEquals("original", Files.readString(output))
    assertEquals("neighbour", Files.readString(neighbour))
    assertEquals("user", Files.readString(prefixed))
  }

  @Test
  fun `copies left by an interrupted run are removed and do not fail the next`() {
    val stateDir = temporaryFolder.newFolder("incremental_state").path
    val output = writeReadOnly(stateDir.resolve("a"), "original")
    // An interrupted run's copy: attributes copied, not yet writable.
    val stale = writeReadOnly(stateDir.resolve("$RECLAIM_DIRECTORY/copy123.tmp"), "original")

    reclaim(
        listOf(stateDir),
        stateDir.resolve(RECLAIM_SENTINEL),
        stateDir.resolve(RECLAIM_DIRECTORY),
    )

    assertTrue(Files.isWritable(output))
    assertEquals("original", Files.readString(output))
    assertFalse(Files.exists(stale))
    Files.list(stateDir).use { files ->
      assertEquals(
          listOf("a", RECLAIM_SENTINEL),
          files.map { it.fileName.toString() }.sorted().toList(),
      )
    }
  }

  @Test
  fun `the sentinel is replaced last, and created when missing`() {
    val root = temporaryFolder.newFolder("incremental_state").path
    val sentinel = writeReadOnly(root.resolve(RECLAIM_SENTINEL), "")
    val cache = writeReadOnly(root.resolve("caches/lookups.tab"), "cache")
    assertTrue(needsReclaim(sentinel))

    reclaim(listOf(root), sentinel, temporaryDir())

    assertTrue(Files.isWritable(sentinel))
    assertTrue(Files.isWritable(cache))
    assertFalse(needsReclaim(sentinel))

    Files.delete(sentinel)
    reclaim(listOf(root), sentinel, temporaryDir())
    assertTrue(Files.isRegularFile(sentinel))
    assertFalse(needsReclaim(sentinel))
  }

  @Test
  fun `only a writable sentinel skips reclaiming`() {
    val root = temporaryFolder.newFolder("incremental_state").path
    val sentinel = root.resolve(RECLAIM_SENTINEL)
    assertTrue(needsReclaim(sentinel))
    Files.writeString(sentinel, "")
    assertFalse(needsReclaim(sentinel))
    sentinel.toFile().setWritable(false, false)
    assertTrue(needsReclaim(sentinel))
  }

  private fun temporaryDir(): Path = temporaryFolder.root.path.resolve(RECLAIM_DIRECTORY)

  private fun reclaimAll(vararg roots: Path) {
    reclaim(
        roots.toList(),
        temporaryFolder.root.path.resolve("absent_metadata.json"),
        temporaryDir(),
    )
    assertFalse(Files.exists(temporaryDir()))
  }

  private fun writeReadOnly(path: Path, content: String): Path {
    Files.createDirectories(path.parent)
    Files.writeString(path, content)
    path.toFile().setWritable(false, false)
    return path
  }
}
