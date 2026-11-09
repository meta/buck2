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
@file:OptIn(org.jetbrains.kotlin.fir.declarations.DirectDeclarationsAccess::class)

package com.facebook

import com.facebook.buck.jvm.kotlin.compilerplugins.common.InlineBodyKeepability
import com.facebook.buck.jvm.kotlin.compilerplugins.common.isStub
import java.lang.reflect.InvocationTargetException
import java.lang.reflect.Modifier
import java.util.Collections
import java.util.IdentityHashMap
import org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension
import org.jetbrains.kotlin.backend.common.extensions.IrPluginContext
import org.jetbrains.kotlin.cli.common.messages.CompilerMessageSeverity
import org.jetbrains.kotlin.cli.common.messages.MessageCollector
import org.jetbrains.kotlin.config.KotlinCompilerVersion
import org.jetbrains.kotlin.fir.backend.Fir2IrComponentsStorage
import org.jetbrains.kotlin.fir.backend.Fir2IrConfiguration
import org.jetbrains.kotlin.fir.backend.Fir2IrConversionScope
import org.jetbrains.kotlin.fir.backend.Fir2IrDeclarationStorage
import org.jetbrains.kotlin.fir.backend.Fir2IrPluginContext
import org.jetbrains.kotlin.fir.backend.Fir2IrVisitor
import org.jetbrains.kotlin.fir.backend.FirMetadataSource
import org.jetbrains.kotlin.fir.declarations.FirDeclaration
import org.jetbrains.kotlin.fir.declarations.FirFunction
import org.jetbrains.kotlin.fir.declarations.FirProperty
import org.jetbrains.kotlin.fir.declarations.FirPropertyAccessor
import org.jetbrains.kotlin.fir.declarations.FirRegularClass
import org.jetbrains.kotlin.fir.expressions.FirBlock
import org.jetbrains.kotlin.fir.symbols.impl.FirPropertyAccessorSymbol
import org.jetbrains.kotlin.ir.IrElement
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrDeclaration
import org.jetbrains.kotlin.ir.declarations.IrDeclarationParent
import org.jetbrains.kotlin.ir.declarations.IrFunction
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.declarations.IrSimpleFunction
import org.jetbrains.kotlin.ir.expressions.IrBlockBody
import org.jetbrains.kotlin.ir.expressions.IrErrorExpression
import org.jetbrains.kotlin.psi.KtFile

/**
 * Keeps the bodies of non-private inline functions that resolve without Kosabi stubs and that the
 * ABI can carry as plain method bodies (see [InlineBodyKeepability]).
 *
 * Callers inline the body they find in the ABI jar. A kept body is converted to IR after the
 * body-skipping conversion, left alone by the IR stripper and generated as real code. Any other
 * non-private inline body fails the compilation; see [skipBody]. When no function is kept, every
 * later phase runs exactly as before.
 */
internal class InlineBodyStage : AbiGenStage {
  override val name = "InlineBodies"

  val keptFirFunctions: MutableSet<FirFunction> = Collections.newSetFromMap(IdentityHashMap())

  /** The IR functions whose bodies were restored; the IR stripper leaves exactly these alone. */
  val restoredIrFunctions: MutableSet<IrSimpleFunction> =
      Collections.newSetFromMap(IdentityHashMap())

  /** Runs the rule on every own source file. Must run before any FIR cleanup. */
  fun decide(
      analysisResults: FrontendOutputCompat,
      sourceFiles: List<KtFile>,
      messageCollector: MessageCollector,
      pluginTriggerAnnotations: Set<String> = emptySet(),
  ) {
    val stubPaths =
        sourceFiles.filter { it.isStub() }.map { it.viewProvider.virtualFile.path }.toSet()
    for (output in analysisResults.outputs) {
      // Stub declarations stand in for dependencies with guessed signatures, so they never count.
      val rule = InlineBodyKeepability(
          output.session,
          lowerableOnly = true,
          isStubFile = { it.sourceFile?.path in stubPaths },
          isLibraryDeclarationAllowed = { _, _ -> true },
          pluginTriggerAnnotations = pluginTriggerAnnotations,
      )
      for (firFile in output.fir) {
        if (firFile.sourceFile?.path in stubPaths) continue
        forEachFunction(firFile.declarations) {
          when {
            !rule.isCandidate(it) -> {}
            rule.isKeepable(it) -> keptFirFunctions += it
            else ->
                skipBody(
                    messageCollector,
                    functionDisplayName(it),
                    rule.keepRejectReason(it) ?: "unresolved or not lowerable",
                )
          }
        }
      }
    }
  }

