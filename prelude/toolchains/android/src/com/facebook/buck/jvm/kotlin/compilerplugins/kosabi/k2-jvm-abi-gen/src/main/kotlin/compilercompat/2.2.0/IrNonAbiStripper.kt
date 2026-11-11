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
    com.facebook.DirectDeclarationsAccessCompat::class,
)

package com.facebook

import com.facebook.buck.jvm.kotlin.compilerplugins.common.isStub
import org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension
import org.jetbrains.kotlin.backend.common.extensions.IrPluginContext
import org.jetbrains.kotlin.backend.common.lower.DeclarationIrBuilder
import org.jetbrains.kotlin.descriptors.ClassKind
import org.jetbrains.kotlin.descriptors.DescriptorVisibilities
import org.jetbrains.kotlin.descriptors.Modality
import org.jetbrains.kotlin.ir.IrBuiltIns
import org.jetbrains.kotlin.ir.IrElement
import org.jetbrains.kotlin.ir.IrStatement
import org.jetbrains.kotlin.ir.builders.irCallConstructor
import org.jetbrains.kotlin.ir.declarations.*
import org.jetbrains.kotlin.ir.expressions.*
import org.jetbrains.kotlin.ir.expressions.impl.IrConstImpl
import org.jetbrains.kotlin.ir.expressions.impl.IrReturnImpl
import org.jetbrains.kotlin.ir.symbols.IrClassSymbol
import org.jetbrains.kotlin.ir.symbols.IrReturnTargetSymbol
import org.jetbrains.kotlin.ir.symbols.UnsafeDuringIrConstructionAPI
import org.jetbrains.kotlin.ir.types.*
import org.jetbrains.kotlin.ir.util.constructors
import org.jetbrains.kotlin.ir.util.kotlinFqName
import org.jetbrains.kotlin.ir.util.parentAsClass
import org.jetbrains.kotlin.ir.util.primaryConstructor
import org.jetbrains.kotlin.name.Name
import org.jetbrains.kotlin.psi.KtFile

