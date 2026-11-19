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
import java.util.concurrent.ConcurrentHashMap
import java.util.jar.JarFile

/**
 * The simple names Kosabi stub generation counts as default imports from `java.lang`.
 *
 * Stub generation fabricates a same-package stub for every unimported name it does not know.
 * kotlinc also resolves the `java.lang` classes of the JDK it compiles against, which stub
 * generation never reads; the applicability checks use [knownNames] to find the names stub
 * generation would get wrong, so both read the same set.
 */
object JavaLangDefaultImports {
  private const val JAVA_LANG = "java.lang"
  private const val JAVA_LANG_PATH = "java/lang/"

  // https://docs.oracle.com/javase/8/docs/api/java/lang/package-summary.html
  val STATIC_NAMES: List<String> = listOf(
      "Appendable",
      "AutoCloseable",
      "CharSequence",
      "Cloneable",
      "Comparable",
      "Iterable",
      "Readable",
      "Runnable",
      "Boolean",
      "Byte",
      "Character",
      "Class",
      "ClassLoader",
      "ClassValue",
      "Compiler",
      "Double",
      "Enum",
      "Float",
      "InheritableThreadLocal",
      "Integer",
      "Long",
      "Math",
      "Number",
      "Object",
      "Package",
      "Process",
      "ProcessBuilder",
      "Runtime",
      "RuntimePermission",
      "SecurityManager",
      "Short",
      "StackTraceElement",
      "StrictMath",
      "String",
      "StringBuffer",
      "StringBuilder",
      "System",
      "Thread",
      "ThreadGroup",
      "ThreadLocal",
      "Throwable",
      "Void",
      "Deprecated",
      "FunctionalInterface",
      "Override",
      "SafeVarargs",
      "SuppressWarnings",
      "ArithmeticException",
      "ArrayIndexOutOfBoundsException",
      "ArrayStoreException",
      "ClassCastException",
      "ClassNotFoundException",
      "CloneNotSupportedException",
      "EnumConstantNotPresentException",
      "Exception",
      "IllegalAccessException",
      "IllegalArgumentException",
      "IllegalMonitorStateException",
      "IllegalStateException",
      "IllegalThreadStateException",
      "IndexOutOfBoundsException",
      "InstantiationException",
      "InterruptedException",
      "NegativeArraySizeException",
      "NoSuchFieldException",
      "NoSuchMethodException",
      "NullPointerException",
      "NumberFormatException",
      "ReflectiveOperationException",
      "RuntimeException",
      "SecurityException",
      "StringIndexOutOfBoundsException",
      "TypeNotPresentException",
      "UnsupportedOperationException",
      "AbstractMethodError",
      "AssertionError",
      "BootstrapMethodError",
      "ClassCircularityError",
      "ClassFormatError",
      "Error",
      "ExceptionInInitializerError",
      "IllegalAccessError",
      "IncompatibleClassChangeError",
      "InstantiationError",
      "InternalError",
      "LinkageError",
      "NoClassDefFoundError",
      "NoSuchFieldError",
      "NoSuchMethodError",
      "OutOfMemoryError",
      "StackOverflowError",
      "ThreadDeath",
      "UnknownError",
      "UnsatisfiedLinkError",
      "UnsupportedClassVersionError",
      "VerifyError",
      "VirtualMachineError",
  )

  private val classpathTypesByJar = ConcurrentHashMap<JarKey, Set<FullTypeQualifier>>()

  private data class JarKey(val path: String, val length: Long, val lastModified: Long)

  /**
   * Whether stub generation counts [qualifier], a classpath type, as a default import. Only
   * `java.lang` itself is default-imported: `java.lang.reflect`, `.annotation`, `.ref` and
   * `.invoke` need an import like any other package.
   */
  fun isAutoImported(qualifier: FullTypeQualifier): Boolean = qualifier.pkgAsString() == JAVA_LANG

  /**
   * The names stub generation knows as default imports, given the types it read from its classpath:
   * [STATIC_NAMES] plus every segment and name of the classpath's [isAutoImported] types.
   */
  fun knownNames(classpathTypes: Collection<FullTypeQualifier>): Set<String> =
      STATIC_NAMES.toSet() +
          classpathTypes.filter(::isAutoImported).flatMap { it.segments + it.names }

  /**
   * The [isAutoImported] types of [classpath], read as [parseClasspathFileClassesAndPackages] reads
   * them for stub generation. Each jar is read once per process.
   */
  fun classpathTypes(classpath: List<File>): Set<FullTypeQualifier> =
      classpath
          .filterNot { shouldSkipLookup(it.path) }
          .flatMap { jar ->
            val key = JarKey(jar.path, jar.length(), jar.lastModified())
            classpathTypesByJar.getOrPut(key) { readJavaLangTypes(jar) }
          }
          .toSet()

  /**
   * Whether [jar] provides `java.lang.Object`: a platform bootclasspath root (android.jar, or the
   * JDK stub of a JVM target), which every compile of a target sees, the source-only ABI compile
   * included.
   */
  fun isPlatformRoot(jar: File): Boolean =
      classpathTypes(listOf(jar)).any { it.segments == JAVA_LANG_OBJECT }

  private val JAVA_LANG_OBJECT = listOf("java", "lang", "Object")

  @Suppress("detekt.SwallowedException")
  private fun readJavaLangTypes(jar: File): Set<FullTypeQualifier> =
      try {
        JarFile(jar).use { jarFile ->
          jarFile
              .entries()
              .asSequence()
              .filter { it.name.startsWith(JAVA_LANG_PATH) && it.name.endsWith(".class") }
              .map { FullTypeQualifier(it.name.removeSuffix(".class").split("/")) }
              .filter { it.outerClassOnlyQualifier() != null && isAutoImported(it) }
              .toSet()
        }
      } catch (e: IOException) {
        emptySet()
      }
}
