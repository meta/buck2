/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin

import com.facebook.buck.core.filesystems.RelPath
import com.facebook.buck.jvm.java.ActionMetadata
import java.nio.file.Paths
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class JarsActionMetadataTest {

  @Test
  fun `when constructor called then filters only jar files`() {
    val previousDigest = mutableMapOf(
        Paths.get("lib1.jar") to "digest1",
        Paths.get("lib2.class") to "digest2",
        Paths.get("lib3.txt") to "digest3",
        Paths.get("lib4.jar") to "digest4",
    )
    val currentDigest = mutableMapOf(
        Paths.get("lib1.jar") to "digest1",
        Paths.get("lib2.class") to "digest2",
        Paths.get("lib3.txt") to "digest3",
        Paths.get("lib5.jar") to "digest5",
    )

    val actionMetadata = ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest)
    val jarsActionMetadata = JarsActionMetadata(actionMetadata)

    assertEquals(2, jarsActionMetadata.previousJarsDigest.size)
    assertEquals(2, jarsActionMetadata.currentJarsDigest.size)
    assertTrue(jarsActionMetadata.previousJarsDigest.containsKey(Paths.get("lib1.jar")))
    assertTrue(jarsActionMetadata.previousJarsDigest.containsKey(Paths.get("lib4.jar")))
    assertFalse(jarsActionMetadata.previousJarsDigest.containsKey(Paths.get("lib2.class")))
    assertFalse(jarsActionMetadata.previousJarsDigest.containsKey(Paths.get("lib3.txt")))
    assertTrue(jarsActionMetadata.currentJarsDigest.containsKey(Paths.get("lib1.jar")))
    assertTrue(jarsActionMetadata.currentJarsDigest.containsKey(Paths.get("lib5.jar")))
    assertFalse(jarsActionMetadata.currentJarsDigest.containsKey(Paths.get("lib2.class")))
    assertFalse(jarsActionMetadata.currentJarsDigest.containsKey(Paths.get("lib3.txt")))
  }

  @Test
  fun `when jar file has not changed then hasChanged returns false`() {
    val previousDigest = mutableMapOf(Paths.get("lib1.jar") to "digest1")
    val currentDigest = mutableMapOf(Paths.get("lib1.jar") to "digest1")

    val actionMetadata = ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest)
    val jarsActionMetadata = JarsActionMetadata(actionMetadata)

    assertFalse(jarsActionMetadata.hasChanged(RelPath.of(Paths.get("lib1.jar"))))
  }

  @Test
  fun `when jar file digest changed then hasChanged returns true`() {
    val previousDigest = mutableMapOf(Paths.get("lib1.jar") to "digest1")
    val currentDigest = mutableMapOf(Paths.get("lib1.jar") to "digest1_changed")

    val actionMetadata = ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest)
    val jarsActionMetadata = JarsActionMetadata(actionMetadata)

    assertTrue(jarsActionMetadata.hasChanged(RelPath.of(Paths.get("lib1.jar"))))
  }

  @Test
  fun `when jar file is new then hasChanged returns true`() {
    val previousDigest = mutableMapOf<java.nio.file.Path, String>()
    val currentDigest = mutableMapOf(Paths.get("lib1.jar") to "digest1")

    val actionMetadata = ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest)
    val jarsActionMetadata = JarsActionMetadata(actionMetadata)

    assertTrue(jarsActionMetadata.hasChanged(RelPath.of(Paths.get("lib1.jar"))))
  }

  @Test
  fun `when jar file is removed then hasChanged returns true`() {
    val previousDigest = mutableMapOf(Paths.get("lib1.jar") to "digest1")
    val currentDigest = mutableMapOf<java.nio.file.Path, String>()

    val actionMetadata = ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest)
    val jarsActionMetadata = JarsActionMetadata(actionMetadata)

    assertTrue(jarsActionMetadata.hasChanged(RelPath.of(Paths.get("lib1.jar"))))
  }

  @Test
  fun `when multiple jar files with same digest then hasChanged returns false`() {
    val previousDigest =
        mutableMapOf(Paths.get("lib1.jar") to "digest1", Paths.get("lib2.jar") to "digest2")
    val currentDigest =
        mutableMapOf(Paths.get("lib1.jar") to "digest1", Paths.get("lib2.jar") to "digest2")

    val actionMetadata = ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest)
    val jarsActionMetadata = JarsActionMetadata(actionMetadata)

    assertFalse(jarsActionMetadata.hasChanged(RelPath.of(Paths.get("lib1.jar"))))
    assertFalse(jarsActionMetadata.hasChanged(RelPath.of(Paths.get("lib2.jar"))))
  }

  @Test
  fun `when checking different jar files with different changes then hasChanged returns correct results`() {
    val previousDigest = mutableMapOf(
        Paths.get("lib1.jar") to "digest1",
        Paths.get("lib2.jar") to "digest2",
        Paths.get("lib3.jar") to "digest3",
    )
    val currentDigest = mutableMapOf(
        Paths.get("lib1.jar") to "digest1",
        Paths.get("lib2.jar") to "digest2_changed",
        Paths.get("lib4.jar") to "digest4",
    )

    val actionMetadata = ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest)
    val jarsActionMetadata = JarsActionMetadata(actionMetadata)

    assertFalse(jarsActionMetadata.hasChanged(RelPath.of(Paths.get("lib1.jar"))))
    assertTrue(jarsActionMetadata.hasChanged(RelPath.of(Paths.get("lib2.jar"))))
    assertTrue(jarsActionMetadata.hasChanged(RelPath.of(Paths.get("lib3.jar"))))
    assertTrue(jarsActionMetadata.hasChanged(RelPath.of(Paths.get("lib4.jar"))))
  }

  @Test
  fun `when jar file in subdirectory then filters correctly`() {
    val previousDigest = mutableMapOf(
        Paths.get("path/to/lib1.jar") to "digest1",
        Paths.get("path/to/lib2.class") to "digest2",
    )
    val currentDigest = mutableMapOf(
        Paths.get("path/to/lib1.jar") to "digest1",
        Paths.get("path/to/lib2.class") to "digest2",
    )

    val actionMetadata = ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest)
    val jarsActionMetadata = JarsActionMetadata(actionMetadata)

    assertEquals(1, jarsActionMetadata.previousJarsDigest.size)
    assertEquals(1, jarsActionMetadata.currentJarsDigest.size)
    assertTrue(jarsActionMetadata.previousJarsDigest.containsKey(Paths.get("path/to/lib1.jar")))
    assertFalse(jarsActionMetadata.previousJarsDigest.containsKey(Paths.get("path/to/lib2.class")))
  }

  @Test
  fun `when empty digests then filters return empty maps`() {
    val previousDigest = mutableMapOf<java.nio.file.Path, String>()
    val currentDigest = mutableMapOf<java.nio.file.Path, String>()

    val actionMetadata = ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest)
    val jarsActionMetadata = JarsActionMetadata(actionMetadata)

    assertEquals(0, jarsActionMetadata.previousJarsDigest.size)
    assertEquals(0, jarsActionMetadata.currentJarsDigest.size)
  }

  @Test
  fun `when only non-jar files then filters return empty maps`() {
    val previousDigest = mutableMapOf(
        Paths.get("lib1.class") to "digest1",
        Paths.get("lib2.txt") to "digest2",
        Paths.get("lib3.bin") to "digest3",
    )
    val currentDigest = mutableMapOf(
        Paths.get("lib1.class") to "digest1",
        Paths.get("lib2.txt") to "digest2",
        Paths.get("lib3.bin") to "digest3",
    )

    val actionMetadata = ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest)
    val jarsActionMetadata = JarsActionMetadata(actionMetadata)

    assertEquals(0, jarsActionMetadata.previousJarsDigest.size)
    assertEquals(0, jarsActionMetadata.currentJarsDigest.size)
  }

  @Test(expected = IllegalArgumentException::class)
  fun `when hasChanged called with non-jar file then throws exception`() {
    val previousDigest = mutableMapOf(Paths.get("lib1.jar") to "digest1")
    val currentDigest = mutableMapOf(Paths.get("lib1.jar") to "digest1")

    val actionMetadata = ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest)
    val jarsActionMetadata = JarsActionMetadata(actionMetadata)

    jarsActionMetadata.hasChanged(RelPath.of(Paths.get("lib1.class")))
  }

  @Test(expected = IllegalArgumentException::class)
  fun `when hasChanged called with txt file then throws exception`() {
    val previousDigest = mutableMapOf(Paths.get("lib1.jar") to "digest1")
    val currentDigest = mutableMapOf(Paths.get("lib1.jar") to "digest1")

    val actionMetadata = ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest)
    val jarsActionMetadata = JarsActionMetadata(actionMetadata)

    jarsActionMetadata.hasChanged(RelPath.of(Paths.get("file.txt")))
  }

  @Test
  fun `when jar file with uppercase extension then it is not treated as a jar`() {
    val previousDigest = mutableMapOf(Paths.get("lib1.JAR") to "digest1")
    val currentDigest = mutableMapOf(Paths.get("lib1.JAR") to "digest1")

    val actionMetadata = ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest)
    val jarsActionMetadata = JarsActionMetadata(actionMetadata)

    assertEquals(0, jarsActionMetadata.previousJarsDigest.size)
    assertEquals(0, jarsActionMetadata.currentJarsDigest.size)
  }
}