internal class NonAbiDeclarationsStrippingIrExtension(
    private val sourceFiles: List<KtFile>,
    private val repairLog: AbiGenRepairLog,
    private val keptInlineBodies: Set<IrSimpleFunction>,
) : IrGenerationExtension {

  private companion object {
    // ACC_SYNTHETIC (ClassCodegen, FunctionCodegen), ACC_STRICT and ACC_SYNCHRONIZED
    // (FunctionCodegen), and the DefaultImpls decision (InterfaceLowering, JvmCachedDeclarations).
    val BACKEND_READ_SOURCE_ANNOTATIONS = setOf(
        "kotlin.jvm.JvmSynthetic",
        "kotlin.jvm.Strictfp",
        "kotlin.jvm.Synchronized",
        "kotlin.jvm.JvmDefaultWithCompatibility",
        "kotlin.jvm.JvmDefaultWithoutCompatibility",
    )
  }

  private fun shouldStripAnnotation(annotation: IrConstructorCall): Boolean {
    val annotationClass = annotation.symbol.owner.parent as? IrClass ?: return false
    val annotationFqName = annotationClass.kotlinFqName.asString()

    // Keep @Throws annotation in IR to generate bytecode throws clause for Java interop.
    // The JVM backend (FunctionCodegen.getThrownExceptions) reads @Throws from IR annotations
    // to generate the method's throws clause. If we strip it here, Java code cannot catch
    // checked exceptions because the throws clause won't be in bytecode.
    // Note: K2 JVM backend writes @Throws to both throws clause AND RuntimeInvisibleAnnotations.
    // We strip it from RuntimeInvisibleAnnotations via bytecode post-processing (see
    // ThrowsAnnotationStripper) to match K1 behavior and prevent Safe Kotlin errors.
    if (annotationFqName == "kotlin.jvm.Throws" || annotationFqName == "kotlin.Throws") {
      return false
    }

    // Keep @Transient and @Volatile in IR: the JVM backend (ClassCodegen.computeFieldFlags)
    // derives ACC_TRANSIENT/ACC_VOLATILE from these IR annotations. The retention-based strip
    // below cannot be trusted for them: deserialized stdlib annotation classes (origin
    // IR_EXTERNAL_DECLARATION_STUB) report @Retention(SOURCE) for kotlin.jvm.Transient, so the
    // SOURCE check misfires and the field loses its flag (location-style compare failure:
    // access 1 vs 129). Proven by fixture compare/parcelableTransient.
    if (annotationFqName == "kotlin.jvm.Transient" || annotationFqName == "kotlin.jvm.Volatile") {
      return false
    }

    // Keep SOURCE-retention annotations that the JVM backend reads to shape bytecode; the
    // retention strip below would otherwise change the emitted ABI.
    if (annotationFqName in BACKEND_READ_SOURCE_ANNOTATIONS) {
      return false
    }

    // Strip SOURCE retention annotations (not needed in ABI)
    for (retentionAnnotation in annotationClass.annotations) {
      val retentionClass = retentionAnnotation.symbol.owner.parent as? IrClass ?: continue
      if (retentionClass.kotlinFqName.asString() == "kotlin.annotation.Retention") {
        if (retentionAnnotation.valueArgumentsCount > 0) {
          val arg = retentionAnnotation.getValueArgument(0)
          if (arg is IrGetEnumValue && arg.symbol.owner.name.asString() == "SOURCE") {
            return true
          }
        }
      }
    }
    return false
  }

  // Check if an annotation contains error types in its arguments.
  // Error types occur when K2 cannot resolve constants from source-only ABI dependencies.
  // These annotations cause crashes during constant evaluation, so we strip them.
  private fun hasErrorType(annotation: IrConstructorCall): Boolean {
    for (i in 0 until annotation.valueArgumentsCount) {
      val arg = annotation.getValueArgument(i)
      if (arg != null && containsErrorType(arg)) {
        return true
      }
    }
    return false
  }

  // Recursively check if an IR expression contains error types
  private fun containsErrorType(expression: IrExpression): Boolean {
    // Check if the expression's type is an error type
    if (expression.type is IrErrorType) {
      return true
    }

    // For class references (like IOException::class in @Throws), check if the referenced type
    // is an error type. This handles cases where @Throws has unresolved exception classes.
    if (expression is IrClassReference && expression.classType is IrErrorType) {
      return true
    }

    // For varargs (like @Throws(E1::class, E2::class)), check all elements
    if (expression is IrVararg) {
      for (element in expression.elements) {
        if (element is IrExpression && containsErrorType(element)) {
          return true
        }
      }
    }

    // Recursively check children
    var hasError = false
    expression.acceptChildren(
        object : IrElementVisitorVoidCompat() {
          override fun visitElement(element: IrElement) {
            if (element is IrExpression && element.type is IrErrorType) {
              hasError = true
            }
            // Also check class references in children
            if (element is IrClassReference && element.classType is IrErrorType) {
              hasError = true
            }
            element.acceptChildren(this, null)
          }
        },
        null,
    )
    return hasError
  }

  private fun stripSourceRetentionAnnotations(moduleFragment: IrModuleFragment) {
    moduleFragment.accept(
        object : IrElementVisitorVoidCompat() {
          override fun visitElement(element: IrElement) {
            element.acceptChildren(this, null)
          }

          override fun visitClass(declaration: IrClass) {
            declaration.annotations =
                declaration.annotations.filter { !shouldStripAnnotation(it) && !hasErrorType(it) }
            super.visitClass(declaration)
          }

          override fun visitSimpleFunction(declaration: IrSimpleFunction) {
            declaration.annotations =
                declaration.annotations.filter { !shouldStripAnnotation(it) && !hasErrorType(it) }
            declaration.valueParameters.forEach { param ->
              param.annotations =
                  param.annotations.filter { !shouldStripAnnotation(it) && !hasErrorType(it) }
            }
            super.visitSimpleFunction(declaration)
          }

          override fun visitField(declaration: IrField) {
            declaration.annotations =
                declaration.annotations.filter { !shouldStripAnnotation(it) && !hasErrorType(it) }
            super.visitField(declaration)
          }

          override fun visitProperty(declaration: IrProperty) {
            declaration.annotations =
                declaration.annotations.filter { !shouldStripAnnotation(it) && !hasErrorType(it) }
            super.visitProperty(declaration)
          }

          override fun visitConstructor(declaration: IrConstructor) {
            declaration.annotations =
                declaration.annotations.filter { !shouldStripAnnotation(it) && !hasErrorType(it) }
            declaration.valueParameters.forEach { param ->
              param.annotations =
                  param.annotations.filter { !shouldStripAnnotation(it) && !hasErrorType(it) }
            }
            super.visitConstructor(declaration)
          }
        },
        null,
    )
  }

  @OptIn(UnsafeDuringIrConstructionAPI::class)
  override fun generate(moduleFragment: IrModuleFragment, pluginContext: IrPluginContext) {
    // Filter out files generated from stubs, similar to K1 implementation
    val stubs = sourceFiles.filter { it.isStub() }
    val stubPaths: Set<String> = stubs.map { it.viewProvider.virtualFile.path }.toSet()

    // Remove IR files that were generated from stubs
    moduleFragment.files.removeAll { irFile -> stubPaths.contains(irFile.fileEntry.name) }

    // Remove IR files for plugin-generated declarations (e.g., "__GENERATED DECLARATIONS__")
    // These files contain stub declarations generated by our
    // MissingConstantDeclarationGenerationExtension
    // for FIR resolution of missing constants and transitive dependencies. They're temporary
    // artifacts
    // used during FIR analysis and should not be included in the final bytecode.
    // Note: This filtering preserves legitimate plugin-generated methods like Parcelize's
    // describeContents()/writeToParcel() which are generated directly in the class.
    moduleFragment.files.removeAll { irFile ->
      irFile.fileEntry.name.contains("__GENERATED DECLARATIONS__") ||
          irFile.fileEntry.name.contains("GENERATED_DECLARATIONS")
    }

    // Strip SOURCE retention annotations from all declarations.
    // SOURCE retention annotations (like @IntDef from androidx.annotation) should not appear
    // in bytecode, but K2 preserves them. K1 Kosabi naturally omits them because source stubs
    // don't carry meta-annotations. We strip them here at the IR level where we can inspect
    // the annotation class's @Retention policy.
    stripSourceRetentionAnnotations(moduleFragment)

    moduleFragment.transform(
        NonAbiDeclarationsStrippingIrVisitor(
            pluginContext.irFactory,
            pluginContext.irBuiltIns,
            pluginContext,
            repairLog,
            keptInlineBodies,
            collectMustKeepPrivateClassNames(moduleFragment),
        ),
        null,
    )
  }
}

