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

package com.facebook

import org.jetbrains.kotlin.backend.common.extensions.IrPluginContext
import org.jetbrains.kotlin.backend.common.lower.createIrBuilder
import org.jetbrains.kotlin.descriptors.ClassKind
import org.jetbrains.kotlin.descriptors.DescriptorVisibilities
import org.jetbrains.kotlin.descriptors.Modality
import org.jetbrains.kotlin.ir.IrElement
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrDeclaration
import org.jetbrains.kotlin.ir.declarations.IrDeclarationOrigin
import org.jetbrains.kotlin.ir.declarations.IrField
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.declarations.IrProperty
import org.jetbrains.kotlin.ir.expressions.IrConst
import org.jetbrains.kotlin.ir.expressions.impl.IrConstImpl
import org.jetbrains.kotlin.ir.symbols.IrClassSymbol
import org.jetbrains.kotlin.ir.symbols.UnsafeDuringIrConstructionAPI
import org.jetbrains.kotlin.ir.types.IrSimpleType
import org.jetbrains.kotlin.ir.types.IrType
import org.jetbrains.kotlin.ir.types.classFqName
import org.jetbrains.kotlin.ir.types.classOrNull
import org.jetbrains.kotlin.ir.types.isAny
import org.jetbrains.kotlin.ir.types.isBoolean
import org.jetbrains.kotlin.ir.types.isByte
import org.jetbrains.kotlin.ir.types.isChar
import org.jetbrains.kotlin.ir.types.isDouble
import org.jetbrains.kotlin.ir.types.isFloat
import org.jetbrains.kotlin.ir.types.isInt
import org.jetbrains.kotlin.ir.types.isLong
import org.jetbrains.kotlin.ir.types.isNullable
import org.jetbrains.kotlin.ir.types.isShort
import org.jetbrains.kotlin.ir.types.isString
import org.jetbrains.kotlin.ir.types.isUnit
import org.jetbrains.kotlin.ir.types.makeNotNull
import org.jetbrains.kotlin.ir.util.constructors
import org.jetbrains.kotlin.ir.util.getAnnotation
import org.jetbrains.kotlin.ir.util.hasAnnotation
import org.jetbrains.kotlin.ir.util.isEnumClass
import org.jetbrains.kotlin.ir.util.isEnumEntry
import org.jetbrains.kotlin.ir.util.isFileClass
import org.jetbrains.kotlin.ir.util.isInterface
import org.jetbrains.kotlin.name.ClassId
import org.jetbrains.kotlin.name.FqName
import org.jetbrains.kotlin.name.Name

/**
 * Adds $stable static field and @StabilityInferred annotation to eligible classes.
 *
 * The Compose compiler adds $stable and @StabilityInferred to every eligible class so downstream
 * modules can determine stability without re-analyzing fields. This emulation replicates both
 * artifacts for source-only ABI.
 *
 * Eligible classes: public/internal, non-enum, non-interface, non-annotation, non-anonymous,
 * non-expect, non-inner, non-file-class, non-companion, non-inline/value.
 *
 * An eligible class that carries a stable marker gets `$stable = STABLE` and no @StabilityInferred,
 * as Compose's ClassStabilityTransformer does. A stable marker is an annotation whose class is
 * annotated @StableMarker (Compose's @Stable and @Immutable among them), or one Compose lists in
 * KnownStableConstructs.stableMarkers.
 *
 * Stability rules (simplified for ABI emulation):
 * - A class that carries a stable marker, or has a supertype that does: stable, with no
 *   type-parameter dependencies
 * - An object, when the compiler's Compose plugin calls every object stable (2.4 on)
 * - val properties of stable types: stable
 * - val properties typed with one of the class's own type parameters: set that parameter's bit and
 *   are not known stable, as Compose's Stability.Parameter
 * - var properties (non-delegated): unstable
 * - Primitives, String, Unit, enums, function types: stable
 * - A type that carries a stable marker, or has a supertype that does: stable, and so is an object
 *   type when objects are stable
 * - A class is no more stable than its superclass
 * - Unrecognised types: conservatively not-known-stable
 *
 * The two outputs use different encodings. $stable carries StabilityBits: 0 when known stable,
 * UNSTABLE.bitsForSlot(0) = 8 otherwise. @StabilityInferred(parameters) carries the type-parameter
 * bitmask plus a known-stable high bit at (1 shl typeParameters.size). $stable is annotated
 * `@JvmField` when the compiler's Compose plugin writes it as a property's backing field (2.4 on).
 */
@OptIn(UnsafeDuringIrConstructionAPI::class)
internal class ClassStabilityTransformer(private val pluginContext: IrPluginContext) {

