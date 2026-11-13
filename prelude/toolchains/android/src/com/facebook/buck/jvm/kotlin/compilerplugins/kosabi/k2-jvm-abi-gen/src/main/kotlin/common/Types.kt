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
import org.jetbrains.kotlin.descriptors.ClassKind
import org.jetbrains.kotlin.descriptors.DescriptorVisibilities
import org.jetbrains.kotlin.descriptors.Modality
import org.jetbrains.kotlin.fir.types.ConeKotlinType
import org.jetbrains.kotlin.ir.IrElement
import org.jetbrains.kotlin.ir.declarations.IrAnnotationContainer
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrConstructor
import org.jetbrains.kotlin.ir.declarations.IrDeclaration
import org.jetbrains.kotlin.ir.declarations.IrDeclarationContainer
import org.jetbrains.kotlin.ir.declarations.IrDeclarationOrigin
import org.jetbrains.kotlin.ir.declarations.IrDeclarationWithName
import org.jetbrains.kotlin.ir.declarations.IrDeclarationWithVisibility
import org.jetbrains.kotlin.ir.declarations.IrField
import org.jetbrains.kotlin.ir.declarations.IrFunction
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.declarations.IrParameterKind
import org.jetbrains.kotlin.ir.declarations.IrProperty
import org.jetbrains.kotlin.ir.declarations.IrSimpleFunction
import org.jetbrains.kotlin.ir.declarations.IrTypeParametersContainer
import org.jetbrains.kotlin.ir.expressions.IrClassReference
import org.jetbrains.kotlin.ir.types.IrSimpleType
import org.jetbrains.kotlin.ir.types.IrType
import org.jetbrains.kotlin.ir.types.IrTypeProjection
import org.jetbrains.kotlin.ir.types.classFqName
import org.jetbrains.kotlin.ir.types.classOrNull
import org.jetbrains.kotlin.ir.util.kotlinFqName
import org.jetbrains.kotlin.name.Name

data object JvmAbiGenPlugin : GeneratedDeclarationKey()

data class InterfaceMethodInfo(
    val name: Name,
    val returnType: ConeKotlinType,
    val valueParameters: List<Pair<Name, ConeKotlinType>>,
    val modality: Modality,
)

// Private classes the ABI must keep as stubs, or consumers break: a kept sealed
// type's metadata names every subclass in `sealedSubclasses` (dropping one lets
// consumers compile a non-exhaustive `when` that throws NoWhenBranchMatchedException),
// and annotation arguments name classes javac annotation processors read
// (a missing one surfaces as an ERROR type). Collect fq names transitively, plus
// private enclosing classes (a kept nested class needs its outers as stubs).
// Only classes that would otherwise be dropped need listing, but recording the
// full set is harmless.
internal fun collectMustKeepPrivateClassNames(moduleFragment: IrModuleFragment): Set<String> {
  val keep = mutableSetOf<String>()
  val classes = mutableListOf<IrClass>()
  moduleFragment.files.forEach { file ->
    file.acceptChildren(
        object : IrElementVisitorVoidCompat() {
          override fun visitElement(element: IrElement) {
            if (element is IrClass) classes.add(element)
            element.acceptChildren(this, null)
          }
        },
        null,
    )
  }
  moduleFragment.files.forEach { file ->
    file.declarations.forEach { scanKeptDeclaration(it, keep, ownerKept = true) }
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
  // A kept private class keeps its own annotations and, when sealed, its subclasses;
  // each can name further private classes.
  val scanned = mutableSetOf<String>()
  do {
    val keptBefore = keep.size
    keepSealedSubclasses(classes, keep)
    for (klass in classes) {
      val fqName = klass.kotlinFqName.asString()
      if (klass.isDroppedVisibility() && fqName in keep && scanned.add(fqName)) {
        scanKeptPrivateClass(klass, keep)
      }
    }
  } while (keep.size != keptBefore)
  return keep
}

private fun keepSealedSubclasses(classes: List<IrClass>, keep: MutableSet<String>) {
  val worklist = ArrayDeque(classes)
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
      keep.addWithOuters(sub)
      if (visited.add(sub.kotlinFqName.asString())) worklist.add(sub)
    }
  }
}

// The stripper keeps a private class as a shell: only its annotations, supertypes,
// constructors, nested classes and constants survive.
private fun scanKeptPrivateClass(klass: IrClass, keep: MutableSet<String>) {
  scanAnnotationsForClassLiterals(klass, keep)
  scanSignatures(klass, keep)
  klass.declarations
      .filter { it is IrClass || it is IrConstructor || (it as? IrProperty)?.isConst == true }
      .forEach { scanKeptDeclaration(it, keep, ownerKept = true) }
}

private fun MutableSet<String>.addWithOuters(klass: IrClass) {
  add(klass.kotlinFqName.asString())
  var parent = klass.parent
  while (parent is IrClass) {
    add(parent.kotlinFqName.asString())
    parent = parent.parent
  }
}