@OptIn(UnsafeDuringIrConstructionAPI::class)
internal class NonAbiDeclarationsStrippingIrVisitor(
    private val irFactory: IrFactory,
    private val irBuiltins: IrBuiltIns,
    private val pluginContext: IrPluginContext,
    private val repairLog: AbiGenRepairLog,
    private val keptInlineBodies: Set<IrSimpleFunction>,
    private val mustKeepPrivateClassNames: Set<String>,
) : IrElementTransformerVoidCompat() {

  override fun visitFile(declaration: IrFile): IrFile {
    declaration.removeNonPublicApi()
    return super.visitFile(declaration)
  }

  override fun visitDeclaration(declaration: IrDeclarationBase): IrStatement {
    if (declaration is IrDeclarationContainer) {
      declaration.removeNonPublicApi()
    }
    return super.visitDeclaration(declaration)
  }

  override fun visitClass(declaration: IrClass): IrStatement {
    // Strip supertypes whose class file the ABI jar will not contain, since the reference would
    // dangle. Internal supertypes are kept because source-only ABI is consumed within the same
    // module, where internal types are accessible. Stripping them would cause Java consumers
    // to see "incompatible types" errors when a public class implements an internal interface.
    val strippedSupertypes =
        declaration.superTypes.filter { superType ->
          val superClass = superType.classOrNull?.owner ?: return@filter false
          isClassAbsentFromAbi(superClass)
        }

    declaration.superTypes =
        declaration.superTypes.filter { superType ->
          val superClass = superType.classOrNull?.owner ?: return@filter true
          !isClassAbsentFromAbi(superClass)
        }

    // For each stripped supertype that was an interface, convert fake override methods
    // that implement that interface to real methods with stub bodies.
    // This is needed because the JVM backend skips generating bytecode for fake overrides.
    if (strippedSupertypes.isNotEmpty()) {
      convertFakeOverridesFromStrippedSupertypes(declaration, strippedSupertypes)
    }

    // Also convert fake override methods that override non-public API methods.
    // This handles cases like enum entry classes where the fake override methods
    // override methods from the parent enum class that in turn implements a private interface.
    convertPublicFakeOverridesOfNonPublicMethods(declaration)

    return super.visitClass(declaration)
  }

  // Convert public fake override methods that ultimately override non-public methods.
  // This handles enum entry classes where getFlavor/getName are fake overrides that
  // override the parent enum class's abstract methods (which themselves implement
  // a stripped private interface).
  private fun convertPublicFakeOverridesOfNonPublicMethods(irClass: IrClass) {
    for (decl in irClass.declarations) {
      if (decl !is IrSimpleFunction) continue
      if (!decl.isFakeOverride) continue
      if (!decl.visibility.isPublicAPI) continue

      // Materialize only when the declaring class is gone from the ABI. If it survives -- any
      // nested private class does -- the fake override still resolves, and emitting a body here
      // would add a method the library jar does not have.
      val shouldMaterialize =
          decl.overriddenSymbols.any { overriddenSymbol ->
            val overriddenParent = overriddenSymbol.owner.parent as? IrClass
            overriddenParent != null && isClassAbsentFromAbi(overriddenParent)
          }

      if (shouldMaterialize) {
        materializeFakeOverride(decl)
      }
    }
  }

  // Convert fake override methods that came from stripped supertypes to real methods.
  // The JVM backend skips generating bytecode for fake overrides (isFakeOverride = true),
  // so when we strip a private interface from a public class, we need to convert the
  // fake override methods that implemented that interface to real methods with stub bodies.
  private fun convertFakeOverridesFromStrippedSupertypes(
      irClass: IrClass,
      strippedSupertypes: List<IrType>,
  ) {
    // Collect method signatures from stripped interfaces
    val strippedInterfaceSignatures = mutableSetOf<MethodSignature>()
    for (superType in strippedSupertypes) {
      val superClass = superType.classOrNull?.owner ?: continue
      if (superClass.kind != ClassKind.INTERFACE) continue
      for (decl in superClass.declarations) {
        if (decl is IrSimpleFunction && !decl.isFakeOverride) {
          strippedInterfaceSignatures.add(decl.methodSignature())
        }
      }
    }

    if (strippedInterfaceSignatures.isEmpty()) return

    // Materialize fake override methods matching stripped interface signatures
    materializeFakeOverridesMatching(irClass, strippedInterfaceSignatures)

    // For enum classes, also process enum entry classes
    // Enum entry classes (correspondingClass) inherit methods from the enum class, and when
    // a private interface is stripped from the enum class, those methods become orphaned
    // fake overrides in the enum entry classes too.
    if (irClass.kind == ClassKind.ENUM_CLASS) {
      for (enumEntry in irClass.declarations.filterIsInstance<IrEnumEntry>()) {
        val entryClass = enumEntry.correspondingClass ?: continue
        materializeFakeOverridesMatching(entryClass, strippedInterfaceSignatures)
      }
    }
  }

  // Convert a fake override to a real method with a stub body.
  private fun materializeFakeOverride(decl: IrSimpleFunction) {
    // Any's methods always resolve through the present Any supertype: materializing
    // them would add methods the library jar does not have (and consumers flag the
    // mismatch). Leave the fake, which the backend skips as in the class ABI.
    if (decl.isAnyMethod()) return
    decl.isFakeOverride = false
    // Keep the FIR modality: forcing FINAL breaks subclasses of open classes.
    decl.origin = IrDeclarationOrigin.DEFINED
    if (decl.body == null) {
      decl.body =
          generateDefaultBody(decl.returnType, decl.symbol) ?: irFactory.createBlockBody(-1, -1)
    }
  }

  // Find and materialize public fake override methods whose signature matches
  // a method from a stripped interface.
  private fun materializeFakeOverridesMatching(
      irClass: IrClass,
      signatures: Set<MethodSignature>,
  ) {
    for (decl in irClass.declarations) {
      if (decl !is IrSimpleFunction) continue
      if (!decl.isFakeOverride) continue
      if (!decl.visibility.isPublicAPI) continue
      if (decl.methodSignature() in signatures) {
        materializeFakeOverride(decl)
      }
    }
  }

  // True when removeNonPublicApi will drop this class from the ABI, which is the only reason a
  // supertype has to leave a supertype list. Only top-level private classes are dropped; a nested
  // private class is kept, so a `private sealed class`/`sealed interface` and everything nested
  // under it stays referenceable and must keep appearing as a supertype. Must-keep classes
  // stay, so hierarchies keep resolving; interfaces still strip even when kept, because
  // fake overrides of their members materialize instead.
  private fun isClassAbsentFromAbi(irClass: IrClass): Boolean {
    if (
        irClass.kind != ClassKind.INTERFACE &&
            mustKeepPrivateClassNames.contains(irClass.kotlinFqName.asString())
    ) {
      return false
    }
    var current: IrClass = irClass
    while (true) {
      if (current.visibility == DescriptorVisibilities.LOCAL) {
        return true
      }
      val outer = current.parent as? IrClass
      if (outer == null) {
        return current.visibility == DescriptorVisibilities.PRIVATE
      }
      current = outer
    }
  }

  override fun visitField(declaration: IrField): IrStatement {
    // For fields with initializers containing function calls,
    // replace the initializer with a default constant value.
    // This handles cases like: const val X = (10 * TimeConstants.MS_PER_SECOND).toInt()
    // where the initializer contains a function call that can't be evaluated at compile time
    // in source-only ABI mode.
    // We specifically check for IrCall to avoid replacing valid expressions like unary minus
    // (-1).
    val initializer = declaration.initializer
    if (initializer != null) {
      val expression = initializer.expression
      if (containsFunctionCalls(expression)) {
        // Replace with a default constant value based on the field type
        val defaultExpressionBody = generateDefaultExpressionBody(declaration.type)
        if (defaultExpressionBody != null) {
          declaration.initializer = defaultExpressionBody
          // For a non-const field this only affects the initializer, which is not part of the
          // ABI. For a const val it rewrites the ConstantValue attribute consumers inline.
          val ownerPrefix = (declaration.parent as? IrClass)?.kotlinFqName?.asString()?.plus(".")
          repairLog.recordReplacedFieldInitializer(
              (ownerPrefix ?: "") + declaration.name.asString(),
              "initializer contained a call that source-only ABI cannot evaluate; " +
                  "replaced with the default value for ${declaration.type.classFqName?.asString()}",
          )
        }
      }
    }
    return super.visitField(declaration)
  }

  // Check if an expression tree contains any function calls
  private fun containsFunctionCalls(expression: IrExpression): Boolean {
    if (expression is IrCall) return true
    var hasCall = false
    expression.acceptChildren(
        object : IrElementVisitorVoidCompat() {
          override fun visitElement(element: IrElement) {
            if (element is IrCall) hasCall = true
            element.acceptChildren(this, null)
          }
        },
        null,
    )
    return hasCall
  }

  // we shouldn't generate default values for constants becuase the values are getting inlined
  // TODO: fix it: T235115614
  private fun generateDefaultValue(type: IrType): IrExpression? {
    val constructedType = type as? IrSimpleType ?: return null

    // For primitive types, create a default constant value
    // The code handles properties like val DEFAULT_SHADOW_COLOR: Int = Color.argb(128, 0, 0, 0)
    // If we don't set a default value, compiler crashes because it expects a ConstExpression
    // Primitives are matched by classifier (isInt/isLong/...) rather than by instance equality with
    // irBuiltins: a value class deserialized from another module carries a non-canonical underlying
    // primitive type that is not `==` to irBuiltins.longType, and matching it by instance used to
    // drop through to the null branch -- storing ACONST_NULL into an unboxed primitive slot. The
    // predicates are non-null-only, so a nullable primitive (a boxed, reference slot) still falls
    // through to null, which is correct there.
    var defaultValue: IrExpression? =
        when {
          constructedType.isInt() -> IrConstImpl.int(-1, -1, irBuiltins.intType, 0)
          constructedType.isBoolean() -> IrConstImpl.boolean(-1, -1, irBuiltins.booleanType, false)
          constructedType.isString() -> IrConstImpl.string(-1, -1, irBuiltins.stringType, "")
          constructedType.isDouble() -> IrConstImpl.double(-1, -1, irBuiltins.doubleType, 0.0)
          constructedType.isFloat() -> IrConstImpl.float(-1, -1, irBuiltins.floatType, 0.0f)
          constructedType.isLong() -> IrConstImpl.long(-1, -1, irBuiltins.longType, 0L)
          constructedType.isChar() -> IrConstImpl.char(-1, -1, irBuiltins.charType, '\u0000')
          constructedType.isByte() -> IrConstImpl.byte(-1, -1, irBuiltins.byteType, 0)
          constructedType.isShort() -> IrConstImpl.short(-1, -1, irBuiltins.shortType, 0)
          else ->
              // For value/inline classes (e.g. Compose's `Color`, which wraps `ULong`/`long`) a
              // null default is wrong: the JVM slot is the unboxed primitive, so the
              // compiler-generated `<fn>$default` method would emit ACONST_NULL into e.g. a `long`
              // slot and fail bytecode verification ("Expected J, but found R"). Build a boxed
              // default from the underlying primitive so inline-class lowering unboxes it to the
              // correct primitive zero. Falls back to null for everything else.
              generateInlineClassDefaultValue(constructedType)
                  ?: IrConstImpl.constNull(
                      -1,
                      -1,
                      constructedType.makeNullable(),
                  ) // If we can't handle this type, return null
        }

    return defaultValue
  }

  // Build a default value for a non-null value/inline class over a primitive, whose `<fn>$default`
  // slot is the *unboxed* primitive. Returns null (leaving the caller's ACONST_NULL fallback) for a
  // non-value class or a nullable value class -- which is boxed, so its slot is a reference and
  // null
  // is correct.
  //
  // The underlying primitive is read from the class's inline-class representation, which survives
  // deserialization even when the value class's constructor does not: a value class reached through
  // a source-only-ABI dependency deserializes with zero constructors (e.g. WA
  // `TranscriptionStatus`,
  // `ctors=0`). When a constructor *is* materialized we invoke it, so inline-class lowering unboxes
  // the call to the primitive zero (ICONST_0/LCONST_0). When it is not, we emit the underlying
  // primitive directly -- the slot is already that primitive, so a bare zero verifies -- rather
  // than
  // dropping through to ACONST_NULL and failing bytecode verification ("Expected I, but found R").
  private fun generateInlineClassDefaultValue(type: IrSimpleType): IrExpression? {
    val irClass = type.classOrNull?.owner ?: return null
    if (!irClass.isValue) return null
    if (type.isNullable()) return null
    val constructor = irClass.primaryConstructor ?: irClass.constructors.singleOrNull()
    val underlyingType =
        irClass.inlineClassRepresentation?.underlyingType
            ?: constructor?.valueParameters?.singleOrNull()?.type
            ?: return null
    val underlyingDefault = generateDefaultValue(underlyingType) ?: return null
    return if (constructor != null && constructor.valueParameters.size == 1) {
      DeclarationIrBuilder(pluginContext, constructor.symbol)
          .irCallConstructor(constructor.symbol, emptyList())
          .apply { putValueArgument(0, underlyingDefault) }
    } else {
      underlyingDefault
    }
  }

  private fun IrDeclarationContainer.removeNonPublicApi() {
    // For inline/value classes, we need to keep the backing field even if it's private
    val inlineClassBackingFieldName =
        (this as? IrClass)?.inlineClassRepresentation?.underlyingPropertyName

    // Members of a private nested class are unreachable however they are declared: Kotlin
    // resolves the metadata-private owner, Java sees a non-public nested class. Class-abi
    // drops them (keeping the shell); effective visibility drops them here too. Value,
    // enum, enum-entry and annotation classes keep the old behavior: backend lowerings
    // read their members (value-class lowering crashes once its fake-override equals is
    // gone), and enum-entry overrides stay reachable through the enum type.
    val ownerIsPrivateNestedClass =
        this is IrClass &&
            parent is IrClass &&
            (visibility == DescriptorVisibilities.PRIVATE ||
                visibility == DescriptorVisibilities.LOCAL) &&
            inlineClassRepresentation == null &&
            kind != ClassKind.ENUM_CLASS &&
            kind != ClassKind.ENUM_ENTRY &&
            kind != ClassKind.ANNOTATION_CLASS

    // A top-level private class kept as a shell (a must-keep private class, or one
    // carrying JVM-public nests) needs the same member stripping as a private nested
    // class: its members are unreachable through the private owner, and keeping them leaks
    // `error/NonExistentClass` references into the ABI when their types don't resolve on
    // the ABI classpath. Nested classes still survive via the nested-class rule above, so
    // the shell and its nests stay while the members go.
    val ownerIsKeptPrivateShell =
        this is IrClass &&
            parent !is IrClass &&
            (visibility == DescriptorVisibilities.PRIVATE ||
                visibility == DescriptorVisibilities.LOCAL) &&
            isMustKeepPrivateClass() &&
            inlineClassRepresentation == null &&
            kind != ClassKind.ENUM_CLASS &&
            kind != ClassKind.ENUM_ENTRY &&
            kind != ClassKind.ANNOTATION_CLASS

    this.declarations.filterIsInstance<IrProperty>().forEach { property ->
      if (property.isDelegated) property.backingField = null
    }

    this.declarations.removeAll { declaration ->
      // Keep constructors (needed for instantiation)
      if (declaration is IrConstructor) return@removeAll false
      // Keep companion objects (may contain public members)
      if ((declaration as? IrClass)?.isCompanion == true) return@removeAll false

      // Keep nested/inner classes even if private. They generate separate .class files
      // referenced via the InnerClasses attribute, constant pool entries, and Kotlin
      // @Metadata. Stripping the class but keeping these references causes javac to fail
      // with "class file not found" errors. K1 Kosabi keeps them as stubs — their private
      // members are stripped recursively and method bodies are stubbed by the visitor.
      // Note: `this is IrClass` ensures this only applies to nested classes (parent is a
      // class). Top-level private classes (parent is IrFile) are still stripped.
      if (declaration is IrClass && this is IrClass) return@removeAll false

      // Keep backing field/property of inline/value classes (required even if private)
      if (declaration.isInlineClassBackingMember(inlineClassBackingFieldName))
          return@removeAll false

      if (declaration.isDelegateBackingField()) return@removeAll true

      // Keep synthetic declarations (generated by compiler)
      if (declaration.origin.isSynthetic) return@removeAll false

      // Members of a private nested class are unreachable even when declared public, with
      // three exemptions. Const and @JvmStatic/@JvmField members feed public statics on the
      // outer class that callers genuinely reach, so dropping their IR breaks consumers.
      // Fake overrides emit no bytecode but backend lowerings look some of them up, so
      // removing them crashes compilation instead of shrinking the ABI.
      // Private members go first, so the exemptions only ever keep non-private ones,
      // matching the general private-member rule below.
      if (ownerIsPrivateNestedClass || ownerIsKeptPrivateShell) {
        if (declaration.isPrivateOrLocal()) return@removeAll true
        if (declaration.origin == IrDeclarationOrigin.FAKE_OVERRIDE) return@removeAll false
        if ((declaration as? IrProperty)?.isConst == true) return@removeAll false
        if (declaration.hasJvmStaticOrJvmField()) return@removeAll false
        return@removeAll true
      }

      // A must-keep private class (named by kept sealed metadata or annotation literals) stays as a
      // stub.
      if (declaration.isMustKeepPrivateClass()) return@removeAll false

      // Remove private/local members only (NOT internal - K1 kept internal in ABI)
      declaration.isPrivateOrLocal()
    }
  }

  private fun IrDeclaration.isMustKeepPrivateClass(): Boolean {
    if (this !is IrClass) return false
    // A must-keep private class must stay (as a stub): dropping it breaks consumers.
    return mustKeepPrivateClassNames.contains(this.kotlinFqName.asString())
  }

  private fun IrSimpleFunction.isAnyMethod(): Boolean {
    if (
        name.asString() != "equals" &&
            name.asString() != "hashCode" &&
            name.asString() != "toString"
    ) {
      return false
    }
    if (name.asString() != "equals") return valueParameters.isEmpty()
    // An equals overload with a narrower parameter (equals(other: Foo)) does not
    // override Any.equals; only equals(other: Any?) skips materialization.
    val paramType = valueParameters.singleOrNull()?.type ?: return false
    return (paramType as? IrSimpleType)?.classifier?.let {
      (it as? IrClassSymbol)?.owner?.kotlinFqName?.asString()
    } == "kotlin.Any" && paramType.isNullable()
  }

  private fun IrDeclaration.isInlineClassBackingMember(backingFieldName: Name?): Boolean {
    if (backingFieldName == null) return false
    return (this is IrField || this is IrProperty) &&
        (this as IrDeclarationWithName).name == backingFieldName
  }

  private fun IrDeclaration.isPrivateOrLocal(): Boolean {
    val visibility = (this as? IrDeclarationWithVisibility)?.visibility
    return visibility == DescriptorVisibilities.PRIVATE ||
        visibility == DescriptorVisibilities.PRIVATE_TO_THIS ||
        visibility == DescriptorVisibilities.LOCAL
  }

  // @JvmField lands on the backing field and @get:/@set: use-site targets land on the
  // accessors, so a property consults all four holders.
  private fun IrDeclaration.hasJvmStaticOrJvmField(): Boolean =
      relatedAnnotationContainers().any { container ->
        container.annotations?.any {
          when (it.type.classFqName?.asString()) {
            "kotlin.jvm.JvmStatic",
            "kotlin.jvm.JvmField" -> true
            else -> false
          }
        } == true
      }

  private fun IrDeclaration.relatedAnnotationContainers(): List<IrAnnotationContainer> =
      when (this) {
        is IrProperty -> listOfNotNull<IrAnnotationContainer>(this, backingField, getter, setter)
        is IrAnnotationContainer -> listOf(this)
        else -> emptyList()
      }

  private fun generateDefaultReturnStatement(
      type: IrType,
      symbol: IrReturnTargetSymbol,
  ): IrReturn? {
    val defaultValue = generateDefaultValue(type) ?: return null
    return IrReturnImpl(-1, -1, irBuiltins.nothingType, symbol, defaultValue)
  }

  private fun generateDefaultBody(type: IrType, symbol: IrReturnTargetSymbol): IrBody? {
    val returnStatement = generateDefaultReturnStatement(type, symbol) ?: return null
    return irFactory.createBlockBody(-1, -1).apply { this.statements.add(returnStatement) }
  }

  private fun generateDefaultExpressionBody(type: IrType): IrExpressionBody? {
    val defaultValue = generateDefaultValue(type) ?: return null
    return irFactory.createExpressionBody(-1, -1, defaultValue)
  }

  // Replace each defaulted value parameter's default with a fabricated, type-correct default.
  // For source-only ABI only the presence and type of a default matter, not its value. This
  // turns a value-class-over-primitive default into a boxed constructor call, which inline-class
  // lowering unboxes to the primitive zero, rather than the ACONST_NULL codegen would otherwise
  // store into the synthetic `$default` overload's unboxed slot ("Expected I, but found R").
  // Shared by visitSimpleFunction and visitConstructor -- the latter is where the WA
  // TranscriptionViewModel.VmState `<init>$default` crash lived.
  private fun regenerateDefaultParameterValues(function: IrFunction) {
    function.valueParameters
        .filter { it.defaultValue != null }
        .forEach { parameter ->
          generateDefaultExpressionBody(parameter.type)?.let { parameter.defaultValue = it }
        }
  }

  override fun visitSimpleFunction(declaration: IrSimpleFunction): IrStatement {
    if (declaration in keptInlineBodies) {
      // Callers inline this body from the ABI: leave it, and the lambdas in it, unchanged.
      val body = declaration.body
      declaration.body = null
      return super.visitSimpleFunction(declaration).also { declaration.body = body }
    }
    // Members of a kept-but-dropped interface cannot carry implementations: the JVM
    // backend moves interface bodies to DefaultImpls and crashes on the emptied
    // method. Abstract them; no consumer can implement the private interface anyway.
    val ownerClass = declaration.parent as? IrClass
    if (
        ownerClass != null &&
            ownerClass.kind == ClassKind.INTERFACE &&
            ownerClass.isPrivateOrLocal() &&
            mustKeepPrivateClassNames.contains(ownerClass.kotlinFqName.asString()) &&
            !declaration.isFakeOverride &&
            declaration.modality != Modality.ABSTRACT
    ) {
      declaration.modality = Modality.ABSTRACT
      declaration.body = null
      return super.visitSimpleFunction(declaration)
    }
    if (!declaration.origin.isSynthetic) {
      if (declaration.parent is IrProperty) {
        // for properties we need to generate a default body
        // otherwise we get a crash in the compiler
        // handles properties likeval DEFAULT_SHADOW_COLOR: Int = Color.argb(128, 0, 0, 0)
        // we shouldn't generate default values for constants becuase the values are getting
        // inlined
        // TODO: fix it: T235115614
        val body = generateDefaultBody(declaration.returnType, declaration.symbol)
        declaration.body = body ?: irFactory.createBlockBody(-1, -1)
      } else {
        declaration.body = irFactory.createBlockBody(-1, -1)
      }
      // handles default values in functions like fun foo(x: Int = Something.SomeValue)
      regenerateDefaultParameterValues(declaration)
    }
    return super.visitSimpleFunction(declaration)
  }

  override fun visitConstructor(declaration: IrConstructor): IrStatement {
    // A defaulted value-class-over-primitive constructor parameter reaches the synthetic
    // `<init>$default` with an unboxed slot; regenerate its default as visitSimpleFunction does
    // so the slot gets the unboxed primitive zero rather than ACONST_NULL. Bodies are left to the
    // default traversal -- unlike 2.3.0, this compiler path needs no constructor body stubbing.
    // Skip annotation classes: their constructor parameter defaults ARE the ABI (the
    // `AnnotationDefault` attribute), so regenerating them drops the element default and breaks
    // consumers ("annotation @X is missing a default value for the element ...").
    if (declaration.parentAsClass.kind != ClassKind.ANNOTATION_CLASS) {
      regenerateDefaultParameterValues(declaration)
    }
    return super.visitConstructor(declaration)
  }

  override fun visitAnonymousInitializer(
      declaration: IrAnonymousInitializer,
  ): IrStatement {
    // we also need to strip bodies from init {} blocks
    declaration.body = irFactory.createBlockBody(-1, -1)
    return super.visitAnonymousInitializer(declaration)
  }
}