  companion object {
    private val COMPOSER_CLASS_ID =
        ClassId(FqName("androidx.compose.runtime"), Name.identifier("Composer"))
    private val STABILITY_INFERRED_CLASS_ID =
        ClassId(FqName("androidx.compose.runtime.internal"), Name.identifier("StabilityInferred"))
    private val STABILITY_INFERRED_FQ_NAME = STABILITY_INFERRED_CLASS_ID.asSingleFqName()
    private val STABLE_MARKER_FQ_NAME = FqName("androidx.compose.runtime.StableMarker")
    private val JVM_FIELD_CLASS_ID = ClassId(FqName("kotlin.jvm"), Name.identifier("JvmField"))

    // Compose's KnownStableConstructs.stableMarkers: annotations it treats as stable markers
    // although they are not annotated @StableMarker.
    private val KNOWN_STABLE_MARKER_FQ_NAMES =
        setOf(FqName("com.google.errorprone.annotations.Immutable"))

    // Known stable FQNs beyond primitives/String/Unit
    private val KNOWN_STABLE_FQNS = setOf(
        "kotlin.Pair",
        "kotlin.Triple",
        "kotlin.Result",
        "kotlin.ranges.IntRange",
        "kotlin.ranges.LongRange",
        "kotlin.ranges.CharRange",
        "kotlin.coroutines.CoroutineContext.Key",
        "kotlinx.collections.immutable.ImmutableList",
        "kotlinx.collections.immutable.ImmutableSet",
        "kotlinx.collections.immutable.ImmutableMap",
        "kotlinx.collections.immutable.ImmutableCollection",
        "kotlinx.collections.immutable.PersistentList",
        "kotlinx.collections.immutable.PersistentSet",
        "kotlinx.collections.immutable.PersistentMap",
        "dagger.Lazy",
    )

    // androidx StabilityBits: STABLE 0b000, UNSTABLE 0b100, bitsForSlot(n) = bits shl (1 + n*3).
    // Written as the derivation rather than 8 so the slot-0 substitution stays visible.
    private const val STABLE_BITS = 0
    private const val UNSTABLE_BITS_SLOT0 = 0b100 shl 1
  }

  // Classes currently being analyzed (cycle detection)
  private val analyzing = mutableSetOf<String>()

  // Lazily resolved — null if compose-runtime is not on the classpath
  private var stabilityInferredClass: IrClassSymbol? = null

  private var jvmFieldClass: IrClassSymbol? = null

  // FQNs declared in the module under compilation. A superclass in this set has not had its
  // @StabilityInferred attached yet (this transformer is what attaches it, and visit order does not
  // follow the inheritance graph), so its stability must be recomputed rather than read back.
  private val moduleClassFqns = mutableSetOf<String>()

  private val stableMarkedDescendantCache = mutableMapOf<IrClass, Boolean>()

  fun transform(moduleFragment: IrModuleFragment) {
    // No-op when Compose runtime is not on the classpath.
    pluginContext.referenceClass(COMPOSER_CLASS_ID) ?: return
    stabilityInferredClass = pluginContext.referenceClass(STABILITY_INFERRED_CLASS_ID)
    jvmFieldClass = pluginContext.referenceClass(JVM_FIELD_CLASS_ID)
    // Carrying FQNs across fragments would route a compiled dependency down the recompute branch
    // instead of the external-annotation one.
    moduleClassFqns.clear()
    moduleFragment.accept(ModuleClassCollector(), null)
    moduleFragment.accept(StabilityVisitor(), null)
  }

  private inner class ModuleClassCollector : IrElementVisitorVoidCompat() {
    override fun visitElement(element: IrElement) {
      element.acceptChildren(this, null)
    }

    override fun visitClass(declaration: IrClass) {
      declaration.classFqName()?.let { moduleClassFqns.add(it) }
      super.visitClass(declaration)
    }
  }

  private inner class StabilityVisitor : IrElementVisitorVoidCompat() {

    override fun visitElement(element: IrElement) {
      element.acceptChildren(this, null)
    }

    override fun visitClass(declaration: IrClass) {
      if (shouldTransform(declaration)) {
        addStabilityField(declaration)
      }
      // Visit nested classes
      super.visitClass(declaration)
    }
  }