  private fun forEachFunction(declarations: List<FirDeclaration>, action: (FirFunction) -> Unit) {
    for (declaration in declarations) {
      when (declaration) {
        is FirFunction -> action(declaration)
        // Property accessors are functions in FIR but not members of the declaration list:
        // without this, inline accessors are silently stripped to default stubs.
        is FirProperty -> {
          declaration.getter?.let(action)
          declaration.setter?.let(action)
        }
        is FirRegularClass -> forEachFunction(declaration.declarations, action)
        else -> {}
      }
    }
  }

  // Accessor symbols carry no callable id; name them through their property.
  private fun functionDisplayName(function: FirFunction): String {
    function.symbol.callableId?.let {
      return "$it"
    }
    val property =
        (function as? FirPropertyAccessor)?.propertySymbol?.callableId ?: return "<anonymous>"
    val accessor =
        if ((function.symbol as? FirPropertyAccessorSymbol)?.isGetter != false) ".get" else ".set"
    return "$property$accessor"
  }

  fun createExtension(messageCollector: MessageCollector): IrGenerationExtension =
      object : IrGenerationExtension {
        override fun generate(moduleFragment: IrModuleFragment, pluginContext: IrPluginContext) {
          moduleFragment.acceptChildren(
              object : IrElementVisitorVoidCompat() {
                override fun visitElement(element: IrElement) {
                  element.acceptChildren(this, null)
                }

                override fun visitSimpleFunction(declaration: IrSimpleFunction) {
                  restore(declaration, pluginContext, messageCollector)
                }
              },
              null,
          )
        }
      }

  private fun restore(
      function: IrSimpleFunction,
      pluginContext: IrPluginContext,
      messageCollector: MessageCollector,
  ) {
    val firFunction = firFunctionOf(function) ?: return
    if (firFunction !in keptFirFunctions) return
    val name = "${firFunction.symbol.callableId}"
    val body =
        try {
          convertBody(pluginContext, function, firFunction)
        } catch (e: Throwable) {
          val cause = (e as? InvocationTargetException)?.targetException ?: e
          when {
            isKosabiCompilerInternalsFailure(e) ->
                reportCompilerInternalsFailure(messageCollector, cause)
            e is Exception -> skipBody(messageCollector, name, "converting it to IR failed: $cause")
            else -> throw e
          }
          return
        }
    var hasError = false
    body.acceptChildren(
        object : IrElementVisitorVoidCompat() {
          override fun visitElement(element: IrElement) {
            if (element is IrErrorExpression) hasError = true
            element.acceptChildren(this, null)
          }
        },
        null,
    )
    if (hasError) {
      skipBody(messageCollector, name, "its IR contains error expressions")
      return
    }
    function.body = body
    restoredIrFunctions += function
  }

  private fun firFunctionOf(function: IrSimpleFunction): FirFunction? =
      (function.metadata as? FirMetadataSource.Function)?.fir

  /**
   * Converts the FIR body of [irFunction] with the components of the module's fir2ir run, so it
   * refers to exactly the IR declarations the rest of the module was converted to. fir2ir ran with
   * `skipBodies`, which has no per-function exemption, and the members used here are not public
   * API; a member that a compiler version lacks fails the compilation (see
   * [isKosabiCompilerInternalsFailure]).
   */
  private fun convertBody(
      pluginContext: IrPluginContext,
      irFunction: IrSimpleFunction,
      firFunction: FirFunction,
  ): IrBlockBody {
    val storage =
        Fir2IrPluginContext::class.java.declaredField("c").get(pluginContext)
            as Fir2IrComponentsStorage
    val scope =
        Fir2IrComponentsStorage::class.java.declaredField("conversionScope").get(storage)
            as Fir2IrConversionScope
    val skipBodies = Fir2IrConfiguration::class.java.declaredField("skipBodies")
    val firBody = firFunction.body ?: error("no FIR body")
    val parents =
        generateSequence(irFunction.parent) { (it as? IrDeclaration)?.parent }.toList().asReversed()
    return withParents(scope, parents, 0) {
      scope.withParent(irFunction) {
        scope.withFunction(irFunction) {
          val declarations = storage.declarationStorage
          declarations.enterScope(irFunction.symbol)
          // Lambdas in the body are converted through the member generator, which reads skipBodies.
          val configuration = storage.configuration
          val skipped = skipBodies.getBoolean(configuration)
          skipBodies.setBoolean(configuration, false)
          try {
            Fir2IrDeclarationStorage::class
                .java
                .getMethod("putParametersInScope", IrFunction::class.java, FirFunction::class.java)
                .invoke(declarations, irFunction, firFunction)
            val converter = convertToIrBlockBodyMethod()
            (if (converter.parameterCount == 1) {
              converter.invoke(storage.fir2IrVisitor, firBody)
            } else {
              // The newer API's null prologue is the ordinary block-conversion default.
              converter.invoke(storage.fir2IrVisitor, firBody, null)
            })
                as IrBlockBody
          } finally {
            skipBodies.setBoolean(configuration, skipped)
            declarations.leaveScope(irFunction.symbol)
          }
        }
      }
    }
  }

