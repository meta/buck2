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

import java.io.File
import java.io.IOException
import java.util.Enumeration
import java.util.jar.JarEntry
import java.util.jar.JarFile

/**
 * returns a Pair of [1] Set<FullTypeQualifier>: set of outerClass FTQs in classpaths, [2]
 * Set<List<String>>: set of strings of package segments in classpath. [1] used to determine the
 * outerClass FTQ from class name so a/b/c.class -> a.b.C [2] used for ClassLevelFunction, these
 * functions will exist in class [package]/[FileName]Kt.class. Since [FileName] is not known at
 * function check, we are assuming same [package] will be in same classpath when checking
 */
@Suppress("detekt.SwallowedException")
fun parseClasspathFileClassesAndPackages(
    classpath: List<File>,
): Pair<Set<FullTypeQualifier>, Set<List<String>>> {
  val outerFTQs = mutableSetOf<FullTypeQualifier>()
  val pkgs = mutableSetOf<List<String>>()
  for (file in classpath) {
    if (shouldSkipLookup(file.path)) {
      continue
    }
    try {
      JarFile(file).use { jarFile ->
        val je: Enumeration<JarEntry> = jarFile.entries()
        je.iterator().forEach { jarEntry ->
          if (jarEntry.name.endsWith(".class")) {
            val qualifier = FullTypeQualifier(getSegmentsFromJarEntry(jarEntry))
            pkgs.add(qualifier.pkg)
            val outerQualifier = qualifier.outerClassOnlyQualifier()
            if (outerQualifier != null) {
              outerFTQs.add(qualifier)
            }
          }
        }
      }
    } catch (e: IOException) {
      Logger.log("[Possible error]: failed to open jar file: ${file.canonicalPath}")
    }
  }
  return Pair(outerFTQs, pkgs)
}

private fun getSegmentsFromJarEntry(jarEntry: JarEntry): List<String> {
  return jarEntry.name.replace(".class", "").split("/")
}

internal fun shouldSkipLookup(path: String): Boolean {
  // skip lookup in large kotlin/java libraries that doesn't have known CONST val exports used in
  // our codebase
  if (
      // Filtering out at least guava, failaccess, listeneablefuture
      path.contains("/java/com/google/guava/") ||
          // Filtering out at least stdlib, annotations, parcelize
          path.contains("/org/jetbrains/kotlin/") ||
          path.contains("/org/jetbrains/kotlinx/kotlinx-coroutines-android/") ||
          path.contains("/org/jetbrains/kotlinx/kotlinx-coroutines-core-jvm/") ||
          path.contains("/third-party/java/kotlin/")
  ) {
    return true
  }

  return false
}
