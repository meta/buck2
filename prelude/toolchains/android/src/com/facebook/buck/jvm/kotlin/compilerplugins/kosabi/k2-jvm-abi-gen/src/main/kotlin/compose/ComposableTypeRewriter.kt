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

import com.facebook.kotlin.compilercompat.compatValueParameters
import org.jetbrains.kotlin.backend.common.extensions.IrPluginContext
import org.jetbrains.kotlin.ir.IrElement
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrConstructor
import org.jetbrains.kotlin.ir.declarations.IrDeclarationOrigin
import org.jetbrains.kotlin.ir.declarations.IrExternalPackageFragment
import org.jetbrains.kotlin.ir.declarations.IrField
import org.jetbrains.kotlin.ir.declarations.IrFunction
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.declarations.IrParameterKind
import org.jetbrains.kotlin.ir.declarations.IrProperty
import org.jetbrains.kotlin.ir.declarations.IrSimpleFunction
import org.jetbrains.kotlin.ir.expressions.IrCall
import org.jetbrains.kotlin.ir.expressions.IrConstructorCall
import org.jetbrains.kotlin.ir.expressions.IrDelegatingConstructorCall
import org.jetbrains.kotlin.ir.expressions.IrTypeOperator
import org.jetbrains.kotlin.ir.expressions.IrTypeOperatorCall
import org.jetbrains.kotlin.ir.types.IrSimpleType
import org.jetbrains.kotlin.ir.types.IrType
import org.jetbrains.kotlin.ir.types.classFqName
import org.jetbrains.kotlin.ir.types.classOrNull
import org.jetbrains.kotlin.ir.types.defaultType
import org.jetbrains.kotlin.ir.types.isMarkedNullable
import org.jetbrains.kotlin.ir.types.makeNullable
import org.jetbrains.kotlin.ir.types.typeOrNull
import org.jetbrains.kotlin.ir.types.typeWith
import org.jetbrains.kotlin.ir.util.getPackageFragment
import org.jetbrains.kotlin.ir.util.hasAnnotation
import org.jetbrains.kotlin.name.ClassId
import org.jetbrains.kotlin.name.FqName
import org.jetbrains.kotlin.name.Name

/**
 * Rewrites @Composable function types in IR declarations.
 *
 * A @Composable (P1, P2, ..., Pn) -> R function type is represented in IR as FunctionN(P1, P2, ...,
 * Pn, R) with a @Composable annotation on the type.
 *
 * The Compose compiler transforms this to Function(N + 1 + changedCount)(P1, ..., Pn, Composer,
 * Int, ..., R).
 *
 * This rewriter applies that transformation to function types appearing in:
 * - Value parameter types
 * - Return types
 * - Supertype declarations
 * - The signatures of the external functions that the module overrides or calls, as the real
 *   plugin's `ComposableTypeTransformer` remaps them (see [TypeRewriterVisitor.remapExternal])
 *
 * After rewriting, the @Composable annotation is removed from the type since the synthetic
 * parameters now encode the composable nature.
 */
internal class ComposableTypeRewriter(private val pluginContext: IrPluginContext) {

  companion object {
    private val COMPOSABLE_FQ_NAME = FqName("androidx.compose.runtime.Composable")
    private val COMPOSER_CLASS_ID =
        ClassId(FqName("androidx.compose.runtime"), Name.identifier("Composer"))
    private val FUNCTION_FQ_PREFIX = "kotlin.Function"

    /**
     * Under K2 the Compose plugin registers its own function-type *kinds*
     * (ComposeFirExtensions.registerKinds), so a `@Composable (P..) -> R` is classified as the
     * synthetic class `androidx.compose.runtime.internal.(K)ComposableFunctionN` and carries no
     * `@Composable` annotation on the type itself. Only deserialized dependency metadata still uses
     * the legacy `kotlin.FunctionN` + annotation form.
     */
    private const val COMPOSABLE_FUNCTION_FQ_PREFIX =
        "androidx.compose.runtime.internal.ComposableFunction"
    private const val K_COMPOSABLE_FUNCTION_FQ_PREFIX =
        "androidx.compose.runtime.internal.KComposableFunction"

    /** Each parameter uses 3 bits. 32 / 3 = 10 slots per Int. */
    private const val SLOTS_PER_INT = 10
  }

  private val composerType: IrType? by lazy {
    pluginContext.referenceClass(COMPOSER_CLASS_ID)?.defaultType?.makeNullable()
  }

  /** Run the type rewriting on the entire module. */
  fun transform(moduleFragment: IrModuleFragment) {
    val composerIrType = composerType ?: return
    moduleFragment.accept(TypeRewriterVisitor(composerIrType, declarations = true), null)
  }

