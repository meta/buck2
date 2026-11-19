/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util

// Kotlin type alias that's not from java.lang
// https://github.com/JetBrains/kotlin/blob/master/libraries/stdlib/jvm/runtime/kotlin/TypeAliases.kt
object PlainKTJavaTypeAlias : FTQCollection {
  /**
   * Returns all FullTypeQualifier objects in this collection.
   *
   * @return A collection of all FullTypeQualifier vals defined in this class
   */
  override fun all(): Collection<String> {
    return listOf(
        "NoSuchElementException",
        "ConcurrentModificationException",
        "Comparator",
        "RandomAccess",
        "ArrayList",
        "LinkedHashMap",
        "HashMap",
        "LinkedHashSet",
        "HashSet",
    )
  }
}
