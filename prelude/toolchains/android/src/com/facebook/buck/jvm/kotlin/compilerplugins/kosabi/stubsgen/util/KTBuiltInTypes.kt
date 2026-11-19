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

// created by parsing types contained in kotlin/core/builtins
// https://github.com/JetBrains/kotlin/tree/master/core/builtins
object PlainKTBuiltInTypes : FTQCollection {
  /**
   * Returns the simple names of all types in this collection.
   *
   * @return A collection of simple type names defined in this class
   */
  override fun all(): Collection<String> {
    return listOf(
        "Comparable",
        "CharSequence",
        "Iterator",
        "MutableIterator",
        "ListIterator",
        "MutableListIterator",
        "Iterable",
        "MutableIterable",
        "Collection",
        "MutableCollection",
        "List",
        "MutableList",
        "Set",
        "MutableSet",
        "Map",
        "MutableMap",
        "String",
        "Enum",
        "Nothing",
        "Array",
        "Any",
        "Throwable",
        "ByteArray",
        "CharArray",
        "ShortArray",
        "IntArray",
        "LongArray",
        "FloatArray",
        "DoubleArray",
        "BooleanArray",
        "Annotation",
        "Boolean",
        "Byte",
        "Short",
        "Int",
        "Long",
        "Float",
        "Double",
        "Number",
        "Char",
        "Unit",
        "Function",
        "AnnotationTarget",
        "AnnotationRetention",
        "Target",
        "Retention",
        "Repeatable",
        "MustBeDocumented",
        "Deprecated",
        "DeprecatedSinceKotlin",
        "ReplaceWith",
        "DeprecationLevel",
        "ExtensionFunctionType",
        "ContextFunctionTypeParams",
        "ParameterName",
        "Suppress",
        "UnsafeVariance",
        "SinceKotlin",
        "DslMarker",
        "PublishedApi",
    )
  }
}