  private fun Class<*>.declaredField(name: String) =
      getDeclaredField(name).apply { isAccessible = true }

  // An `internal` member, so its JVM name ends in the compiler module's name: `fir2ir` up to Kotlin
  // 2.3, `org_jetbrains_kotlin_fir2ir` from 2.4.
  private fun convertToIrBlockBodyMethod() =
      Fir2IrVisitor::class.java.methods.singleOrNull {
        it.name.startsWith("convertToIrBlockBody\$") &&
            !Modifier.isStatic(it.modifiers) &&
            it.returnType == IrBlockBody::class.java &&
            (it.parameterTypes.contentEquals(arrayOf(FirBlock::class.java)) ||
                it.parameterTypes.contentEquals(arrayOf(FirBlock::class.java, List::class.java)))
      }
          ?: throw NoSuchMethodException(
              "${Fir2IrVisitor::class.java.name}.convertToIrBlockBody(${FirBlock::class.java.name})",
          )

  // Recreates the conversion scope that fir2ir had at the function: the file, then each enclosing
  // class from the outermost in.
  private fun <R> withParents(
      scope: Fir2IrConversionScope,
      parents: List<IrDeclarationParent>,
      index: Int,
      body: () -> R,
  ): R {
    if (index == parents.size) return body()
    val parent = parents[index]
    if (parent !is IrClass) {
      return scope.withParent(parent) { withParents(scope, parents, index + 1, body) }
    }
    return scope.withParent(parent) {
      scope.withClass(parent) {
        val firClass = (parent.metadata as? FirMetadataSource.Class)?.fir
        if (firClass == null) {
          withParents(scope, parents, index + 1, body)
        } else {
          var result: Result<R>? = null
          scope.withContainingFirClass(firClass) {
            result = runCatching { withParents(scope, parents, index + 1, body) }
          }
          result!!.getOrThrow()
        }
      }
    }
  }

  /**
   * The one decision for a body that cannot be kept: an empty body would miscompile every caller,
   * so the compilation fails.
   */
  private fun skipBody(messageCollector: MessageCollector, name: String, reason: String) {
    messageCollector.report(
        CompilerMessageSeverity.ERROR,
        "Kosabi inline bodies: the body of inline function `$name` cannot be kept in the " +
            "source-only ABI: $reason. Its callers would inline an empty body. Remove `inline`, " +
            "make the function private, or change its body; or set abi_generation_mode = \"class\".",
    )
  }
}

/**
 * Whether [e] means that the compiler internals k2-jvm-abi-gen reaches by reflection do not exist
 * in this Kotlin compiler (a renamed or changed member, or a value of an unexpected type), as
 * opposed to a failure while converting one particular body. A Kotlin upgrade that breaks them
 * would otherwise leave every kept body empty, so this fails the compilation instead of skipping
 * the body.
 */
fun isKosabiCompilerInternalsFailure(e: Throwable): Boolean {
  if (e is InvocationTargetException) return e.targetException is LinkageError
  return e is ReflectiveOperationException || e is LinkageError || e is ClassCastException
}

internal fun reportCompilerInternalsFailure(messageCollector: MessageCollector, cause: Throwable) {
  messageCollector.report(
      CompilerMessageSeverity.ERROR,
      "Kosabi inline bodies: the Kotlin compiler internals that k2-jvm-abi-gen uses to keep inline " +
          "function bodies are missing in Kotlin ${KotlinCompilerVersion.VERSION} ($cause). " +
          "k2-jvm-abi-gen must be updated for this compiler.",
  )
}