  /**
   * Remap only the external functions that bodies call. Kept inline bodies are restored, and
   * stripped constructors get a synthesized delegation, after [transform] runs; their calls must
   * name the remapped signatures, as the real plugin's do.
   */
  fun transformCallSites(moduleFragment: IrModuleFragment) {
    val composerIrType = composerType ?: return
    moduleFragment.accept(TypeRewriterVisitor(composerIrType, declarations = false), null)
  }

  private inner class TypeRewriterVisitor(
      private val composerIrType: IrType,
      private val declarations: Boolean,
  ) : IrElementVisitorVoidCompat() {

    override fun visitElement(element: IrElement) {
      element.acceptChildren(this, null)
    }

    /** The external functions already remapped (`externalTransformedDecls` of the real plugin). */
    private val remappedExternals = mutableSetOf<IrFunction>()

    override fun visitSimpleFunction(declaration: IrSimpleFunction) {
      if (declarations) {
        rewriteFunctionSignatureTypes(declaration)
        remapOverriddenFunctionTypes(declaration)
      }
      super.visitSimpleFunction(declaration)
    }

    override fun visitCall(expression: IrCall) {
      val callee = expression.symbol.owner
      remapExternal(callee)
      if (isExternalFunction(callee)) {
        callee.correspondingPropertySymbol?.owner?.let { property ->
          property.getter?.let { remapExternal(it, always = true) }
          property.setter?.let { remapExternal(it, always = true) }
        }
      }
      super.visitCall(expression)
    }

    override fun visitConstructorCall(expression: IrConstructorCall) {
      remapExternal(expression.symbol.owner)
      super.visitConstructorCall(expression)
    }

    override fun visitDelegatingConstructorCall(expression: IrDelegatingConstructorCall) {
      remapExternal(expression.symbol.owner)
      super.visitDelegatingConstructorCall(expression)
    }

    override fun visitTypeOperator(expression: IrTypeOperatorCall) {
      if (expression.operator == IrTypeOperator.SAM_CONVERSION) {
        val samClass = expression.typeOperand.classOrNull
        if (
            samClass != null &&
                samClass.isBound &&
                samClass.owner.origin == IrDeclarationOrigin.IR_EXTERNAL_DECLARATION_STUB &&
                samClass.owner.isFun
        ) {
          samClass.owner.declarations.filterIsInstance<IrSimpleFunction>().forEach {
            remapExternal(it)
          }
        }
      }
      super.visitTypeOperator(expression)
    }

    /**
     * The real plugin's `ComposableTypeTransformer.remapOverriddenFunctionTypes`: every function
     * that [function] overrides, directly or transitively, has its signature types remapped when it
     * is external. kotlinc's bridges then see the lowered `FunctionN+2` types of the overridden
     * function instead of the synthetic `ComposableFunctionN` kind.
     */
    private fun remapOverriddenFunctionTypes(function: IrSimpleFunction) {
      for (symbol in function.overriddenSymbols) {
        if (!symbol.isBound) continue
        val overridden = symbol.owner
        remapExternal(overridden)
        remapOverriddenFunctionTypes(overridden)
      }
    }

    /**
     * The real plugin's `ComposableTypeTransformer.visitFunctionIfExternal`: an external function
     * (`isExternalFunction`) whose return or parameter types carry `@Composable`
     * (`needsComposableRemapping`) has every parameter type, its return type and its type
     * parameters' bounds remapped, once. [always] skips the `@Composable` check, as the real
     * plugin's transform of an external property's accessors does.
     */
    private fun remapExternal(function: IrFunction, always: Boolean = false) {
      if (!isExternalFunction(function)) return
      if (!always && !needsComposableRemapping(function)) return
      if (!remappedExternals.add(function)) return
      function.returnType = rewriteTypeIfComposable(function.returnType)
      for (parameter in function.parameters) {
        parameter.type = rewriteTypeIfComposable(parameter.type)
        parameter.varargElementType =
            parameter.varargElementType?.let { rewriteTypeIfComposable(it) }
      }
      for (typeParameter in function.typeParameters) {
        typeParameter.superTypes = typeParameter.superTypes.map { rewriteTypeIfComposable(it) }
      }
      if (function is IrSimpleFunction) remapOverriddenFunctionTypes(function)
    }

    /** `isExternalFunction` of the real plugin (AbstractComposeLowering.kt). */
    private fun isExternalFunction(function: IrFunction): Boolean =
        function.origin == IrDeclarationOrigin.IR_EXTERNAL_DECLARATION_STUB ||
            function.origin == IrDeclarationOrigin.FAKE_OVERRIDE &&
                function.getPackageFragment() is IrExternalPackageFragment

    /** `needsComposableRemapping` of the real plugin (ComposableTypeRemapper.kt). */
    private fun needsComposableRemapping(function: IrFunction): Boolean =
        containsComposableAnnotation(function.returnType) ||
            function.parameters.any { containsComposableAnnotation(it.type) }

    private fun containsComposableAnnotation(type: IrType?): Boolean {
      if (type == null) return false
      if (type.hasAnnotation(COMPOSABLE_FQ_NAME)) return true
      return type is IrSimpleType &&
          type.arguments.any { containsComposableAnnotation(it.typeOrNull) }
    }

    /**
     * Constructors need their own override: [IrConstructor] extends IrFunction but NOT
     * IrSimpleFunction, so [visitSimpleFunction] never sees one. Without this, a `class Foo(val
     * content: @Composable () -> Unit)` keeps ComposableFunction0 in its `<init>` descriptor while
     * its generated getter is rewritten correctly -- and every consumer of the class calls a
     * constructor.
     */
    override fun visitConstructor(declaration: IrConstructor) {
      if (declarations) rewriteFunctionSignatureTypes(declaration)
      super.visitConstructor(declaration)
    }

    override fun visitProperty(declaration: IrProperty) {
      if (declarations) {
        declaration.getter?.let { rewriteFunctionSignatureTypes(it) }
        declaration.setter?.let { rewriteFunctionSignatureTypes(it) }
      }
      super.visitProperty(declaration)
    }

    /**
     * Backing fields are not reached via [visitSimpleFunction] or [visitProperty] either. These are
     * private and so do not affect consumers, but leaving the synthetic ComposableFunctionN type in
     * the stub jar leaves a dangling reference to a class that exists only inside a Compose-enabled
     * compilation.
     */
    override fun visitField(declaration: IrField) {
      val rewritten = rewriteTypeIfComposable(declaration.type)
      if (declarations && rewritten !== declaration.type) {
        declaration.type = rewritten
      }
      super.visitField(declaration)
    }

    override fun visitClass(declaration: IrClass) {
      // Rewrite @Composable function types in supertypes.
      if (declarations) {
        declaration.superTypes = declaration.superTypes.map { rewriteTypeIfComposable(it) }
      }
      super.visitClass(declaration)
    }

    /** Rewrite composable function types in a function's value parameters and return type. */
    private fun rewriteFunctionSignatureTypes(function: IrFunction) {
      // Rewrite parameter types.
      for (param in function.compatValueParameters) {
        val rewritten = rewriteTypeIfComposable(param.type)
        if (rewritten !== param.type) {
          param.type = rewritten
        }
      }

      // Rewrite return type.
      val rewrittenReturn = rewriteTypeIfComposable(function.returnType)
      if (rewrittenReturn !== function.returnType) {
        function.returnType = rewrittenReturn
      }

      // Rewrite extension receiver type.
      function.parameters
          .firstOrNull { it.kind == IrParameterKind.ExtensionReceiver }
          ?.let { ext ->
            val rewritten = rewriteTypeIfComposable(ext.type)
            if (rewritten !== ext.type) {
              ext.type = rewritten
            }
          }
    }

    /**
     * If [type] is a @Composable function type, rewrite it to include Composer + $changed params.
     * Otherwise, recursively check type arguments for nested composable function types.
     */
    private fun rewriteTypeIfComposable(type: IrType): IrType {
      if (type !is IrSimpleType) return type

      // Check if this is a @Composable function type.
      if (isComposableFunctionType(type)) {
        return rewriteComposableFunctionType(type)
      }

      // Recursively rewrite type arguments (handles e.g. List<@Composable () -> Unit>).
      var anyArgChanged = false
      val rewrittenArgTypes =
          type.arguments.map { arg ->
            if (arg is org.jetbrains.kotlin.ir.types.IrTypeProjection) {
              val rewritten = rewriteTypeIfComposable(arg.type)
              if (rewritten !== arg.type) {
                anyArgChanged = true
                rewritten
              } else {
                arg.type
              }
            } else {
              pluginContext.irBuiltIns.anyNType
            }
          }

      if (anyArgChanged) {
        return type.classOrNull?.typeWith(rewrittenArgTypes) ?: type
      }

      return type
    }

    /**
     * Two representations reach IR: the K2 kind `(K)ComposableFunctionN` with no annotation
     * (in-module source), and `kotlin.FunctionN` plus `@Composable` (read back from metadata).
     */
    private fun isComposableFunctionType(type: IrSimpleType): Boolean {
      if (isSyntheticComposableFunction(type) || isKComposableFunction(type)) return true
      if (!type.hasAnnotation(COMPOSABLE_FQ_NAME)) return false
      val classFqn = type.classFqName?.asString() ?: return false
      return classFqn.startsWith(FUNCTION_FQ_PREFIX)
    }

    /** The non-reflect Compose kind, `androidx.compose.runtime.internal.ComposableFunctionN`. */
    private fun isSyntheticComposableFunction(type: IrSimpleType): Boolean =
        hasNumberedPrefix(type, COMPOSABLE_FUNCTION_FQ_PREFIX)

    /** The reflect Compose kind, `androidx.compose.runtime.internal.KComposableFunctionN`. */
    private fun isKComposableFunction(type: IrSimpleType): Boolean =
        hasNumberedPrefix(type, K_COMPOSABLE_FUNCTION_FQ_PREFIX)

    /**
     * These classes are synthetic (declared nowhere) and unbounded in arity, so match on prefix +
     * an all-digit suffix rather than enumerating. The two prefixes are disjoint:
     * `...internal.KComposableFunction0` does not start with `...internal.ComposableFunction`.
     */
    private fun hasNumberedPrefix(type: IrSimpleType, prefix: String): Boolean {
      val fqn = type.classFqName?.asString() ?: return false
      if (!fqn.startsWith(prefix)) return false
      val suffix = fqn.substring(prefix.length)
      return suffix.isNotEmpty() && suffix.all { it.isDigit() }
    }

    // Rewrite a @Composable function type.
    //
    // @Composable FunctionN(P1, ..., Pn, R) becomes
    // Function(N + 1 + changedCount)(P1, ..., Pn, Composer, Int, ..., R)
    //
    // The @Composable annotation is removed (the synthetic params encode composability).
    private fun rewriteComposableFunctionType(type: IrSimpleType): IrType {
      val args = type.arguments
      if (args.isEmpty()) return type

      // FunctionN<P1, ..., Pn, R>: last type arg is the return type, rest are params.
      val paramTypeArgs = args.dropLast(1)
      val returnTypeArg = args.last()

      // Number of user-visible params in the function type (no receivers in function types).
      val userParamCount = paramTypeArgs.size

      // $changed count: ceil((userParamCount + 1) / SLOTS_PER_INT), minimum 1.
      val changedCount = maxOf(1, ceilDiv(userParamCount + 1, SLOTS_PER_INT))

      // New arity: original params + Composer + changedCount Ints.
      val newArity = userParamCount + 1 + changedCount

      // Build new type arguments: [P1, ..., Pn, Composer, Int, ..., R]
      // Recurse: a composable lambda nested inside another composable lambda's arguments is
      // itself a composable function type and must be rewritten too. rewriteTypeIfComposable
      // returns early on the composable branch, so this is the only place that reaches them.
      val newTypeArgs = mutableListOf<IrType>()
      for (arg in paramTypeArgs) {
        newTypeArgs.add(rewriteTypeIfComposable(extractTypeFromArgument(arg)))
      }
      // Add Composer? type argument.
      newTypeArgs.add(composerIrType)
      // Add Int type arguments for $changed params.
      repeat(changedCount) { newTypeArgs.add(pluginContext.irBuiltIns.intType) }
      // Add return type.
      newTypeArgs.add(rewriteTypeIfComposable(extractTypeFromArgument(returnTypeArg)))

      // irBuiltIns.functionN/kFunctionN cannot fail, unlike referenceClass, whose `?: return type`
      // silently left the type un-rewritten when the synthetic class was not in the symbol table.
      val functionClass =
          if (isKComposableFunction(type)) pluginContext.irBuiltIns.kFunctionN(newArity)
          else pluginContext.irBuiltIns.functionN(newArity)

      // typeWith() constructs NOT_SPECIFIED, so a declared-nullable type would come back non-null
      // and be emitted @NotNull; Java interop and NullAway read those JVM annotations.
      val rewritten = functionClass.typeWith(newTypeArgs)
      return if (type.isMarkedNullable()) rewritten.makeNullable() else rewritten
    }

    private fun extractTypeFromArgument(
        arg: org.jetbrains.kotlin.ir.types.IrTypeArgument,
    ): IrType {
      return when (arg) {
        is org.jetbrains.kotlin.ir.types.IrTypeProjection -> arg.type
        else -> pluginContext.irBuiltIns.anyNType
      }
    }
  }
}

/** Integer ceiling division: ceil(a / b) for positive a, b. */
private fun ceilDiv(a: Int, b: Int): Int = (a + b - 1) / b