  private fun shouldTransform(irClass: IrClass): Boolean {
    // Skip enum classes and enum entries
    if (irClass.isEnumClass || irClass.isEnumEntry) return false
    // Skip interfaces
    if (irClass.isInterface) return false
    // Skip annotation classes
    if (irClass.kind == ClassKind.ANNOTATION_CLASS) return false
    // Skip anonymous objects
    if (irClass.kind == ClassKind.OBJECT && irClass.name.isSpecial) return false
    // Skip expect declarations
    if (irClass.isExpect) return false
    // Skip inner classes
    if (irClass.isInner) return false
    // Skip file classes (top-level function holders)
    if (irClass.isFileClass) return false
    // Skip companion objects
    if (irClass.isCompanion) return false
    // Skip inline/value classes
    if (irClass.isValue) return false
    val vis = irClass.visibility
    if (vis != DescriptorVisibilities.PUBLIC && vis != DescriptorVisibilities.INTERNAL) {
      return false
    }
    return true
  }

  private fun addStabilityField(irClass: IrClass) {
    val stability = computeStability(irClass)
    val stableValue = if (stability.knownStable) STABLE_BITS else UNSTABLE_BITS_SLOT0

    // Add static final $stable: Int field
    val stableField =
        pluginContext.irFactory.createField(
            startOffset = -1,
            endOffset = -1,
            origin = IrDeclarationOrigin.DEFINED,
            name = Name.identifier("\$stable"),
            type = pluginContext.irBuiltIns.intType,
            visibility = DescriptorVisibilities.PUBLIC,
            symbol = org.jetbrains.kotlin.ir.symbols.impl.IrFieldSymbolImpl(),
            isFinal = true,
            isExternal = false,
            isStatic = true,
        )
    stableField.parent = irClass
    val constExpr = IrConstImpl.int(-1, -1, pluginContext.irBuiltIns.intType, stableValue)
    stableField.initializer = pluginContext.irFactory.createExpressionBody(-1, -1, constExpr)
    // The Compose plugin of 2.4 and later writes $stable as a property whose backing field is
    // annotated @JvmField, so its getter is never generated. The annotation is all that reaches the
    // class file.
    if (COMPOSE_STABLE_FIELD_IS_JVM_FIELD) {
      jvmFieldClass?.constructors?.singleOrNull()?.let { constructor ->
        val jvmField =
            pluginContext.irBuiltIns.createIrBuilder(irClass.symbol).run {
              irAnnotationCompat(constructor, emptyList())
            }
        stableField.annotations = stableField.annotations + jvmField
      }
    }
    irClass.declarations.add(stableField)

    // Compose gives a marked class `$stable = STABLE` and no @StabilityInferred.
    if (hasStableMarker(irClass)) return

    // Not the same encoding as $stable: a bitmask over the class's type parameters, plus a high
    // bit at (1 shl typeParameters.size) meaning "known stable regardless of its parameters".
    // The `< 32` bound mirrors Compose exactly (ClassStabilityTransformer.kt:161 emits it,
    // Stability.kt:246 reads it). At 31 params both sides use `1 shl 31` = Int.MIN_VALUE and
    // compare with `and`, so the sign bit is correct; tightening to `< 31` would emit 0 here
    // while the consumer still looks for that bit, silently losing "known stable".
    val knownStableBit =
        if (stability.knownStable && irClass.typeParameters.size < 32) {
          1 shl irClass.typeParameters.size
        } else {
          0
        }
    addStabilityInferredAnnotation(irClass, stability.typeParamMask or knownStableBit)
  }

  private fun addStabilityInferredAnnotation(irClass: IrClass, parametersValue: Int) {
    val annotationClass = stabilityInferredClass ?: return
    val constructor = annotationClass.constructors.singleOrNull() ?: return
    val annotation =
        pluginContext.irBuiltIns.createIrBuilder(irClass.symbol).run {
          irAnnotationCompat(constructor, emptyList()).apply {
            arguments[0] =
                IrConstImpl.int(-1, -1, pluginContext.irBuiltIns.intType, parametersValue)
          }
        }
    // Register via metadataDeclarationRegistrar so HAS_ANNOTATIONS is set in Kotlin metadata.
    // Without it, consumers cannot see the emulated annotation and fold stability constants
    // instead of deferring to runtime.
    pluginContext.metadataDeclarationRegistrar.addMetadataVisibleAnnotationsToElement(
        irClass,
        annotation,
    )
  }

  /** Type-parameter bitmask plus whether the class itself is known stable. */
  private data class Stability(val typeParamMask: Int, val knownStable: Boolean)

