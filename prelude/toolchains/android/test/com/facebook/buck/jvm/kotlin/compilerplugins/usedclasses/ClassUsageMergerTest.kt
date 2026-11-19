/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.compilerplugins.usedclasses

import java.nio.file.Path
import java.nio.file.Paths
import java.util.HashMap
import org.junit.Assert.assertEquals
import org.junit.Test

internal class ClassUsageMergerTest {

  @Test
  fun `when previous class usage is empty then current class usage is returned`() {
    val prevClassUsageMap: Map<Path, Set<Path>> = HashMap()
    val currentClassUsageMap: Map<Path, Set<Path>> = mapOf(Paths.get("A.jar") to HashSet())

    val mergedMap: Map<Path, Set<Path>> =
        mergeClassUsageMaps(prevClassUsageMap, currentClassUsageMap)

    assertEquals(currentClassUsageMap, mergedMap)
  }

  @Test
  fun `when current class usage is empty then previous class usage is returned`() {
    val prevClassUsageMap: Map<Path, Set<Path>> = mapOf(Paths.get("A.jar") to HashSet())
    val currentClassUsageMap: Map<Path, Set<Path>> = HashMap()

    val mergedMap: Map<Path, Set<Path>> =
        mergeClassUsageMaps(prevClassUsageMap, currentClassUsageMap)

    assertEquals(prevClassUsageMap, mergedMap)
  }

  @Test
  fun `when outer addition is made then new class usage is included in merged map`() {
    val prevClassUsageMap: Map<Path, Set<Path>> = classUsageMapBase

    val innerMap: MutableSet<Path> = mutableSetOf(Paths.get("C.class"))
    val currentClassUsageMap = classUsageMapBase.apply {
      toMutableMap()[Paths.get("new-class-abi.jar")] = innerMap
    }

    val mergedMap: Map<Path, Set<Path>> =
        mergeClassUsageMaps(prevClassUsageMap, currentClassUsageMap)

    assertEquals(currentClassUsageMap, mergedMap)
  }

  @Test
  fun `when inner addition is made then new class is included in merged inner map`() {
    val prevClassUsageMap: Map<Path, Set<Path>> = classUsageMapBase
    val currentClassUsageMap: Map<Path, Set<Path>> = classUsageMapBase.apply {
      toMutableMap()[Paths.get("class-abi.jar")] = setOf(Paths.get("C.class"))
    }

    val mergedMap: Map<Path, Set<Path>> =
        mergeClassUsageMaps(prevClassUsageMap, currentClassUsageMap)

    assertEquals(currentClassUsageMap, mergedMap)
  }

  @Test
  fun `when outer deletion is made then deleted class usage is still included in merged map`() {
    val prevClassUsageMap: Map<Path, Set<Path>> = classUsageMapBase
    val currentClassUsageMap: Map<Path, Set<Path>> = classUsageMapBase.apply {
      toMutableMap().remove(Paths.get("class-abi.jar"))
    }

    val mergedMap: Map<Path, Set<Path>> =
        mergeClassUsageMaps(prevClassUsageMap, currentClassUsageMap)

    assertEquals(prevClassUsageMap, mergedMap)
  }

  @Test
  fun `when inner deletion is made then deleted class is still included in merged inner map`() {
    val prevClassUsageMap: Map<Path, Set<Path>> = classUsageMapBase
    val currentClassUsageMap: Map<Path, Set<Path>> =
        classUsageMapBase.toMutableMap().apply {
          getValue(Paths.get("symlink_kotlin-stdlib-2.0.0.jar-class-abi.jar"))
              .toMutableSet()
              .remove(Paths.get("kotlin/Unit.class"))
        }

    val mergedMap: Map<Path, Set<Path>> =
        mergeClassUsageMaps(prevClassUsageMap, currentClassUsageMap)

    assertEquals(prevClassUsageMap, mergedMap)
  }

  @Test
  fun `when no changes are made then merged map is equal to both previous and current class usage maps`() {
    val prevClassUsageMap: Map<Path, Set<Path>> = classUsageMapBase
    val currentClassUsageMap: Map<Path, Set<Path>> = classUsageMapBase

    val mergedMap: Map<Path, Set<Path>> =
        mergeClassUsageMaps(prevClassUsageMap, currentClassUsageMap)

    assertEquals(prevClassUsageMap, mergedMap)
    assertEquals(currentClassUsageMap, mergedMap)
  }

  private val classUsageMapBase: Map<Path, Set<Path>>
    get() {
      val outerMap: MutableMap<Path, MutableSet<Path>> = HashMap()
      val innerSet: MutableSet<Path> = mutableSetOf(
          Paths.get("kotlin/Unit.class"),
          Paths.get("kotlin/io/ConsoleKt.class"),
          Paths.get("kotlin/internal/InlineOnly.class"),
          Paths.get("kotlin/annotation/Target.class"),
          Paths.get("kotlin/annotation/MustBeDocumented.class"),
          Paths.get("kotlin/annotation/Retention.class"),
      )

      outerMap[Paths.get("symlink_kotlin-stdlib-2.0.0.jar-class-abi.jar")] = innerSet

      val innerSet2: MutableSet<Path> = mutableSetOf(
          Paths.get("java/lang/Object.class"),
          Paths.get("java/lang/annotation/Annotation.class"),
      )
      outerMap[Paths.get("android.jar")] = innerSet2

      val innerSet3: MutableSet<Path> = mutableSetOf(Paths.get("D.class"))
      outerMap[Paths.get("class-abi.jar")] = innerSet3

      return outerMap
    }
}
