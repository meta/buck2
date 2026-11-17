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
import org.jetbrains.kotlin.backend.common.lower.createIrBuilder
import org.jetbrains.kotlin.descriptors.InlineClassRepresentation
import org.jetbrains.kotlin.ir.IrElement
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrDeclarationContainer
import org.jetbrains.kotlin.ir.declarations.IrDeclarationOrigin
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.declarations.IrParameterKind
import org.jetbrains.kotlin.ir.declarations.IrProperty
import org.jetbrains.kotlin.ir.declarations.IrSimpleFunction
import org.jetbrains.kotlin.ir.declarations.IrValueParameter
import org.jetbrains.kotlin.ir.expressions.impl.IrConstImpl
import org.jetbrains.kotlin.ir.symbols.IrClassSymbol
import org.jetbrains.kotlin.ir.symbols.UnsafeDuringIrConstructionAPI
import org.jetbrains.kotlin.ir.types.IrSimpleType
import org.jetbrains.kotlin.ir.types.IrType
import org.jetbrains.kotlin.ir.types.classOrNull
import org.jetbrains.kotlin.ir.types.defaultType
import org.jetbrains.kotlin.ir.types.isNullable
import org.jetbrains.kotlin.ir.types.isPrimitiveType
import org.jetbrains.kotlin.ir.types.makeNullable
import org.jetbrains.kotlin.ir.util.constructors
import org.jetbrains.kotlin.ir.util.hasAnnotation
import org.jetbrains.kotlin.ir.util.isPublishedApi
import org.jetbrains.kotlin.load.java.JvmAbi
import org.jetbrains.kotlin.name.ClassId
import org.jetbrains.kotlin.name.FqName
import org.jetbrains.kotlin.name.Name

/**
 * Injects Compose synthetic parameters ($composer, $changed, $default) into @Composable functions.
 *
 * For every @Composable function (excluding constructors and expect functions), this adds:
 * - `$composer: Composer?` — the Composer instance threaded through composable calls
 * - `$changed: Int` (1 or more) — bitmask tracking parameter change state
 * - `$default: Int` (0 or more) — bitmask for default parameter values, only on a concrete function
 *   that has defaults
 *
 * It also clears the parameters' default values, as Compose does, and like Compose makes each
 * parameter with a default able to take the placeholder passed for "use the default".
 *
 * Parameter counting follows the Compose compiler spec:
 * - changedParamCount = max(1, ceil((realValueParams + thisParams) / SLOTS_PER_INT))
 * - defaultParamCount = ceil(valueParams / BITS_PER_DEFAULT_INT), only if the function is concrete
 *   and any param has a default
 * - thisParams = count of dispatch receiver + extension receiver (context receivers are value
 *   params)
 */
@OptIn(UnsafeDuringIrConstructionAPI::class)
internal class ComposerParamInjector(private val pluginContext: IrPluginContext) {

  companion object {
    val COMPOSABLE_FQ_NAME = FqName("androidx.compose.runtime.Composable")
    private val COMPOSER_CLASS_ID =
        ClassId(FqName("androidx.compose.runtime"), Name.identifier("Composer"))
    private val JVM_NAME_CLASS_ID = ClassId(FqName("kotlin.jvm"), Name.identifier("JvmName"))
    private val JVM_NAME_FQ_NAME = FqName("kotlin.jvm.JvmName")

    /** Each parameter uses 3 bits in the $changed bitmask. 32 / 3 = 10 slots per Int. */
    private const val SLOTS_PER_INT = 10

    /** Each $default Int can track 31 parameters (bit 31 is sign bit, unusable). */
    private const val BITS_PER_DEFAULT_INT = 31
  }

  /** The Composer type, resolved lazily. Nullable if Compose runtime is not on classpath. */
  private val composerType: IrType? by lazy {
    pluginContext.referenceClass(COMPOSER_CLASS_ID)?.defaultType?.makeNullable()
  }

  private val jvmNameClass: IrClassSymbol? by lazy {
    pluginContext.referenceClass(JVM_NAME_CLASS_ID)
  }

  private val pendingStubs = mutableListOf<IrSimpleFunction>()

  /** Run the transform on the entire module. */
  fun transform(moduleFragment: IrModuleFragment) {
    val composerIrType = composerType ?: return // Compose runtime not on classpath; skip.
    moduleFragment.accept(
        ComposerParamVisitor(pluginContext, composerIrType),
        null,
    )
    // Added once the walk is over, so it never iterates a container it is adding to.
    pendingStubs.forEach { (it.parent as IrDeclarationContainer).declarations += it }
    // This must run after every in-module composable has been transformed, so links between two
    // transformed declarations are preserved while stale links to dependency declarations are not.
    moduleFragment.accept(OverrideLinkRepairVisitor(), null)
  }

