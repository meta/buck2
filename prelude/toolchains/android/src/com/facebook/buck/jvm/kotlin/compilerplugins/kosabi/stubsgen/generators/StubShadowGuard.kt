/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.kosabi.stubsgen.generators

import com.facebook.kotlin.compilerplugins.kosabi.common.Logger
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.KotlinClasspathDeclarations

/**
 * Removes the class stubs that read a classpath class as a package, and refuses the module where
 * that removal would not leave the classpath class in their place.
 *
 * The generators split a written name at its first capitalized segment (`FullTypeQualifier`), and
 * their classpath index skips classes whose name starts lower-case, so a class nested in a
 * lower-case outer class (`a/b/proto$Event`, which protoc writes for a lower-case
 * `java_outer_classname`) is invisible to them, and `import a.b.proto.Event` gets the stub
 * `a/b/proto/Event`. That stub makes `a.b.proto` a package, and kotlinc binds a name to the longest
 * existing package prefix of it (`findLongestExistingPackage`, ImportUtils.kt:24-41 at
 * kotlin-2.4.21), so the name resolves to the stub even when the classpath has `a/b/proto$Event`.
 *
 * A stub's readings are the classpath classes with its qualified name, `/` and `$` both read as
 * name separators. The guard acts only on a stub that a classpath class names with a shorter
 * package: a reading whose package is a strict prefix of the stub's package. Every other stub stays
 * as the generators wrote it, a file facade in a package that is a classpath class included. For
 * such a stub:
 * - with one reading, the stub is dropped, so kotlinc binds the classpath class;
 * - with more readings (its own binary name included), the written name is ambiguous: refuse;
 * - if a prefix of the written name longer than the reading's package is a package of the
 *   classpath, of the module's sources or of a kept stub, kotlinc reads that package first and
 *   misses the class: refuse;
 * - each nested stub must have exactly one reading nested in the classpath class: else refuse.
 *
 * Decisions run over the root stubs in order, each followed by its nested stubs; the checks of the
 * dropped stubs run after all decisions, in the same order. `rokoko-stubgen` `shadow_guard.rs`
 * follows the same rules in the same order.
 */
class StubShadowGuard(
    private val declarations: KotlinClasspathDeclarations,
    sourcePackages: Collection<List<String>>,
) {
  private val sourcePackages: Set<String> =
      sourcePackages.flatMapTo(mutableSetOf()) { segments -> prefixes(segments) }

  private class Drop(val stub: KStub, val parent: KStub?, val binaryName: String, val real: String)

  fun filter(stubs: Collection<KStub>): List<KStub> {
    val drops = mutableListOf<Drop>()
    val keptPackages = mutableSetOf<String>()
    for (stub in stubs) {
      val pkg = packageOf(stub)
      if (stub.treatAsReal || !decide(stub, null, rootBinaryName(stub), pkg, drops)) {
        keptPackages += prefixes(pkg)
      }
    }
    for (drop in drops) {
      Logger.log(
          "  [Shadowing stub dropped] ${drop.binaryName}: the classpath declares ${drop.real}",
      )
      checkPackages(drop, keptPackages)
      drop.stub.innerStubs.forEach { inner -> checkNestedInReal(inner, drop.real) }
    }
    drops.forEach { drop -> drop.parent?.innerStubs?.remove(drop.stub) }
    val droppedRoots = drops.filter { it.parent == null }.mapTo(mutableSetOf()) { it.stub }
    return stubs.filterNot { it in droppedRoots }
  }

  /** Records a drop for [stub] when the guard acts on it; true when it does. */
  private fun decide(
      stub: KStub,
      parent: KStub?,
      binaryName: String,
      pkg: List<String>,
      drops: MutableList<Drop>,
  ): Boolean {
    val qualifiedName = qualifiedName(binaryName)
    val readings = declarations.binaryNamesOf(qualifiedName)
    val acting = readings.filter { isStrictPrefix(packageOf(it), pkg) }
    if (acting.isEmpty()) {
      stub.innerStubs.toList().forEach { inner ->
        decide(inner, stub, "$binaryName\$${inner.name}", pkg, drops)
      }
      return false
    }
    if (readings.size > 1) {
      refuse(
          "the classpath declares ${quoted(readings)} with the qualified name `$qualifiedName` of " +
              "the stub `$binaryName`; the written name is ambiguous",
      )
    }
    drops += Drop(stub, parent, binaryName, acting.single())
    return true
  }

  private fun checkPackages(drop: Drop, keptPackages: Set<String>) {
    val segments = qualifiedName(drop.binaryName).split('.')
    for (size in packageOf(drop.real).size + 1 until segments.size) {
      val pkg = segments.take(size).joinToString(".")
      if (
          declarations.hasPackage(pkg.replace('.', '/')) ||
              pkg in sourcePackages ||
              pkg in keptPackages
      ) {
        refuse(
            "the written name `${segments.joinToString(".")}` is the classpath class " +
                "`${drop.real}`, and `$pkg` is a package, which kotlinc reads first; the written " +
                "name is ambiguous",
        )
      }
    }
  }

  private fun checkNestedInReal(stub: KStub, outer: String) {
    val nested = "$outer\$${stub.name}"
    val qualifiedName = qualifiedName(nested)
    val readings = declarations.binaryNamesOf(qualifiedName)
    if (readings.size > 1) {
      refuse(
          "the classpath declares ${quoted(readings)} with the qualified name `$qualifiedName`, " +
              "a class nested in the classpath class `$outer`; the written name is ambiguous",
      )
    }
    val real =
        readings.singleOrNull()
            ?: refuse(
                "the sources name `$qualifiedName`, a class nested in the classpath class " +
                    "`$outer`, and the classpath does not declare `$nested`; add the target " +
                    "that provides it to source_only_abi_deps",
            )
    stub.innerStubs.forEach { inner -> checkNestedInReal(inner, real) }
  }

  private fun refuse(reason: String): Nothing {
    val message = "Kosabi stubsgen refuses to write a stub that shadows a classpath class: $reason"
    Logger.log("  [Error] $message")
    throw StubShadowException(message)
  }

  private companion object {
    fun packageOf(stub: KStub): List<String> =
        stub.pkg?.split('.')?.filter { it.isNotEmpty() }.orEmpty()

    fun packageOf(binaryName: String): List<String> =
        binaryName.substringBeforeLast('/', "").split('/').filter { it.isNotEmpty() }

    fun rootBinaryName(stub: KStub): String = (packageOf(stub) + stub.name).joinToString("/")

    fun qualifiedName(binaryName: String): String = binaryName.replace('/', '.').replace('$', '.')

    fun isStrictPrefix(prefix: List<String>, of: List<String>): Boolean =
        prefix.size < of.size && of.subList(0, prefix.size) == prefix

    fun prefixes(segments: List<String>): List<String> =
        (1..segments.size).map { segments.take(it).joinToString(".") }

    fun quoted(names: Collection<String>): String = names.sorted().joinToString(" and ") { "`$it`" }
  }
}

class StubShadowException(message: String) : IllegalStateException(message)
