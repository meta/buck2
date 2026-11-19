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
import org.jetbrains.kotlin.ir.declarations.IrFile
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.declarations.IrProperty
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
import org.jetbrains.kotlin.ir.util.fileOrNull
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
 * Stability rules (Compose's `Stability` algebra over the members and the superclass):
 * - A class that carries a stable marker, or has a supertype that does: stable, with no
 *   type-parameter dependencies
 * - An object, when the compiler's Compose plugin calls every object stable (2.4 on)
 * - val properties of stable types: stable
 * - val properties typed with one of the class's own type parameters: set that parameter's bit and
 *   are not known stable, as Compose's Stability.Parameter
 * - var properties (non-delegated): unstable
 * - Primitives, String, Unit, enums, function types (including the synthetic
 *   `(K)ComposableFunctionN` kinds): stable
 * - Interface-typed members are `Unknown`: one such member leaves the class not known stable, two
 *   or more combine and are dropped by `normalize()`, so the class is known stable. From 2.4 a
 *   non-final class starts at `Unknown`, so one interface member is enough there. Java-stub
 *   interfaces are unstable, and on 2.3 so is any interface from another module.
 * - An in-module member class is analysed structurally: from 2.4 one declared in the same file, on
 *   2.3 any in-module class. A member class from another file is runtime stability, as is any other
 *   unrecognised type: conservatively not-known-stable
 * - An unmarked value-class member takes the stability of its underlying type (2.3/2.4) or of its
 *   fields, after the marker check (whose supertype scope is versioned)
 * - A class combines the stability of its superclass, unless that superclass is only `Unknown`,
 *   which is dropped (2.4 on). A superclass from another file of the module, or from a compiled
 *   dependency, is runtime stability: never known stable. The superclass is the one the class had
 *   before DataClassGenerate made it `DataClassSuper` ([DeclaredSuperclassRecorder])
 *
 * The two outputs use different encodings. $stable carries StabilityBits: 0 when known stable,
 * UNSTABLE.bitsForSlot(0) = 8 otherwise. @StabilityInferred(parameters) carries the type-parameter
 * bitmask plus a known-stable high bit at (1 shl typeParameters.size). $stable is annotated
 * `@JvmField` when the compiler's Compose plugin writes it as a property's backing field (2.4 on).
 */
@OptIn(UnsafeDuringIrConstructionAPI::class)
internal class ClassStabilityTransformer(
    private val pluginContext: IrPluginContext,
    // The superclass of each module class before the plugin IR extensions ran
    // ([DeclaredSuperclassRecorder]); a class missing here falls back to its current superclass.
    private val declaredSuperclasses: Map<IrClass, IrClass?> = emptyMap(),
) {

  companion object {
    private val COMPOSER_CLASS_ID =
        ClassId(FqName("androidx.compose.runtime"), Name.identifier("Composer"))
    private val STABILITY_INFERRED_CLASS_ID =
        ClassId(FqName("androidx.compose.runtime.internal"), Name.identifier("StabilityInferred"))
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

    // The synthetic composable function-type kinds K2 classifies `@Composable (P..) -> R` as.
    // Compose's StabilityInferencer calls both stable; see ComposableTypeRewriter for the split
    // between these and the legacy `kotlin.FunctionN` + annotation form.
    private const val COMPOSABLE_FUNCTION_FQ_PREFIX =
        "androidx.compose.runtime.internal.ComposableFunction"
    private const val K_COMPOSABLE_FUNCTION_FQ_PREFIX =
        "androidx.compose.runtime.internal.KComposableFunction"
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

  /** Type-parameter bitmask plus the folded member/superclass stability. */
  private data class Stability(val typeParamMask: Int, val folded: EmulatedStability) {
    val knownStable: Boolean
      get() = folded.knownStable()
  }

  // Deliberately conservative: an unrecognised member type yields not-known-stable, so consumers
  // resolve stability at runtime rather than assume the wrong answer.
  private fun computeStability(irClass: IrClass): Stability {
    // Compose answers Stable here before it looks at any member, superclass or type parameter.
    if (hasStableMarkedDescendant(irClass)) {
      return Stability(0, EmulatedStability.Stable)
    }
    if (COMPOSE_OBJECTS_ARE_STABLE && irClass.kind == ClassKind.OBJECT) {
      return Stability(0, EmulatedStability.Stable)
    }

    val classFqn = irClass.classFqName()
    if (classFqn != null && analyzing.contains(classFqn)) {
      // Cycle — Compose answers Unstable for a class it is already analysing.
      return Stability(0, EmulatedStability.Unstable)
    }
    if (classFqn != null) analyzing.add(classFqn)
    try {
      var mask = 0
      // From 2.4 Compose starts a class that is not final at `Stability.Unknown`.
      var folded: EmulatedStability =
          if (
              COMPOSE_NON_FINAL_CLASSES_ARE_UNKNOWN &&
                  irClass.modality != Modality.FINAL &&
                  !irClass.isEnumClass
          ) {
            EmulatedStability.Unknown
          } else {
            EmulatedStability.Stable
          }
      val analyzedFile = irClass.fileOrNull
      for (declaration in irClass.declarations) {
        when (declaration) {
          is IrProperty -> {
            if (declaration.isVar && declaration.isDelegated != true) {
              return Stability(0, EmulatedStability.Unstable)
            }
            // Only stored state counts. A computed getter has no backing field and is skipped,
            // which is what the real StabilityInferencer does - it reaches members through
            // `member.backingField?.let { ... }`.
            val backingField = declaration.backingField
            if (backingField != null) {
              mask = mask or typeParamBits(backingField.type, irClass)
              folded = folded.plus(stabilityOfType(backingField.type, analyzedFile))
            }
          }
          is IrField -> {
            if (!declaration.isStatic) {
              mask = mask or typeParamBits(declaration.type, irClass)
              folded = folded.plus(stabilityOfType(declaration.type, analyzedFile))
            }
          }
          else -> {}
        }
      }

      // A subclass holds every field its superclass declares, so the superclass's stability is
      // combined into its own. Skipping this let a class with no stored state of its own be called
      // stable while the real compiler called it unstable — e.g. `object EmptyPainter :
      // Painter()`, where all the mutable state lives in Painter. Over-asserting stability makes
      // a consumer skip recomposition for a value that did change, so this is deliberately the
      // pessimistic direction.
      folded = folded.plus(superclassStability(irClass))

      return Stability(mask, folded)
    } finally {
      if (classFqn != null) analyzing.remove(classFqn)
    }
  }

  /**
   * The superclass's contribution to [irClass]'s stability. Interfaces and [Any] are ignored:
   * neither contributes stored state. A superclass the analysis cannot see — from another file of
   * the module (2.4 on), or from a compiled dependency — is runtime stability: Compose forces it
   * rather than read back a bitmask that incremental compilation may have produced from stale
   * inputs.
   */
  private fun superclassStability(irClass: IrClass): EmulatedStability {
    val superClass =
        (if (irClass in declaredSuperclasses) declaredSuperclasses[irClass]
        else irClass.superclassOrNull()) ?: return EmulatedStability.Stable
    val superFqn = superClass.classFqName() ?: return EmulatedStability.Unstable
    if (superFqn == "kotlin.Any") return EmulatedStability.Stable
    if (superFqn in moduleClassFqns) {
      if (COMPOSE_STABILITY_IS_FILE_SCOPED && superClass.fileOrNull !== irClass.fileOrNull) {
        return EmulatedStability.Other
      }
      val folded = computeStability(superClass).folded
      // From 2.4 Compose drops a superclass that is only Unknown and combines any other. A
      // private member type cannot cross files, so the class under analysis and the superclass it
      // reaches structurally are always in the same file here, and no entry file needs threading.
      return if (COMPOSE_STABILITY_IS_FILE_SCOPED && folded === EmulatedStability.Unknown) {
        EmulatedStability.Stable
      } else {
        folded
      }
    }
    // Compiled dependency: Compose forces runtime stability for what it cannot analyse (a class
    // outside a Compose-enabled compilation carries no stability information, and its private
    // state may be stripped from the ABI jar, so its declarations cannot be inspected instead).
    // Treating its bitmask as authoritative over-asserted every subclass of a stable dependency
    // class.
    return EmulatedStability.Other
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
    if (stabilityOfType(type, irClass.fileOrNull) === EmulatedStability.Stable) return 0

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

  // The stability of a member type, following Compose's StabilityInferencer check order: markers
  // and enums before interfaces, known-stable containers before the interface rule (an
  // `ImmutableList` of unstable arguments takes the argument's stability, which normalize()
  // keeps — it is never Unknown), and structural analysis last.
  private fun stabilityOfType(type: IrType, analyzedFile: IrFile?): EmulatedStability {
    // Handle nullable — strip and check inner
    if (type.isNullable()) {
      return stabilityOfType(type.makeNotNull(), analyzedFile)
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
      return EmulatedStability.Stable
    }
    // String, Unit
    if (type.isString() || type.isUnit()) return EmulatedStability.Stable

    val fqn = type.classFqName?.asString() ?: return EmulatedStability.Unstable

    // Function types, including the synthetic composable kinds K2 classifies `@Composable`
    // function types as. Compose's isSyntheticComposableFunction matches on the same prefix.
    if (
        fqn.startsWith("kotlin.Function") ||
            fqn.startsWith("kotlin.reflect.KFunction") ||
            fqn.startsWith(COMPOSABLE_FUNCTION_FQ_PREFIX) ||
            fqn.startsWith(K_COMPOSABLE_FUNCTION_FQ_PREFIX)
    ) {
      return EmulatedStability.Stable
    }

    // Enums, and the types Compose calls stable before it looks at their state or their arguments:
    // those that carry a stable marker or have a supertype that does, and objects where its
    // StabilityInferencer calls every object stable. A value class without its marker takes the
    // stability of its underlying type instead (an unmarked full value class combines its fields).
    val classSymbol = (type as? IrSimpleType)?.classOrNull
    val owner = classSymbol?.owner
    if (owner != null) {
      if (owner.isEnumClass) return EmulatedStability.Stable
      if (owner.isValue) {
        val marked =
            if (!COMPOSE_VALUE_CLASS_MARKERS_INCLUDE_SUPERTYPES) hasStableMarker(owner)
            else hasStableMarkedDescendant(owner)
        if (marked) return EmulatedStability.Stable
        if (fqn in KNOWN_STABLE_FQNS) {
          return knownStableConstructStability(type, analyzedFile)
        }
        return valueClassStability(owner, analyzedFile)
      }
      if (hasStableMarkedDescendant(owner)) return EmulatedStability.Stable
      if (COMPOSE_OBJECTS_ARE_STABLE && owner.kind == ClassKind.OBJECT) {
        return EmulatedStability.Stable
      }
    }

    // Known stable types. The container-shaped ones are only as stable as their arguments:
    // `Pair<String, MutableList<Int>>` matches on FQN alone but is not stable, and claiming it is
    // makes a consumer skip a recomposition it needed. A star projection or a type-parameter
    // argument is not known stable here either — that stability is carried by the type-parameter
    // mask, which is a separate signal from this one. The arguments fold through plus(), so an
    // `ImmutableList` of `Unknown` arguments stays `Unknown` rather than collapsing to stable.
    if (fqn in KNOWN_STABLE_FQNS) {
      return knownStableConstructStability(type, analyzedFile)
    }

    if (owner != null) {
      if (owner.origin == IrDeclarationOrigin.IR_EXTERNAL_JAVA_DECLARATION_STUB) {
        return EmulatedStability.Unstable
      }
      // Interfaces are Unknown: stability bitmasks are never populated for them. On 2.3 an
      // interface from another module takes the bitmask branch instead, which has no bitmask to
      // find, so it is Unstable there.
      if (owner.isInterface) {
        if (!COMPOSE_STABILITY_IS_FILE_SCOPED) {
          val ownerFqn = owner.classFqName()
          if (ownerFqn == null || ownerFqn !in moduleClassFqns) {
            return EmulatedStability.Unstable
          }
        }
        return EmulatedStability.Unknown
      }
      // An in-module member class is analysed structurally: from 2.4 one declared in the same
      // file (a member type visible from another file is public or internal, so Compose forces
      // runtime stability for it), on 2.3 any in-module class. Annotation classes keep the
      // legacy answer: their IR members do not model what Compose analyses.
      if (owner.kind == ClassKind.CLASS || owner.kind == ClassKind.OBJECT) {
        val ownerFqn = owner.classFqName()
        if (
            ownerFqn != null &&
                ownerFqn in moduleClassFqns &&
                (!COMPOSE_STABILITY_IS_FILE_SCOPED || owner.fileOrNull === analyzedFile)
        ) {
          return computeStability(owner).folded
        }
      }
    }

    return EmulatedStability.Unstable
  }

  /** A known-stable container folded over its type arguments through plus(). */
  private fun knownStableConstructStability(
      type: IrType,
      analyzedFile: IrFile?,
  ): EmulatedStability {
    var folded: EmulatedStability = EmulatedStability.Stable
    for (arg in (type as? IrSimpleType)?.arguments.orEmpty()) {
      val argType = (arg as? org.jetbrains.kotlin.ir.types.IrTypeProjection)?.type
      // A star projection carries no stability information.
      folded =
          folded.plus(
              if (argType == null) EmulatedStability.Unstable
              else stabilityOfType(argType, analyzedFile),
          )
    }
    return folded
  }

  /**
   * The stability of an unmarked value class: its underlying type's, or the combination of its
   * fields for a full value class. The analysed file is not threaded through: a value class's
   * members are its underlying representation, and a member class reached from there is analysed
   * against the entry file by the caller.
   */
  private fun valueClassStability(owner: IrClass, analyzedFile: IrFile?): EmulatedStability {
    val ownerFqn = owner.classFqName()
    if (ownerFqn != null && analyzing.contains(ownerFqn)) {
      return EmulatedStability.Unstable
    }
    if (ownerFqn != null) analyzing.add(ownerFqn)
    try {
      var folded: EmulatedStability = EmulatedStability.Stable
      for (declaration in owner.declarations) {
        when (declaration) {
          is IrProperty ->
              declaration.backingField?.let {
                folded = folded.plus(stabilityOfType(it.type, analyzedFile))
              }
          is IrField ->
              if (!declaration.isStatic) {
                folded = folded.plus(stabilityOfType(declaration.type, analyzedFile))
              }
          else -> {}
        }
      }
      return folded
    } finally {
      if (ownerFqn != null) analyzing.remove(ownerFqn)
    }
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

/**
 * Compose's `Stability` algebra (`analysis/Stability.kt`), just enough of it to answer the
 * known-stable bit. `Parameter` and `Runtime` both survive `normalize()` and are never known
 * stable, so they share [EmulatedStability.Other]; nothing here needs them told apart.
 */
private sealed interface EmulatedStability {
  data object Stable : EmulatedStability

  data object Unstable : EmulatedStability

  data object Unknown : EmulatedStability

  /** A type-parameter or runtime part: kept by `normalize()`, never known stable. */
  data object Other : EmulatedStability

  data class Combined(val parts: List<EmulatedStability>) : EmulatedStability
}

/** Compose's `Stability.plus`, unchanged. */
private fun EmulatedStability.plus(other: EmulatedStability): EmulatedStability =
    when {
      other === EmulatedStability.Stable -> this
      other === EmulatedStability.Unstable -> other
      this === EmulatedStability.Stable -> other
      this === EmulatedStability.Unstable -> this
      else -> EmulatedStability.Combined(listOf(this, other))
    }

/**
 * Compose's `normalize().knownStable()`: no unstable, parameter or runtime part, and not a bare
 * `Unknown`. A bare `Unknown` is a single interface-typed member (or a member-less non-final
 * class); two or more `Unknown` parts combine, and `normalize()` drops every one of them, so the
 * class is known stable.
 */
private fun EmulatedStability.knownStable(): Boolean {
  val leaves = ArrayDeque<EmulatedStability>()
  val stack = ArrayDeque<EmulatedStability>()
  stack.add(this)
  while (stack.isNotEmpty()) {
    val next = stack.removeLast()
    if (next is EmulatedStability.Combined) stack.addAll(next.parts) else leaves.add(next)
  }
  if (leaves.any { it === EmulatedStability.Unstable || it === EmulatedStability.Other }) {
    return false
  }
  return this is EmulatedStability.Combined || leaves.none { it === EmulatedStability.Unknown }
}
