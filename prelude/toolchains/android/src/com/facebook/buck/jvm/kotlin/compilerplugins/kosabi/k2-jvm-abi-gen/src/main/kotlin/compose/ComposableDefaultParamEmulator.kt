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
import org.jetbrains.kotlin.config.LanguageVersion
import org.jetbrains.kotlin.descriptors.DescriptorVisibilities
import org.jetbrains.kotlin.descriptors.Modality
import org.jetbrains.kotlin.ir.IrElement
import org.jetbrains.kotlin.ir.builders.declarations.buildClass
import org.jetbrains.kotlin.ir.builders.declarations.buildFun
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrDeclarationOrigin
import org.jetbrains.kotlin.ir.declarations.IrEnumEntry
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.declarations.IrParameterKind
import org.jetbrains.kotlin.ir.declarations.IrSimpleFunction
import org.jetbrains.kotlin.ir.expressions.IrAnnotation
import org.jetbrains.kotlin.ir.expressions.impl.IrConstImpl
import org.jetbrains.kotlin.ir.expressions.impl.IrGetEnumValueImpl
import org.jetbrains.kotlin.ir.symbols.UnsafeDuringIrConstructionAPI
import org.jetbrains.kotlin.ir.types.defaultType
import org.jetbrains.kotlin.ir.util.addChild
import org.jetbrains.kotlin.ir.util.constructors
import org.jetbrains.kotlin.ir.util.copyAnnotationsFrom
import org.jetbrains.kotlin.ir.util.copyParametersFrom
import org.jetbrains.kotlin.ir.util.createThisReceiverParameter
import org.jetbrains.kotlin.ir.util.hasAnnotation
import org.jetbrains.kotlin.ir.util.isPublishedApi
import org.jetbrains.kotlin.name.ClassId
import org.jetbrains.kotlin.name.FqName
import org.jetbrains.kotlin.name.Name

/**
 * Emulates the Compose compiler's `ComposableDefaultParamLowering` for a virtual @Composable
 * function with default parameters, first in its override chain:
 * - a static `<name>$default` wrapper in a nested `ComposeDefaultImpls` class, taking the value
 *   parameters, then the receivers, and keeping the defaults;
 * - the function loses its defaults, so it gets no `$default` parameter;
 * - an open function also gets Compose metadata, which tells consumers to call the wrapper, and, if
 *   it is public, a hidden synthetic stub with the shape older compilers called.
 *
 * Runs before [ComposerParamInjector], which then adds the Composer parameters to all three.
 */
@OptIn(UnsafeDuringIrConstructionAPI::class)
internal class ComposableDefaultParamEmulator(private val pluginContext: IrPluginContext) {

  private companion object {
    val DEFAULT_IMPLS = Name.identifier("ComposeDefaultImpls")
    const val COMPOSE_PLUGIN_ID = "androidx.compose.compiler.plugins.kotlin"
  }

  fun transform(moduleFragment: IrModuleFragment) {
    val functions = mutableListOf<IrSimpleFunction>()
    moduleFragment.accept(
        object : IrElementVisitorVoidCompat() {
          override fun visitElement(element: IrElement) {
            element.acceptChildren(this, null)
          }

          override fun visitSimpleFunction(declaration: IrSimpleFunction) {
            if (declaration.isVirtualComposableWithDefaults()) functions += declaration
            super.visitSimpleFunction(declaration)
          }
        },
        null,
    )
    functions.forEach(::emulate)
  }

  private fun IrSimpleFunction.isVirtualComposableWithDefaults(): Boolean =
      hasAnnotation(ComposerParamInjector.COMPOSABLE_FQ_NAME) &&
          !isExpect &&
          (modality == Modality.ABSTRACT || modality == Modality.OPEN) &&
          overriddenSymbols.isEmpty() &&
          parent is IrClass &&
          parameters.any { it.defaultValue != null }

  private fun emulate(function: IrSimpleFunction) {
    val parentClass = function.parent as IrClass
    defaultImpls(parentClass).addChild(wrapper(function))
    if (function.modality == Modality.OPEN && function.origin == IrDeclarationOrigin.DEFINED) {
      pluginContext.metadataDeclarationRegistrar.addCustomMetadataExtension(
          function,
          COMPOSE_PLUGIN_ID,
          byteArrayOf(
              LanguageVersion.LATEST_STABLE.major.toByte(),
              LanguageVersion.LATEST_STABLE.minor.toByte(),
          ),
      )
      if (function.visibility.isPublicAPI || function.isPublishedApi()) {
        parentClass.addChild(stub(function))
      }
    }
    function.parameters.forEach { it.defaultValue = null }
  }