  // Deliberately conservative: an unrecognised member type yields not-known-stable, so consumers
  // resolve stability at runtime rather than assume the wrong answer.
  private fun computeStability(irClass: IrClass, asSuperclass: Boolean = false): Stability {
    // Compose answers Stable here before it looks at any member, superclass or type parameter.
    if (hasStableMarkedDescendant(irClass)) return Stability(0, knownStable = true)
    if (COMPOSE_OBJECTS_ARE_STABLE && irClass.kind == ClassKind.OBJECT) {
      return Stability(0, knownStable = true)
    }

    val classFqn = irClass.classFqName()
    if (classFqn != null && analyzing.contains(classFqn)) {
      // Cycle — do not claim knowledge we do not have.
      return Stability(0, knownStable = false)
    }
    if (classFqn != null) analyzing.add(classFqn)

    var mask = 0
    var knownStable = true
    for (declaration in irClass.declarations) {
      when (declaration) {
        is IrProperty -> {
          if (declaration.isVar && declaration.isDelegated != true) {
            if (classFqn != null) analyzing.remove(classFqn)
            return Stability(0, knownStable = false)
          }
          // Only stored state counts. A computed getter has no backing field and is skipped,
          // which is what the real StabilityInferencer does - it reaches members through
          // `member.backingField?.let { ... }`.
          val backingField = declaration.backingField
          if (backingField != null) {
            mask = mask or typeParamBits(backingField.type, irClass)
            if (!isKnownStableType(backingField.type)) {
              knownStable = false
            }
          }
        }
        is IrField -> {
          if (!declaration.isStatic) {
            mask = mask or typeParamBits(declaration.type, irClass)
            if (!isKnownStableType(declaration.type)) {
              knownStable = false
            }
          }
        }
        else -> {}
      }
    }

    // A subclass holds every field its superclass declares, so it can be no more stable than that
    // superclass. Skipping this let a class with no stored state of its own be called stable while
    // the real compiler called it unstable — e.g. `object EmptyPainter : Painter()`, where all the
    // mutable state lives in Painter. Over-asserting stability makes a consumer skip recomposition
    // for a value that did change, so this is deliberately the pessimistic direction.
    if (knownStable && !superclassKnownStable(irClass)) {
      knownStable = false
    }
    // From 2.4 Compose starts a class that is not final at `Stability.Unknown`. As a superclass
    // such
    // a class is still answered by its members: Compose drops a superclass that is only Unknown and
    // keeps one with an unstable member.
    if (
        !asSuperclass &&
            COMPOSE_NON_FINAL_CLASSES_ARE_UNKNOWN &&
            irClass.modality != Modality.FINAL &&
            !irClass.isEnumClass
    ) {
      knownStable = false
    }

    if (classFqn != null) analyzing.remove(classFqn)
    return Stability(mask, knownStable)
  }

  /**
   * Whether [irClass]'s superclass permits it to be known stable. Interfaces and [Any] are ignored:
   * neither contributes stored state.
   */
  private fun superclassKnownStable(irClass: IrClass): Boolean {
    val superClass =
        irClass.superTypes.mapNotNull { it.classOrNull?.owner }.firstOrNull { !it.isInterface }
            ?: return true
    val superFqn = superClass.classFqName() ?: return false
    if (superFqn == "kotlin.Any") return true
    if (superFqn in moduleClassFqns) {
      return computeStability(superClass, asSuperclass = true).knownStable
    }
    // Compiled dependency: trust its @StabilityInferred, and treat an absent one as unstable. A
    // class outside a Compose-enabled compilation carries no stability information, and its private
    // state may be stripped from the ABI jar, so its declarations cannot be inspected instead.
    return externalClassKnownStable(superClass)
  }

  private fun externalClassKnownStable(irClass: IrClass): Boolean {
    val annotation = irClass.getAnnotation(STABILITY_INFERRED_FQ_NAME) ?: return false
    val parameters = (annotation.arguments[0] as? IrConst)?.value as? Int ?: return false
    val typeParamCount = irClass.typeParameters.size
    if (typeParamCount >= 32) return false
    return (parameters and (1 shl typeParamCount)) != 0
  }

  /** Whether [irClass] itself carries a stable marker (Compose's `hasStableMarker`). */
  private fun hasStableMarker(irClass: IrClass): Boolean =
      irClass.annotations.any { annotation ->
        annotation.type.classFqName in KNOWN_STABLE_MARKER_FQ_NAMES ||
            annotation.type.classOrNull?.owner?.hasAnnotation(STABLE_MARKER_FQ_NAME) == true
      }

  /**
   * Whether [irClass] or any of its supertypes carries a stable marker. Compose names this
   * `hasStableMarkedDescendant`, and walks superclasses and interfaces alike.
   */
  private fun hasStableMarkedDescendant(irClass: IrClass): Boolean =
      stableMarkedDescendantCache.getOrPut(irClass) {
        hasStableMarker(irClass) ||
            irClass.superTypes.any { superType ->
              !superType.isAny() &&
                  superType.classOrNull?.owner?.let { hasStableMarkedDescendant(it) } == true
            }
      }

