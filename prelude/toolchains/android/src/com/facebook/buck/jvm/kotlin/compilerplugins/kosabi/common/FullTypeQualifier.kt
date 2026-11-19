/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.kosabi.common

/**
 * [FullTypeQualifier] is a data structure for full qualified name. It interprets package, names and
 * member from segments.
 *
 * There are several import conditions in Java/Kotlin other than import package name
 * 1. Nested class/interface - com.facebook.TopLevelClass.InnerClass.InnerClass2
 * 2. Top level declaration - com.facebook.kotlin.someMethod
 * 3. Static member - com.facebook.SomeClass.CONSTANT_VALUE
 *
 * @param segments ordered list from Fqname, ex: listOf("com", "facebook", "kotlin", "SomeName")
 */
class FullTypeQualifier : Comparable<FullTypeQualifier> {
  val segments: List<String>

  /** Package qualified name, ex: com.facebook.kotlin */
  val pkg: List<String>

  /** List of package name or inner classes/interface/enum class */
  val names: List<String>

  /** method or constant value property */
  val member: String?

  constructor(segments: List<String>) {
    this.segments = segments
    val firstUpperCaseIndex = segments.indexOfFirst { it.first().isUpperCase() }
    if (firstUpperCaseIndex == -1) {
      names = emptyList()
      pkg = segments.dropLast(1)
      member = segments.last()
    } else {
      pkg = segments.take(firstUpperCaseIndex)
      val lastSegment = segments.last()
      if (NonConventionalImports.classLikeDeclarations.contains(segments)) {
        names = listOf(lastSegment)
        member = null
      } else if (lastSegment.isStaticConstQualifier() || lastSegment.isStaticMethodQualifier()) {
        names = segments.subList(firstUpperCaseIndex, segments.lastIndex)
        member = lastSegment
      } else {
        names = segments.drop(firstUpperCaseIndex)
        member = null
      }
    }
    if (names.isEmpty() && member == null) {
      Logger.log(
          "Topic: [FullTypeQualifier] Invalid segments: $segments caused empty names and null member in FullTypeQualifier",
      )
    }
  }

  private constructor(
      segments: List<String>,
      pkg: List<String> = emptyList(),
      names: List<String> = emptyList(),
      member: String? = null,
  ) {
    this.segments = segments
    this.pkg = pkg
    this.names = names
    this.member = member
  }

  fun pkgAsString(): String = pkg.joinToString(separator = ".")

  // TopLevelDeclaration:
  // - com.some.package.method
  fun isTopLevelDeclaration(): Boolean = names.isEmpty()

  /**
   * Call only with segments from a type slot: there the trailing segment names a type by position,
   * whatever its case.
   */
  fun withMemberAsNestedName(): FullTypeQualifier =
      if (member == null) this else unsafeBuildQualifier(segments, pkg, names + member)

  override fun compareTo(other: FullTypeQualifier): Int {
    val thisSize = this.segments.size
    val otherSize = other.segments.size
    // Compare list sizes first
    if (thisSize < otherSize) return -1
    if (thisSize > otherSize) return 1
    for (i in 0 until thisSize) {
      val comparison = this.segments[i].compareTo(other.segments[i])
      if (comparison != 0) return comparison
    }
    return 0
  }

  override fun equals(other: Any?): Boolean {
    if (this === other) return true
    if (javaClass != other?.javaClass) return false

    other as FullTypeQualifier

    if (segments != other.segments) return false

    return true
  }

  override fun hashCode(): Int {
    return segments.hashCode()
  }

  companion object {
    fun unsafeBuildQualifier(
        segments: List<String>,
        pkg: List<String> = emptyList(),
        names: List<String> = emptyList(),
        member: String? = null,
    ): FullTypeQualifier = FullTypeQualifier(segments, pkg, names, member)
  }
}

/**
 * Filter out same outer class full qualifier from set [qualifiersToFilter]
 *
 * @param qualifiersToFilter set of [FullTypeQualifier] to filter from current set
 */
fun Collection<FullTypeQualifier>.filterDifferentOuterClassIn(
    qualifiersToFilter: Collection<FullTypeQualifier>,
): List<FullTypeQualifier> = filterNot { qualifier ->
  qualifiersToFilter.contains(qualifier.outerClassOnlyQualifier())
}

/**
 * [FullTypeQualifier.outerClassOnlyQualifier] will return outerClass only [FullTypeQualifier]
 * Example: com.facebook.kotlin.SampleClass.method -> com.facebook.kotlin.SampleClass
 */
fun FullTypeQualifier.outerClassOnlyQualifier(): FullTypeQualifier? =
    if (names.isEmpty()) null
    else FullTypeQualifier.unsafeBuildQualifier(pkg + names.first(), pkg, listOf(names.first()))

// Regex matches:
//  - non-empty upper screaming case
// Examples:
// "A"     - NO
// "A_"    - YES
// "AB"    - YES
// "A_9"   - YES
// "A___C" - YES
// private val SCREAMING_UPPER_SNAKE_CASE = "[A-Z][_A-Z0-9]+(_[A-Z0-9]+)*".toRegex()

private fun String.isStaticMethodQualifier() = first().isLowerCase()

private fun String.isStaticConstQualifier(): Boolean {
  if (length <= 1) return false
  return this.all { it == it.uppercaseChar() }
}