  private inner class ComposerParamVisitor(
      private val pluginContext: IrPluginContext,
      private val composerIrType: IrType,
  ) : IrElementVisitorVoidCompat() {

    override fun visitElement(element: IrElement) {
      element.acceptChildren(this, null)
    }

    override fun visitSimpleFunction(declaration: IrSimpleFunction) {
      if (shouldTransform(declaration)) {
        injectParams(declaration)
      }
      // Continue visiting nested declarations (local functions inside composables).
      super.visitSimpleFunction(declaration)
    }

    override fun visitClass(declaration: IrClass) {
      // Visit class members.
      super.visitClass(declaration)
    }

    override fun visitProperty(declaration: IrProperty) {
      // Property getters can be @Composable.
      declaration.getter?.let { getter ->
        if (shouldTransform(getter)) {
          injectParams(getter)
        }
      }
      super.visitProperty(declaration)
    }

    /**
     * Determine if a function should be transformed.
     *
     * Transform if:
     * - Has @Composable annotation
     * - Is not a constructor
     * - Is not an expect declaration
     */
    private fun shouldTransform(function: IrSimpleFunction): Boolean {
      if (!function.hasAnnotation(COMPOSABLE_FQ_NAME)) return false
      if (function.isExpect) return false
      // Idempotence guard. A @Composable property getter is reached TWICE: once explicitly via
      // visitProperty below, and again when super.visitProperty descends into the getter as an
      // IrSimpleFunction. Without this check the synthetic params are injected twice, producing
      // (..., Composer, int, Composer, int) -- a descriptor no consumer can link against. Only
      // fires when @Composable is on the getter rather than the property, which is why it is rare.
      if (function.compatValueParameters.any { it.name.asString() == "\$composer" }) return false
      return true
    }

    /**
     * Inject $composer, $changed, and $default parameters into a @Composable function.
     *
     * Parameters are appended after all existing value parameters.
     */
    private fun injectParams(function: IrSimpleFunction) {
      val existingValueParams = function.compatValueParameters

      // Count "this" params: dispatch receiver + extension receiver.
      // These contribute to $changed slot allocation but are not value parameters.
      val thisParamCount =
          (if (function.dispatchReceiverParameter != null) 1 else 0) +
              (if (function.parameters.any { it.kind == IrParameterKind.ExtensionReceiver }) 1
              else 0)

      val realValueParamCount = existingValueParams.size
      val totalSlottedParams = realValueParamCount + thisParamCount

      // Deliberately no "+1": the force bit shares int 0 with slots 0..9 rather than consuming a
      // slot, and adding one emits a trailing `I` at every exact multiple of SLOTS_PER_INT.
      val changedCount = maxOf(1, ceilDiv(totalSlottedParams, SLOTS_PER_INT))

      // An abstract @Composable member gets no $default: Compose puts defaults handling in a
      // nested ComposeDefaultImpls, so injecting it here adds a trailing `I` and changes the
      // value-class mangled name, leaving a cross-target implementer overriding a signature the
      // real ABI does not have. Keyed on having a body, not on modality, so bodiless fake
      // overrides are treated alike.
      val hasBody = function.body != null
      val hasDefaults = existingValueParams.any { it.defaultValue != null }
      val defaultCount =
          if (hasDefaults && hasBody) ceilDiv(realValueParamCount, BITS_PER_DEFAULT_INT) else 0
      // Compose retypes each parameter that has a default, its own or an overridden function's, so
      // it can carry the placeholder passed for "use the default" (ComposerParamTransformer:
      // `param.type = param.type.defaultParameterType()`), once the synthetic parameters are in.
      // From 2.5 only a function that takes the `$default` mask is retyped. The defaults of a
      // virtual function were already moved into ComposeDefaultImpls, as Compose also does first.
      val retyped =
          if (COMPOSE_RETYPES_ONLY_WITH_DEFAULT_MASK && defaultCount == 0) emptyList()
          else function.parameters.indices.filter { hasDefaultForParam(function, it) }
      // Compose clears these once it has moved the defaults into the body or into
      // ComposeDefaultImpls, so the backend generates no `<fn>$default` stub for a composable.
      existingValueParams.forEach { it.defaultValue = null }

      // Appended after the existing parameters: $composer, $changed[N], $default[N].
      val injectedParams = mutableListOf<IrValueParameter>()
      injectedParams.add(createInjectedParam(function, "\$composer", composerIrType))
      for (i in 0 until changedCount) {
        val name = if (i == 0) "\$changed" else "\$changed$i"
        injectedParams.add(createInjectedParam(function, name, pluginContext.irBuiltIns.intType))
      }
      for (i in 0 until defaultCount) {
        val name = if (i == 0) "\$default" else "\$default$i"
        injectedParams.add(createInjectedParam(function, name, pluginContext.irBuiltIns.intType))
      }

      function.parameters = function.parameters + injectedParams
      // ComposerParamTransformer.makeValueClassNonPrimitiveStub: retyping a value class over a
      // reference type changes the mangled JVM name, so a public composable keeps a stub with the
      // parameter types it had before.
      if (
          (function.visibility.isPublicAPI || function.isPublishedApi()) &&
              retyped.any { isNonNullValueClassOverReference(function.parameters[it].type) }
      ) {
        pendingStubs += defaultParamStub(pluginContext, function)
      }
      retyped.forEach {
        function.parameters[it].type = defaultParameterType(function.parameters[it].type)
      }
      stampAccessorJvmName(function)
    }

    private fun createInjectedParam(
        function: IrSimpleFunction,
        name: String,
        type: IrType,
    ): IrValueParameter =
        pluginContext.irFactory
            .createValueParameter(
                startOffset = -1,
                endOffset = -1,
                origin = IrDeclarationOrigin.DEFINED,
                kind = IrParameterKind.Regular,
                name = Name.identifier(name),
                type = type,
                isAssignable = false,
                symbol = org.jetbrains.kotlin.ir.symbols.impl.IrValueParameterSymbolImpl(),
                varargElementType = null,
                isCrossinline = false,
                isNoinline = false,
                isHidden = false,
            )
            .also { it.parent = function }

    // A @Composable PROPERTY ACCESSOR must keep the plain JVM name of the accessor.
    //
    // ComposerParamTransformer.copyWithComposerParam stamps @JvmName(getterName(property)) on every
    // accessor it rewrites, and @JvmName suppresses both the value-class mangle and the $module
    // suffix the JVM backend would otherwise apply. Injecting the synthetic params in place leaves
    // neither suppressed, so the stub advertises a name the real jar does not have. Measured on
    // MdsButton$Type:
    //
    //   real: getMinHeight(Composer, int)F
    //   stub: getMinHeight-chRvn1I$fbandroid_java_com_facebook_mds_compose_button_button(
    //             Composer, int)F
    //
    // A consumer compiled against the stub links the mangled name -> NoSuchMethodError. A
    // @Composable FUNCTION has no corresponding property, gets no @JvmName from the real
    // transformer either, and must keep both manglings -- which is why this is scoped to accessors.
    private fun stampAccessorJvmName(function: IrSimpleFunction) {
      val property = function.correspondingPropertySymbol?.owner ?: return
      if (function.hasAnnotation(JVM_NAME_FQ_NAME)) return
      val constructor = jvmNameClass?.constructors?.singleOrNull() ?: return
      val propertyName = property.name.identifier
      val accessorName =
          if (property.setter == function) JvmAbi.setterName(propertyName)
          else JvmAbi.getterName(propertyName)
      val annotation =
          pluginContext.irBuiltIns.createIrBuilder(function.symbol).run {
            irAnnotationCompat(constructor, emptyList()).apply {
              arguments[0] =
                  IrConstImpl.string(-1, -1, pluginContext.irBuiltIns.stringType, accessorName)
            }
          }
      function.annotations = function.annotations + annotation
    }
  }