  private fun defaultImpls(parent: IrClass): IrClass =
      parent.declarations.filterIsInstance<IrClass>().firstOrNull { it.name == DEFAULT_IMPLS }
          ?: pluginContext.irFactory
              .buildClass {
                startOffset = parent.startOffset
                endOffset = parent.endOffset
                name = DEFAULT_IMPLS
              }
              .also {
                parent.addChild(it)
                it.createThisReceiverParameter()
              }

  private fun wrapper(function: IrSimpleFunction): IrSimpleFunction =
      pluginContext.irFactory
          .buildFun {
            startOffset = function.startOffset
            endOffset = function.endOffset
            origin = function.origin
            name = Name.identifier("${function.name.asString()}\$default")
            visibility =
                if (function.visibility.isPublicAPI) DescriptorVisibilities.PUBLIC
                else function.visibility
            modality = Modality.FINAL
            returnType = function.returnType
          }
          .also { wrapper ->
            wrapper.parent = function.parent
            wrapper.copyAnnotationsFrom(function)
            wrapper.copyParametersFrom(function)
            val receiverCount =
                wrapper.parameters.indexOfFirst { it.kind == IrParameterKind.Regular }
            val receivers = wrapper.parameters.take(receiverCount)
            receivers.forEach { it.kind = IrParameterKind.Regular }
            wrapper.parameters = wrapper.parameters.drop(receiverCount) + receivers
            wrapper.body = pluginContext.irFactory.createBlockBody(-1, -1)
          }

  private fun stub(function: IrSimpleFunction): IrSimpleFunction =
      defaultParamStub(pluginContext, function)
}

private const val STUB_MESSAGE = "Binary compatibility stub for default parameters"
private val JVM_SYNTHETIC_CLASS_ID = ClassId(FqName("kotlin.jvm"), Name.identifier("JvmSynthetic"))
private val DEPRECATED_CLASS_ID = ClassId(FqName("kotlin"), Name.identifier("Deprecated"))
private val DEPRECATION_LEVEL_CLASS_ID =
    ClassId(FqName("kotlin"), Name.identifier("DeprecationLevel"))

/**
 * Compose's `makeStub`: a copy of [function] with its current parameters, hidden from source and
 * synthetic in bytecode, that keeps a binary-compatible entry point for callers compiled against an
 * earlier shape of the function.
 */
internal fun defaultParamStub(
    pluginContext: IrPluginContext,
    function: IrSimpleFunction,
): IrSimpleFunction =
    pluginContext.irFactory
        .buildFun {
          startOffset = function.startOffset
          endOffset = function.endOffset
          origin = function.origin
          name = function.name
          visibility = function.visibility
          modality = function.modality
          returnType = function.returnType
          isInline = function.isInline
          isSuspend = function.isSuspend
          isOperator = function.isOperator
          isInfix = function.isInfix
          isExternal = function.isExternal
          isTailrec = function.isTailrec
        }
        .also { stub ->
          stub.parent = function.parent
          stub.copyAnnotationsFrom(function)
          stub.copyParametersFrom(function)
          val hidden = listOfNotNull(
              jvmSynthetic(pluginContext, stub),
              hiddenDeprecated(pluginContext, stub),
          )
          stub.annotations =
              stub.annotations.filterNot { existing ->
                hidden.any { it.symbol == existing.symbol }
              } + hidden
          stub.body = pluginContext.irFactory.createBlockBody(-1, -1)
        }

private fun jvmSynthetic(
    pluginContext: IrPluginContext,
    function: IrSimpleFunction,
): IrAnnotation? {
  val constructor =
      pluginContext.referenceClass(JVM_SYNTHETIC_CLASS_ID)?.constructors?.firstOrNull()
          ?: return null
  return pluginContext.irBuiltIns.createIrBuilder(function.symbol).run {
    irAnnotationCompat(constructor, emptyList())
  }
}

private fun hiddenDeprecated(
    pluginContext: IrPluginContext,
    function: IrSimpleFunction,
): IrAnnotation? {
  val deprecated = pluginContext.referenceClass(DEPRECATED_CLASS_ID) ?: return null
  val level = pluginContext.referenceClass(DEPRECATION_LEVEL_CLASS_ID) ?: return null
  val hidden =
      level.owner.declarations.filterIsInstance<IrEnumEntry>().singleOrNull {
        it.name.asString() == "HIDDEN"
      } ?: return null
  val constructor = deprecated.constructors.firstOrNull { it.owner.isPrimary } ?: return null
  return pluginContext.irBuiltIns.createIrBuilder(function.symbol).run {
    irAnnotationCompat(constructor, emptyList()).apply {
      arguments[0] = IrConstImpl.string(-1, -1, pluginContext.irBuiltIns.stringType, STUB_MESSAGE)
      arguments[2] = IrGetEnumValueImpl(-1, -1, level.defaultType, hidden.symbol)
    }
  }
}