  // Determine which type parameter bits are affected by a given type.
  // Returns a bitmask where bit N is set if type parameter N of the class appears in the type.
  private fun typeParamBits(type: IrType, irClass: IrClass): Int {
    if (irClass.typeParameters.isEmpty()) return 0

    // If the type is already known stable, no type param dependency
    if (isKnownStableType(type)) return 0

    // Check if the type IS a type parameter of this class
    if (type is IrSimpleType) {
      val classifier = type.classifier
      for ((index, typeParam) in irClass.typeParameters.withIndex()) {
        if (classifier == typeParam.symbol) {
          return 1 shl index
        }
      }

      // Recurse into type arguments
      var bits = 0
      for (arg in type.arguments) {
        if (arg is org.jetbrains.kotlin.ir.types.IrTypeProjection) {
          bits = bits or typeParamBits(arg.type, irClass)
        }
      }
      return bits
    }
    return 0
  }

  // Check if a type is known stable (primitives, String, enums, function types, etc.)
  private fun isKnownStableType(type: IrType): Boolean {
    // Handle nullable — strip and check inner
    if (type.isNullable()) {
      return isKnownStableType(type.makeNotNull())
    }

    // Primitives
    if (
        type.isInt() ||
            type.isLong() ||
            type.isFloat() ||
            type.isDouble() ||
            type.isBoolean() ||
            type.isByte() ||
            type.isShort() ||
            type.isChar()
    ) {
      return true
    }
    // String, Unit
    if (type.isString() || type.isUnit()) return true

    val fqn = type.classFqName?.asString() ?: return false

    // Function types
    if (fqn.startsWith("kotlin.Function") || fqn.startsWith("kotlin.reflect.KFunction")) {
      return true
    }

    // Enums, and the types Compose calls stable before it looks at their state or their arguments:
    // those that carry a stable marker or have a supertype that does, and objects where its
    // StabilityInferencer calls every object stable.
    val classSymbol = (type as? IrSimpleType)?.classOrNull
    if (classSymbol != null) {
      val owner = classSymbol.owner
      if (owner.isEnumClass) return true
      val marked =
          if (owner.isValue && !COMPOSE_VALUE_CLASS_MARKERS_INCLUDE_SUPERTYPES)
              hasStableMarker(owner)
          else hasStableMarkedDescendant(owner)
      if (marked) return true
      if (COMPOSE_OBJECTS_ARE_STABLE && owner.kind == ClassKind.OBJECT) return true
    }

    // Known stable types. The container-shaped ones are only as stable as their arguments:
    // `Pair<String, MutableList<Int>>` matches on FQN alone but is not stable, and claiming it is
    // makes a consumer skip a recomposition it needed. A star projection or a type-parameter
    // argument is not known stable here either — that stability is carried by the type-parameter
    // mask, which is a separate signal from this one.
    if (fqn in KNOWN_STABLE_FQNS) {
      return (type as? IrSimpleType)?.arguments.orEmpty().all {
        it is org.jetbrains.kotlin.ir.types.IrTypeProjection && isKnownStableType(it.type)
      }
    }

    // SDK types (java.*, kotlin.* excluding collections interfaces)
    // Kotlin collections interfaces (List, Set, Map) are NOT stable — they're interfaces
    if (
        fqn == "kotlin.collections.List" ||
            fqn == "kotlin.collections.Set" ||
            fqn == "kotlin.collections.Map" ||
            fqn == "kotlin.collections.MutableList" ||
            fqn == "kotlin.collections.MutableSet" ||
            fqn == "kotlin.collections.MutableMap" ||
            fqn == "kotlin.collections.Collection" ||
            fqn == "kotlin.collections.MutableCollection"
    ) {
      return false
    }

    return false
  }

  private fun IrClass.classFqName(): String? {
    return this.classFqName?.asString()
  }

  private val IrClass.classFqName: FqName?
    get() {
      val segments = mutableListOf<String>()
      var current: IrClass? = this
      while (current != null) {
        segments.add(0, current.name.asString())
        current = current.parent as? IrClass
      }
      val packageFqName =
          generateSequence(this.parent) { (it as? IrDeclaration)?.parent }
              .filterIsInstance<org.jetbrains.kotlin.ir.declarations.IrPackageFragment>()
              .firstOrNull()
              ?.packageFqName ?: return null
      return FqName(packageFqName.asString() + "." + segments.joinToString("."))
    }
}