  /**
   * Removes override links whose source-level dependency declarations lack Compose's synthetic
   * parameters. The transformed JVM descriptor still matches the dependency's real Compose-lowered
   * descriptor, so no bridge is needed to preserve the override in bytecode.
   */
  private inner class OverrideLinkRepairVisitor : IrElementVisitorVoidCompat() {
    override fun visitElement(element: IrElement) {
      element.acceptChildren(this, null)
    }

    override fun visitSimpleFunction(declaration: IrSimpleFunction) {
      if (declaration.hasInjectedComposerParam() && declaration.overriddenSymbols.isNotEmpty()) {
        declaration.overriddenSymbols =
            declaration.overriddenSymbols.filter { it.owner.hasInjectedComposerParam() }
      }
      super.visitSimpleFunction(declaration)
    }
  }

  private fun IrSimpleFunction.hasInjectedComposerParam(): Boolean = compatValueParameters.any {
    it.name.asString() == "\$composer"
  }

  // ComposerParamTransformer.hasDefaultForParam: a regular parameter has a default when it
  // declares one or the parameter at its index in an overridden function with as many parameters
  // has one.
  private fun hasDefaultForParam(function: IrSimpleFunction, index: Int): Boolean {
    val param = function.parameters[index]
    if (param.kind != IrParameterKind.Regular) return false
    if (param.defaultValue != null) return true
    return function.overriddenSymbols.any {
      it.owner.parameters.size == function.parameters.size && hasDefaultForParam(it.owner, index)
    }
  }

  // AbstractComposeLowering.defaultParameterType: primitives and value classes over a primitive
  // keep their type; everything else becomes nullable.
  private fun defaultParameterType(type: IrType): IrType =
      if (type.isPrimitiveType() || unboxedValueClassType(type)?.isPrimitiveType() == true) type
      else type.makeNullable()

  private fun isNonNullValueClassOverReference(type: IrType): Boolean =
      unboxedValueClassType(type)?.let { !it.isPrimitiveType() && !it.isNullable() } == true

  private fun unboxedValueClassType(type: IrType): IrType? {
    if (type.isNullable()) return null
    val underlying =
        (type.classOrNull?.owner?.valueClassRepresentation
                as? InlineClassRepresentation<IrSimpleType>)
            ?.underlyingType ?: return null
    return unboxedValueClassType(underlying) ?: underlying
  }
}

/** Integer ceiling division: ceil(a / b) for positive a, b. */
private fun ceilDiv(a: Int, b: Int): Int = (a + b - 1) / b