private fun isKeptOwner(declaration: IrDeclaration): Boolean {
  // Only declarations that survive stripping confer keep through their annotations:
  // a dropped owner's annotations die with it. The stripper keeps every constructor and
  // nested class of a kept owner; any other member must not be dropped-visibility.
  if (declaration is IrConstructor) return true
  if (declaration is IrClass && declaration.parent is IrClass) return true
  val visibility = (declaration as? IrDeclarationWithVisibility)?.visibility
  return visibility != DescriptorVisibilities.PRIVATE &&
      visibility != DescriptorVisibilities.PRIVATE_TO_THIS &&
      visibility != DescriptorVisibilities.LOCAL
}

private fun scanKeptDeclaration(
    declaration: IrDeclaration,
    keep: MutableSet<String>,
    ownerKept: Boolean,
) {
  val kept = ownerKept && isKeptOwner(declaration)
  if (kept) {
    scanAnnotationsForClassLiterals(declaration, keep)
    scanSignatures(declaration, keep)
    if (declaration is IrFunction) {
      declaration.compatValueParameters.forEach { scanAnnotationsForClassLiterals(it, keep) }
    }
    if (declaration is IrProperty) {
      for (accessor in listOfNotNull(declaration.getter, declaration.setter)) {
        if (!isKeptOwner(accessor)) continue
        scanAnnotationsForClassLiterals(accessor, keep)
        scanSignatures(accessor, keep)
        accessor.compatValueParameters.forEach { scanAnnotationsForClassLiterals(it, keep) }
      }
      declaration.backingField?.let {
        scanAnnotationsForClassLiterals(it, keep)
        scanSignatures(it, keep)
      }
    }
  }
  if (declaration is IrDeclarationContainer) {
    declaration.declarations.forEach { scanKeptDeclaration(it, keep, kept) }
  }
}

private fun scanSignatures(declaration: IrDeclaration, keep: MutableSet<String>) {
  // A delegate backing field is always dropped (its type would dangle), so its
  // type must not confer keep: the field dies with the class that names it.
  if (declaration.isDelegateBackingField()) return
  // Supertypes and signatures name classes consumers must resolve (suppressed
  // EXPOSED_* or otherwise): keep them as stubs. Supertypes keep class-kind
  // only: a private interface named only as a supertype stays on the
  // strip-and-materialize path (its members materialize into implementers,
  // including enum entries), while a kept private class must stay a
  // resolvable supertype.
  if (declaration is IrClass) {
    declaration.superTypes.forEach { collectClassesFromType(it, keep, classesOnly = true) }
  }
  if (declaration is IrSimpleFunction) {
    collectClassesFromType(declaration.returnType, keep)
  }
  if (declaration is IrFunction) {
    declaration.parameters
        .filter { it.kind != IrParameterKind.DispatchReceiver }
        .forEach { collectClassesFromType(it.type, keep) }
  }
  if (declaration is IrTypeParametersContainer) {
    declaration.typeParameters.forEach { parameter ->
      parameter.superTypes.forEach { collectClassesFromType(it, keep) }
    }
  }
  if (declaration is IrField) {
    collectClassesFromType(declaration.type, keep)
  }
}

private fun collectClassesFromType(
    type: IrType,
    keep: MutableSet<String>,
    classesOnly: Boolean = false,
) {
  val referenced = type.classOrNull?.owner as? IrClass
  if (referenced != null && (!classesOnly || referenced.kind == ClassKind.CLASS)) {
    keep.addWithOuters(referenced)
  }
  // Type arguments are references, not supertypes: they keep every class kind.
  for (argument in (type as? IrSimpleType)?.arguments.orEmpty()) {
    (argument as? IrTypeProjection)?.type?.let { collectClassesFromType(it, keep) }
  }
}

private fun scanAnnotationsForClassLiterals(
    owner: IrAnnotationContainer,
    keep: MutableSet<String>,
) {
  for (annotation in owner.annotations) {
    annotation.acceptChildren(
        object : IrElementVisitorVoidCompat() {
          override fun visitElement(element: IrElement) {
            val referenced = (element as? IrClassReference)?.symbol?.owner as? IrClass
            if (referenced != null) keep.addWithOuters(referenced)
            element.acceptChildren(this, null)
          }
        },
        null,
    )
  }
}

private fun IrClass.isDroppedVisibility(): Boolean {
  val visibility = (this as? IrDeclarationWithVisibility)?.visibility
  return visibility == DescriptorVisibilities.PRIVATE ||
      visibility == DescriptorVisibilities.PRIVATE_TO_THIS ||
      visibility == DescriptorVisibilities.LOCAL
}

// A synthesized delegate backing field (`by lazy` property delegates and
// `$$delegate_N` interface-delegation fields) is always dropped from the ABI,
// so it must neither survive stripping nor confer keep through its type.
internal fun IrDeclaration.isDelegateBackingField(): Boolean {
  if (this !is IrField) return false
  if (visibility != DescriptorVisibilities.PRIVATE) return false
  if (origin == IrDeclarationOrigin.PROPERTY_DELEGATE) return true
  return origin == IrDeclarationOrigin.DELEGATE ||
      (this as IrDeclarationWithName).name.asString().startsWith("\$\$delegate_")
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
