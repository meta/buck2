/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:SuppressWarnings("PackageLocationMismatch")
@file:Suppress("OPT_IN_USAGE_ERROR")
@file:OptIn(
    com.facebook.DeprecatedForRemovalCompilerApiCompat::class,
)

package com.facebook

import com.facebook.kotlin.compilercompat.compatValueParameters
import org.jetbrains.kotlin.GeneratedDeclarationKey
import org.jetbrains.kotlin.descriptors.DescriptorVisibilities
import org.jetbrains.kotlin.descriptors.Modality
import org.jetbrains.kotlin.fir.types.ConeKotlinType
import org.jetbrains.kotlin.ir.IrElement
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrDeclarationWithVisibility
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.declarations.IrSimpleFunction
import org.jetbrains.kotlin.ir.types.classFqName
import org.jetbrains.kotlin.ir.util.kotlinFqName
import org.jetbrains.kotlin.name.Name

data object JvmAbiGenPlugin : GeneratedDeclarationKey()

data class InterfaceMethodInfo(
    val name: Name,
    val returnType: ConeKotlinType,
    val valueParameters: List<Pair<Name, ConeKotlinType>>,
    val modality: Modality,
)

// A kept (non-dropped) sealed class's metadata names every subclass in
// `sealedSubclasses`, so dropping a private subclass while keeping the parent lets
// consumers compile a non-exhaustive `when` that throws NoWhenBranchMatchedException
// at runtime. Collect the fq names of every class so named, transitively, plus any
// private enclosing classes (a subclass nested in a file-private class needs its
// outers kept as stubs). Only classes that would otherwise be dropped need listing,
// but recording the full set is harmless.
internal fun collectSealedSubclassKeepNames(moduleFragment: IrModuleFragment): Set<String> {
  val keep = mutableSetOf<String>()
  val worklist = ArrayDeque<IrClass>()
  moduleFragment.files.forEach { file ->
    file.acceptChildren(
        object : IrElementVisitorVoidCompat() {
          override fun visitElement(element: IrElement) {
            if (element is IrClass) worklist.add(element)
            element.acceptChildren(this, null)
          }
        },
        null,
    )
  }
  val visited = mutableSetOf<String>()
  while (worklist.isNotEmpty()) {
    val sealedClass = worklist.removeFirst()
    if (
        sealedClass.modality != Modality.SEALED ||
            (sealedClass.isDroppedVisibility() && sealedClass.kotlinFqName.asString() !in keep)
    ) {
      continue
    }
    for (subSymbol in sealedClass.sealedSubclasses) {
      val sub = subSymbol.owner
      val fqName = sub.kotlinFqName.asString()
      if (!visited.add(fqName)) continue
      keep.add(fqName)
      // A nested subclass needs its enclosing classes kept as stubs too.
      var parent = sub.parent
      while (parent is IrClass) {
        keep.add(parent.kotlinFqName.asString())
        parent = parent.parent
      }
      worklist.add(sub)
    }
  }
  // JVM-public nests of a private class stay referenceable (Kotlin cannot name them,
  // but same-package Java can, loudly at compile time): keep the outer as a shell, as
  // jvm-abi-gen does. Only top-level outers need listing; nested owners survive
  // structurally, and nest members follow the normal visibility rules.
  moduleFragment.files.forEach { file ->
    file.declarations.filterIsInstance<IrClass>().forEach { outer ->
      if (!outer.isDroppedVisibility()) return@forEach
      val hasPublicNest =
          outer.declarations.filterIsInstance<IrClass>().any { nest ->
            val visibility = (nest as? IrDeclarationWithVisibility)?.visibility
            visibility != DescriptorVisibilities.PRIVATE &&
                visibility != DescriptorVisibilities.PRIVATE_TO_THIS &&
                visibility != DescriptorVisibilities.LOCAL
          }
      if (hasPublicNest) keep.addWithOuters(outer)
    }
  }
  return keep
}

private fun MutableSet<String>.addWithOuters(klass: IrClass) {
  add(klass.kotlinFqName.asString())
  var parent = klass.parent
  while (parent is IrClass) {
    add(parent.kotlinFqName.asString())
    parent = parent.parent
  }
}

private fun IrClass.isDroppedVisibility(): Boolean {
  val visibility = (this as? IrDeclarationWithVisibility)?.visibility
  return visibility == DescriptorVisibilities.PRIVATE ||
      visibility == DescriptorVisibilities.PRIVATE_TO_THIS ||
      visibility == DescriptorVisibilities.LOCAL
}

internal data class MethodSignature(
    val name: String,
    val parameterTypes: List<String>,
)

internal fun IrSimpleFunction.methodSignature() = MethodSignature(
    name.asString(),
    compatValueParameters.map { it.type.classFqName?.asString() ?: it.type.toString() },
)

// Toolchain-provided packages dep tracking may skip. The trailing dots are load-bearing: `javax.*`
// and `kotlinx.*` are dependency namespaces (`javax.inject`, `kotlinx.coroutines`), not stdlib/JDK.
internal fun isStdlibOrJdkPackage(packageName: String): Boolean =
    packageName == "kotlin" ||
        packageName.startsWith("kotlin.") ||
        packageName == "java" ||
        packageName.startsWith("java.")
