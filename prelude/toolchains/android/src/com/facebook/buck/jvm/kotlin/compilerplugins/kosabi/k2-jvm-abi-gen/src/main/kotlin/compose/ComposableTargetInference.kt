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
@file:OptIn(SymbolInternals::class, DirectDeclarationsAccess::class)

package com.facebook

import androidx.compose.compiler.plugins.kotlin.inference.ApplierInferencer
import androidx.compose.compiler.plugins.kotlin.inference.Item
import androidx.compose.compiler.plugins.kotlin.inference.LazyScheme
import androidx.compose.compiler.plugins.kotlin.inference.LazySchemeStorage
import androidx.compose.compiler.plugins.kotlin.inference.NodeAdapter
import androidx.compose.compiler.plugins.kotlin.inference.NodeKind
import androidx.compose.compiler.plugins.kotlin.inference.Open
import androidx.compose.compiler.plugins.kotlin.inference.Scheme
import androidx.compose.compiler.plugins.kotlin.inference.Token
import androidx.compose.compiler.plugins.kotlin.inference.TypeAdapter
import org.jetbrains.kotlin.KtPsiSourceElement
import org.jetbrains.kotlin.KtRealSourceElementKind
import org.jetbrains.kotlin.com.intellij.psi.PsiElement
import org.jetbrains.kotlin.descriptors.ClassKind
import org.jetbrains.kotlin.descriptors.DescriptorVisibilities
import org.jetbrains.kotlin.descriptors.Modality
import org.jetbrains.kotlin.fir.FirElement
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.declarations.DirectDeclarationsAccess
import org.jetbrains.kotlin.fir.declarations.FirAnonymousFunction
import org.jetbrains.kotlin.fir.declarations.FirClass
import org.jetbrains.kotlin.fir.declarations.FirConstructor
import org.jetbrains.kotlin.fir.declarations.FirDeclaration
import org.jetbrains.kotlin.fir.declarations.FirFile
import org.jetbrains.kotlin.fir.declarations.FirFunction
import org.jetbrains.kotlin.fir.declarations.FirProperty
import org.jetbrains.kotlin.fir.declarations.FirPropertyAccessor
import org.jetbrains.kotlin.fir.declarations.FirValueParameter
import org.jetbrains.kotlin.fir.declarations.impl.FirDefaultPropertyAccessor
import org.jetbrains.kotlin.fir.declarations.toAnnotationClassIdSafe
import org.jetbrains.kotlin.fir.declarations.utils.isFun
import org.jetbrains.kotlin.fir.declarations.utils.sourceElement
import org.jetbrains.kotlin.fir.expressions.FirAnnotation
import org.jetbrains.kotlin.fir.expressions.FirAnonymousFunctionExpression
import org.jetbrains.kotlin.fir.expressions.FirAnonymousObjectExpression
import org.jetbrains.kotlin.fir.expressions.FirBlock
import org.jetbrains.kotlin.fir.expressions.FirCallableReferenceAccess
import org.jetbrains.kotlin.fir.expressions.FirCheckedSafeCallSubject
import org.jetbrains.kotlin.fir.expressions.FirElvisExpression
import org.jetbrains.kotlin.fir.expressions.FirExpression
import org.jetbrains.kotlin.fir.expressions.FirFunctionCall
import org.jetbrains.kotlin.fir.expressions.FirImplicitInvokeCall
import org.jetbrains.kotlin.fir.expressions.FirLiteralExpression
import org.jetbrains.kotlin.fir.expressions.FirOperation
import org.jetbrains.kotlin.fir.expressions.FirPropertyAccessExpression
import org.jetbrains.kotlin.fir.expressions.FirQualifiedAccessExpression
import org.jetbrains.kotlin.fir.expressions.FirResolvedQualifier
import org.jetbrains.kotlin.fir.expressions.FirSafeCallExpression
import org.jetbrains.kotlin.fir.expressions.FirSmartCastExpression
import org.jetbrains.kotlin.fir.expressions.FirSuperReceiverExpression
import org.jetbrains.kotlin.fir.expressions.FirThisReceiverExpression
import org.jetbrains.kotlin.fir.expressions.FirTypeOperatorCall
import org.jetbrains.kotlin.fir.expressions.FirVarargArgumentsExpression
import org.jetbrains.kotlin.fir.expressions.FirWhenExpression
import org.jetbrains.kotlin.fir.expressions.FirWrappedArgumentExpression
import org.jetbrains.kotlin.fir.expressions.resolvedArgumentMapping
import org.jetbrains.kotlin.fir.java.VirtualFileBasedSourceElement
import org.jetbrains.kotlin.fir.references.FirNamedReference
import org.jetbrains.kotlin.fir.references.FirResolvedErrorReference
import org.jetbrains.kotlin.fir.references.FirResolvedNamedReference
import org.jetbrains.kotlin.fir.resolve.fullyExpandedType
import org.jetbrains.kotlin.fir.resolve.providers.firProvider
import org.jetbrains.kotlin.fir.resolve.providers.symbolProvider
import org.jetbrains.kotlin.fir.resolve.toRegularClassSymbol
import org.jetbrains.kotlin.fir.symbols.FirBasedSymbol
import org.jetbrains.kotlin.fir.symbols.SymbolInternals
import org.jetbrains.kotlin.fir.symbols.impl.FirCallableSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirConstructorSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirFunctionSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirNamedFunctionSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirPropertyAccessorSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirPropertySymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirValueParameterSymbol
import org.jetbrains.kotlin.fir.types.ConeDefinitelyNotNullType
import org.jetbrains.kotlin.fir.types.ConeErrorType
import org.jetbrains.kotlin.fir.types.ConeKotlinType
import org.jetbrains.kotlin.fir.types.ConeKotlinTypeProjection
import org.jetbrains.kotlin.fir.types.ConeLookupTagBasedType
import org.jetbrains.kotlin.fir.types.ConeTypeParameterType
import org.jetbrains.kotlin.fir.types.ConeTypeProjection
import org.jetbrains.kotlin.fir.types.classId
import org.jetbrains.kotlin.fir.types.coneTypeOrNull
import org.jetbrains.kotlin.fir.types.contains
import org.jetbrains.kotlin.fir.types.customAnnotations
import org.jetbrains.kotlin.fir.types.isAny
import org.jetbrains.kotlin.fir.types.isNullableAny
import org.jetbrains.kotlin.fir.types.lowerBoundIfFlexible
import org.jetbrains.kotlin.fir.types.resolvedType
import org.jetbrains.kotlin.fir.visitors.FirVisitorVoid
import org.jetbrains.kotlin.load.kotlin.JvmPackagePartSource
import org.jetbrains.kotlin.load.kotlin.KotlinJvmBinarySourceElement
import org.jetbrains.kotlin.name.CallableId
import org.jetbrains.kotlin.name.ClassId
import org.jetbrains.kotlin.name.FqName
import org.jetbrains.kotlin.name.Name
import org.jetbrains.kotlin.util.OperatorNameConventions

/** An applier annotation the Compose backend writes on a declaration. */
internal sealed class ComposableTargetAnnotation {
  data class Target(val applier: String) : ComposableTargetAnnotation()

  data class OpenTarget(val index: Int) : ComposableTargetAnnotation()

  data class InferredTarget(val scheme: String) : ComposableTargetAnnotation()
}

/**
 * Reproduces the `@ComposableTarget` / `@ComposableInferredTarget` / `@ComposableOpenTarget`
 * annotations that the Compose backend (`ComposableTargetAnnotationsTransformer`) infers for the
 * module's composable declarations.
 *
 * The Compose backend infers them from function bodies, which the source-only ABI never lowers, so
 * the stub previously carried none and consumers lost the applier their own Compose checker and
 * applier inference need. This runs the Compose compiler's applier inference ([ApplierInferencer])
 * over the resolved FIR with the node model of the Compose backend transformer:
 * - the same declarations are inferred (composable, or taking a composable lambda; no explicit
 *   target, no `Any`-typed or type-parameter-typed parameter);
 * - calls are attributed to the nearest such declaration, so the body of a non-composable (inline)
 *   lambda counts towards its enclosing composable, as in the lowered IR;
 * - a call to a non-generic declaration of this module with a body shares that declaration's
 *   inferred scheme; any other callee contributes the scheme its annotations declare;
 * - the result is serialized exactly as the backend does: a scheme over composable-lambda
 *   parameters as `@ComposableInferredTarget`, otherwise the target as `@ComposableTarget` or
 *   `@ComposableOpenTarget`.
 *
 * The reduced source-only classpath can hide what a call contributes. A call that does not resolve
 * (or resolves to a stubsgen stand-in) can only add a constraint on the applier of the code it sits
 * in: it is skipped, so the result can lose a binding but never gain a wrong one, which is only
 * safe for a plain target. Wherever the effect of such a call or of an unresolved type on a
 * scheme's parameter links is unknown, the declaration is marked incomplete and only a bound target
 * token is emitted for it; a declaration with an unresolved type in its own signature gets nothing.
 * [notComputed] lists the declarations left without the annotation they may need, each with the
 * unresolved references that block it and the import each reference resolves through, so the
 * applicability rule, the codemod and the driver can act before the compare.
 */
internal class ComposableTargetInference(
    private val session: FirSession,
    private val stubSourcePaths: Set<String> = emptySet(),
    /**
     * Whether a call to an in-module generic function the backend has not annotated yet, whose
     * scheme then comes from the call's argument types, links the lambdas passed to it with the
     * caller. The backend does not always make that link, so [ComposeAbiEmulationStage] keeps only
     * what holds both with and without it.
     */
    private val linkThroughCallTypes: Boolean = true,
    /**
     * Lambdas that do not capture the caller's applier in this run. The backend lets a lambda whose
     * applier is still unbound when the enclosing call is inferred capture the caller's applier
     * (`ApplierInferencer.visitCall`). Code that does not resolve here can bind the lambda earlier
     * in the library compile, so for the lambdas [captureUncertainLambdas] returns the capture may
     * not happen there. Lambdas the backend hoists into singletons are read after their call site
     * either way, so they always capture and are never listed here.
     */
    private val nonCapturing: Set<FirAnonymousFunction> = emptySet(),
) {
  /** Keyed by the declaration's PSI, which survives the FIR copies later stages make. */
  val annotations: MutableMap<PsiElement, ComposableTargetAnnotation> = LinkedHashMap()

  /** Display name of the declaration each emitted annotation belongs to, by annotation key. */
  val annotationOwners: MutableMap<PsiElement, String> = LinkedHashMap()

  /** Declarations whose applier scheme could not be fully computed on this classpath. */
  val notComputed: MutableList<UncomputedApplier> = mutableListOf()

  private val fileOf = HashMap<FirBasedSymbol<*>, FirFile>()
  private val ownerMap = HashMap<Any, FirFunction>()
  private val parameterOwners = HashMap<FirBasedSymbol<*>, Pair<FirFunction, Int>>()
  private val variableDeclarations = HashMap<FirBasedSymbol<*>, VariableNode>()
  private val safeCallSubjects = HashMap<FirCheckedSafeCallSubject, VariableNode>()
  private val inferredSchemes = HashMap<FirBasedSymbol<*>, Scheme>()
  private val current = HashMap<InferenceFunction, Scheme>()
  private val lazySchemes = HashMap<Node, LazyScheme>()

  /** Owners with a region whose contribution to the scheme's parameter links is unknown. */
  private val incomplete = HashSet<FirFunction>()

  /** Owners with a call or read that does not resolve on this classpath. */
  private val unknownCalls = HashSet<FirFunction>()

  /** Unresolved references recorded at the site, keyed by the owner attributed there. */
  private val unknownRefs = HashMap<FirFunction, MutableSet<UnresolvedApplierReference>>()

  /** [unknownRefs] propagated to enclosing owners and shared in-module callees. */
  private val propagatedRefs = HashMap<FirFunction, Set<UnresolvedApplierReference>>()

  /** Simple name -> imported FQN per file, alias-aware; star imports cannot resolve a name. */
  private val fileImports = HashMap<FirFile, Map<String, String>>()

  /** Owners with an unresolved type in their own signature. */
  private val tainted = HashSet<FirFunction>()

  /**
   * Owners whose region (nested lambdas and shared in-module callees included) holds code that does
   * not resolve here. Such code may bind an applier the inferred scheme leaves open, so only the
   * scheme's fully bound parts can be trusted.
   */
  private val uncertain = HashSet<FirFunction>()

  /** Owner -> in-module declarations whose inferred scheme its calls share. */
  private val sharedCallees = HashMap<FirFunction, MutableSet<FirFunction>>()
  private val trackedFunctions = LinkedHashSet<FirFunction>()

  /** Composable lambdas passed as arguments, with the in-module declaration they are passed to. */
  private val lambdaArguments = ArrayList<Pair<FirAnonymousFunction, FirFunction?>>()

  /** Argument lambdas visited after their call, as the backend reads its singletons. */
  private val deferredLambdas = HashSet<FirAnonymousFunction>()
  private val declarations = LinkedHashMap<FirBasedSymbol<*>, FirFunction>()
  private val enclosingOwner = HashMap<FirFunction, FirFunction>()

  private var currentOwner: FirFunction? = null

  private val infer = ApplierInferencer(
      typeAdapter =
          object : TypeAdapter<InferenceFunction> {
            override fun declaredSchemaOf(type: InferenceFunction): Scheme = type.toDeclaredScheme()

            override fun currentInferredSchemeOf(type: InferenceFunction): Scheme? =
                if (type.schemeIsUpdatable) current[type] ?: declaredSchemaOf(type) else null

            override fun updatedInferredScheme(type: InferenceFunction, scheme: Scheme) {
              type.updateScheme(scheme)
              current[type] = scheme
            }
          },
      nodeAdapter =
          object : NodeAdapter<InferenceFunction, Node> {
            override fun containerOf(node: Node): Node =
                ownerMap[node.element]?.let { FunctionDeclarationNode(it) }
                    ?: (node as? ResolvedParameterNode)?.referenceContainer
                    ?: node

            override fun kindOf(node: Node): NodeKind = node.kind

            override fun schemeParameterIndexOf(node: Node, container: Node): Int =
                node.parameterIndex(container)

            override fun typeOf(node: Node): InferenceFunction? = node.function

            override fun referencedContainerOf(node: Node): Node? = node.referenceContainer
          },
      lazySchemeStorage =
          object : LazySchemeStorage<Node> {
            override fun getLazyScheme(node: Node): LazyScheme? = lazySchemes[node]

            override fun storeLazyScheme(node: Node, value: LazyScheme) {
              lazySchemes[node] = value
            }
          },
      errorReporter = silentErrorReporter(),
  )

  fun analyze(files: List<FirFile>) {
    val sourceFiles = files.filter { it.sourceFile?.path !in stubSourcePaths }
    for (file in sourceFiles) file.accept(FileIndexer(file))
    for (file in sourceFiles) file.accept(InferenceVisitor())
    propagateIncompleteness()
    propagateReferences()
    // A declaration annotated through its callers before the backend reached it is not tracked,
    // but keeps the annotation.
    val decided = LinkedHashSet(trackedFunctions)
    for ((symbol, function) in declarations) if (symbol in inferredSchemes) decided += function
    for (function in decided) decide(function)
  }

  private fun decide(function: FirFunction) {
    if (function is FirAnonymousFunction || function.symbol.isLocalCallable()) return
    val key = function.psiKey() ?: return
    val name = function.displayName()
    val scheme = inferredSchemes[function.symbol]
    if (scheme == null || (scheme.parameters.isEmpty() && scheme.target !is Token)) {
      if (function in unknownCalls || function in incomplete || function in tainted) {
        recordNotComputed(
            function,
            name,
            "no applier inferred; its body uses code not on the classpath",
        )
      }
      if (scheme == null) return
    }
    if (function in tainted) {
      recordNotComputed(function, name, "unresolved type in its signature")
      return
    }
    val annotation =
        if (scheme.parameters.isNotEmpty()) {
          if (function in incomplete || (function in uncertain && scheme.mayBeBoundElsewhere())) {
            recordNotComputed(
                function,
                name,
                "parameter appliers depend on code not on the classpath",
            )
            return
          }
          ComposableTargetAnnotation.InferredTarget(scheme.serializeCompat())
        } else {
          when (val target = scheme.target) {
            is Token -> ComposableTargetAnnotation.Target(target.value)
            is Open ->
                if (target.index < 0) return
                else if (function in incomplete || function in uncertain) {
                  recordNotComputed(
                      function,
                      name,
                      "open target depends on code not on the classpath",
                  )
                  return
                } else ComposableTargetAnnotation.OpenTarget(target.index)
          }
        }
    annotations[key] = annotation
    annotationOwners[key] = name
  }

  private fun recordNotComputed(function: FirFunction, name: String, reason: String) {
    notComputed += UncomputedApplier(name, reason, refsFor(function))
  }

  /** The blocking references of [function], propagated and in a stable order. */
  private fun refsFor(function: FirFunction): List<UnresolvedApplierReference> =
      propagatedRefs[function].orEmpty().sortedWith(REF_ORDER)

  /** Blocking references by declaration display name, merged over overloads. */
  fun referencesByDeclaration(): Map<String, List<UnresolvedApplierReference>> =
      propagatedRefs.entries.groupBy({ it.key.displayName() }, { it.value }).mapValues { (_, sets)
        ->
        sets.flatten().distinct().sortedWith(REF_ORDER)
      }

  /**
   * Entries for declarations this run and [other] computed differently: neither run records these
   * in [notComputed], but no annotation is emitted for them either, so without an entry they would
   * vanish silently. Declarations already in either run's [notComputed] keep that entry instead.
   */
  fun disagreementEntries(other: ComposableTargetInference): List<UncomputedApplier> {
    val reported = (notComputed + other.notComputed).mapTo(HashSet()) { it.declaration }
    val ownRefs = referencesByDeclaration()
    val otherRefs = other.referencesByDeclaration()
    return (annotations.keys + other.annotations.keys)
        .filter { key -> annotations[key] != other.annotations[key] }
        .mapNotNull { key ->
          val name = annotationOwners[key] ?: other.annotationOwners[key] ?: return@mapNotNull null
          if (name in reported) return@mapNotNull null
          name to
              (ownRefs[name].orEmpty() + otherRefs[name].orEmpty()).distinct().sortedWith(REF_ORDER)
        }
        .groupBy({ it.first }, { it.second })
        .map { (name, refLists) ->
          UncomputedApplier(
              declaration = name,
              reason = "inference runs disagree; applier not reliably computable on this classpath",
              references = refLists.flatten().distinct().sortedWith(REF_ORDER),
          )
        }
  }

  private fun propagateIncompleteness() {
    for ((nested, outer) in enclosingOwner) {
      if (nested in incomplete || nested in tainted) markIncomplete(outer)
    }
    var changed = true
    while (changed) {
      changed = false
      for ((owner, callees) in sharedCallees) {
        if (owner !in incomplete && callees.any { it in incomplete || it in tainted }) {
          incomplete += owner
          changed = true
        }
      }
      for ((nested, outer) in enclosingOwner) {
        if (nested in incomplete && outer !in incomplete) {
          incomplete += outer
          changed = true
        }
      }
    }
    uncertain += unknownCalls
    uncertain += incomplete
    uncertain += tainted
    changed = true
    while (changed) {
      changed = false
      for ((nested, outer) in enclosingOwner) {
        if (nested in uncertain && uncertain.add(outer)) changed = true
      }
      for ((owner, callees) in sharedCallees) {
        if (owner !in uncertain && callees.any { it in uncertain }) {
          uncertain += owner
          changed = true
        }
      }
    }
  }

  /** An unbound target, or a parameter applier linked to one, could still be bound. */
  private fun Scheme.mayBeBoundElsewhere(): Boolean =
      target !is Token || parameters.any { it.hasOpenIndex() } || result?.hasOpenIndex() == true

  private fun Scheme.hasOpenIndex(): Boolean =
      (target as? Open)?.let { it.index >= 0 } == true ||
          parameters.any { it.hasOpenIndex() } ||
          result?.hasOpenIndex() == true

  /**
   * Lambdas whose capture of the caller's applier depends on code that does not resolve here: code
   * in the lambda's own region may bind it before the call is inferred in the library compile.
   * Lambdas the backend hoists into singletons ([deferredLambdas]) are read after their call site
   * either way, so their capture does not depend on the classpath. Call after [analyze].
   */
  fun captureUncertainLambdas(): Set<FirAnonymousFunction> =
      lambdaArguments
          .filter { (lambda, _) -> lambda in uncertain && lambda !in deferredLambdas }
          .mapTo(HashSet()) { it.first }

  private fun markIncomplete(owner: FirFunction) {
    var cursor: FirFunction? = owner
    while (cursor != null && incomplete.add(cursor)) cursor = enclosingOwner[cursor]
  }

  /**
   * A declaration is blocked by the unresolved references in its own body, its nested lambdas, and
   * the in-module callees whose scheme it shares: the same fixpoint [propagateIncompleteness] runs
   * for uncertainty.
   */
  private fun propagateReferences() {
    val merged = HashMap<FirFunction, MutableSet<UnresolvedApplierReference>>()
    unknownRefs.forEach { (owner, refs) ->
      merged.getOrPut(owner) { LinkedHashSet() } += refs
    }
    var changed = true
    while (changed) {
      changed = false
      for ((nested, outer) in enclosingOwner) {
        if (merged.getOrPut(outer) { LinkedHashSet() }.addAll(merged[nested].orEmpty())) {
          changed = true
        }
      }
      for ((owner, callees) in sharedCallees) {
        val into = merged.getOrPut(owner) { LinkedHashSet() }
        for (callee in callees) if (into.addAll(merged[callee].orEmpty())) changed = true
      }
    }
    propagatedRefs.putAll(merged)
  }

  /**
   * Records a call or read that does not resolve here. [name] is the reference as written;
   * [receiver] the qualifier's root identifier when it has one. The import is looked up for the
   * root identifier: the receiver when present, else the name.
   */
  private fun recordUnknownReference(
      owner: FirFunction,
      element: FirElement,
      name: String?,
      receiver: String?,
      kind: UnresolvedApplierReferenceKind,
  ) {
    val file = fileOf[owner.symbol]
    val root = receiver ?: name
    unknownRefs.getOrPut(owner) { LinkedHashSet() } +=
        UnresolvedApplierReference(
            name = name ?: root ?: sourceTextOf(element) ?: "?",
            kind = kind,
            receiver = receiver,
            importFqn = file?.let { importsOf(it)[root] },
            file = file?.sourceFile?.path,
        )
  }

  /** The root identifier of a call or read qualifier, e.g. `WdsTheme` for `WdsTheme.colors`. */
  private fun receiverRootOf(access: FirQualifiedAccessExpression): String? {
    val receiver = access.explicitReceiver ?: access.dispatchReceiver ?: return null
    return firRootOf(unwrap(receiver)) ?: sourceRootOf(receiver)
  }

  /**
   * The root identifier through FIR: qualifier chains recurse to their head (`A.b.c` is rooted at
   * `A`), and a resolved qualifier reads back as written. Anything else (`this`, a call result) has
   * no imported root and falls through to the source text.
   */
  private fun firRootOf(expression: FirExpression): String? =
      when (val e = unwrap(expression)) {
        is FirPropertyAccessExpression -> {
          val qualifier = e.explicitReceiver ?: e.dispatchReceiver
          if (qualifier != null) firRootOf(qualifier)
          else (e.calleeReference as? FirNamedReference)?.name?.asString()
        }
        is FirResolvedQualifier -> sourceRootOf(e)
        else -> null
      }

  /** The root identifier of a source fragment: `A` for `A`, `A.b`, `(A).b` and `A<T>.b`. */
  private fun sourceRootOf(element: FirElement): String? =
      (element.source as? KtPsiSourceElement)?.psi?.text?.let { rootIdent(it) }

  private fun rootIdent(text: String): String? {
    val ident =
        text.trimStart().trimStart('(').trimStart().takeWhile { it.isLetterOrDigit() || it == '_' }
    return ident.ifEmpty { null }?.takeUnless { it == "this" || it == "super" }
  }

  private fun importsOf(file: FirFile): Map<String, String> =
      fileImports.getOrPut(file) {
        file.imports
            .mapNotNull { import ->
              if (import.isAllUnder) null
              else {
                val fqName = import.importedFqName ?: return@mapNotNull null
                val simpleName = import.aliasName?.asString() ?: fqName.shortName().asString()
                simpleName to fqName.asString()
              }
            }
            .toMap()
      }

  private fun sourceTextOf(element: FirElement): String? =
      (element.source as? KtPsiSourceElement)?.psi?.text?.take(80)?.replace('\n', ' ')

  // --- Traversal -----------------------------------------------------------------------------

  private inner class FileIndexer(private val file: FirFile) : FirVisitorVoid() {
    override fun visitElement(element: FirElement) {
      if (element is FirFunction) {
        fileOf[element.symbol] = file
        declarations[element.symbol] = element
      }
      element.acceptChildren(this)
    }
  }

  private inner class InferenceVisitor : FirVisitorVoid() {
    /** Enclosing public-API inline functions: their lambdas stay inline at the use site. */
    private var publicInlineDepth = 0

    override fun visitElement(element: FirElement) {
      when (element) {
        is FirFunction -> handleFunctionDeclaration(element)
        is FirProperty ->
            if (element.isLocal) handleLocalVariable(element) else element.acceptChildren(this)
        is FirFunctionCall -> handleFunctionCall(element)
        is FirPropertyAccessExpression -> handlePropertyAccess(element)
        is FirSafeCallExpression -> handleSafeCall(element)
        else -> element.acceptChildren(this)
      }
    }

    /**
     * The backend sees `receiver?.selector` as a temporary variable initialized with the receiver,
     * so `content?.invoke()` reaches the parameter through that variable.
     */
    private fun handleSafeCall(safeCall: FirSafeCallExpression) {
      val subject = safeCall.checkedSubjectRef.value
      val receiver = safeCall.receiver
      val type = receiver.resolvedType
      if (isOrHasComposableLambda(type)) {
        currentOwner?.let { ownerMap[subject] = it }
        val initializerNode = resolveExpressionOrNull(receiver) ?: ElementExpressionNode(receiver)
        val node = VariableNode(subject, type)
        safeCallSubjects[subject] = node
        infer.visitVariable(node, initializerNode)
      }
      safeCall.acceptChildren(this)
    }

    private fun handleFunctionDeclaration(function: FirFunction) {
      val symbol = function.symbol
      val tracked =
          !hasSchemeNow(symbol) &&
              (isComposableFunction(function) || hasComposableParameter(symbol)) &&
              !hasOverlyWideParameters(symbol) &&
              !hasOpenTypeParameters(symbol)
      val publicInline =
          function.status.isInline &&
              (function.status.visibility == DescriptorVisibilities.PUBLIC ||
                  function.status.visibility == DescriptorVisibilities.PROTECTED)
      if (publicInline) publicInlineDepth++
      if (!tracked) {
        function.acceptChildren(this)
        if (publicInline) publicInlineDepth--
        return
      }
      trackedFunctions += function
      val oldOwner = currentOwner
      oldOwner?.let { enclosingOwner[function] = it }
      currentOwner = function
      var index = 0
      for ((parameterSymbol, type) in targetParameters(symbol)) {
        if (isOrHasComposableLambda(type)) {
          parameterOwners[parameterSymbol] = function to index++
        }
      }
      if (signatureHasErrors(symbol)) tainted += function
      function.acceptChildren(this)
      currentOwner = oldOwner
      if (publicInline) publicInlineDepth--
    }

    private fun handleLocalVariable(variable: FirProperty) {
      val delegate = variable.delegate
      if (delegate != null) {
        // The backend tracks the `x$delegate` variable and resolves reads of `x` to it.
        val delegateType = delegate.resolvedType
        if (isOrHasComposableLambda(delegateType)) {
          currentOwner?.let { ownerMap[delegate] = it }
          val initializerNode = resolveExpressionOrNull(delegate) ?: ElementExpressionNode(delegate)
          val node = VariableNode(delegate, delegateType)
          variableDeclarations[variable.symbol] = node
          infer.visitVariable(node, initializerNode)
        }
      } else {
        val type = variable.returnTypeRef.coneTypeOrNull
        if (type != null && isOrHasComposableLambda(type)) {
          currentOwner?.let { ownerMap[variable] = it }
          val initializer = variable.initializer
          if (initializer != null) {
            val initializerNode =
                resolveExpressionOrNull(initializer) ?: ElementExpressionNode(initializer)
            val node = VariableNode(variable, type)
            variableDeclarations[variable.symbol] = node
            infer.visitVariable(node, initializerNode)
          }
        }
      }
      variable.acceptChildren(this)
    }

    private fun handleFunctionCall(call: FirFunctionCall) {
      val owner = currentOwner
      if (owner == null) {
        call.acceptChildren(this)
        return
      }
      val callee = resolvedCallee(call)
      if (callee == null || isStandIn(callee)) {
        handleUnknownCall(call, owner)
        return
      }
      val arguments = targetArguments(call, callee)
      val parameterTypes = targetParameterTypes(callee)
      val unknownContexts =
          arguments.indices
              .filter { parameterTypes.getOrNull(it)?.isUnknown() != false }
              .mapNotNull { index -> arguments[index]?.let { unwrap(it) } }
              .toSet()
      if (callee is FirConstructorSymbol) {
        // A constructor call is not an IrCall; the backend transformer ignores it.
        visitCallChildren(call, unknownContexts, owner)
        return
      }
      val composableCall =
          hasComposableAnnotation(callee) ||
              (isInvoke(call, callee) &&
                  receiverOf(call)?.resolvedType?.let { isComposableFunctionType(it) } == true)
      val hasComposableArguments = arguments.any {
        it != null && unwrap(it).resolvedType.let { t -> isOrHasComposableLambda(t) }
      }
      if ((!composableCall && !hasComposableArguments) || isExcluded(callee)) {
        visitCallChildren(call, unknownContexts, owner)
        return
      }
      processCall(call, callee, owner, arguments, parameterTypes, receiverOf(call))
    }

    private fun handlePropertyAccess(access: FirPropertyAccessExpression) {
      val owner = currentOwner
      if (owner == null) {
        access.acceptChildren(this)
        return
      }
      val reference = access.calleeReference
      if (reference !is FirResolvedNamedReference || reference is FirResolvedErrorReference) {
        // An unresolved read can only constrain the applier of the code it sits in.
        unknownCalls += owner
        recordUnknownReference(
            owner,
            access,
            (reference as? FirNamedReference)?.name?.asString(),
            receiverRootOf(access),
            UnresolvedApplierReferenceKind.READ,
        )
        access.acceptChildren(this)
        return
      }
      val symbol = reference.resolvedSymbol as? FirCallableSymbol<*>
      if (symbol != null && !symbol.isLocalCallable() && isStandIn(symbol)) {
        // A stand-in's real getter may be composable.
        unknownCalls += owner
        recordUnknownReference(
            owner,
            access,
            (reference as? FirNamedReference)?.name?.asString(),
            receiverRootOf(access),
            UnresolvedApplierReferenceKind.READ,
        )
        access.acceptChildren(this)
        return
      }
      val property = symbol as? FirPropertySymbol
      val getter = property?.takeUnless { it.isLocal }?.getterSymbol
      if (getter == null || !hasComposableAnnotation(getter) || isExcluded(getter)) {
        access.acceptChildren(this)
        return
      }
      processCall(
          access,
          getter,
          owner,
          if (receiverTypeOf(getter) != null) listOf(access.extensionReceiver) else emptyList(),
          targetParameterTypes(getter),
          access.dispatchReceiver,
      )
    }

    private fun processCall(
        call: FirQualifiedAccessExpression,
        callee: FirCallableSymbol<*>,
        owner: FirFunction,
        arguments: List<FirExpression?>,
        parameterTypes: List<ConeKotlinType?>,
        receiver: FirExpression?,
    ) {
      // Arguments the backend would pass to inference, in parameter order. An argument whose
      // inclusion depends on an unresolved type makes the parameter alignment unknown.
      var alignmentUnknown = false
      val unknownContexts = HashSet<FirExpression>()
      val included = ArrayList<FirExpression>()
      // Lambdas the backend hoists into singletons are read after their call site: their bodies
      // are visited after the call below, so the call still sees them unbound and they capture.
      val deferred = HashSet<FirExpression>()
      val hasReceiver = receiverTypeOf(callee) != null
      arguments.forEachIndexed { index, argument ->
        if (argument == null) return@forEachIndexed
        val expression = unwrap(argument)
        val parameterType = parameterTypes.getOrNull(index)
        if (parameterType == null || parameterType.isUnknown()) unknownContexts += expression
        when (includeArgument(expression, parameterType)) {
          true -> {
            included += expression
            (expression as? FirAnonymousFunctionExpression)?.let {
              if (isSingletonizable(it.anonymousFunction, callee, index, hasReceiver)) {
                deferred += expression
              }
            }
          }
          false -> {}
          null -> alignmentUnknown = true
        }
      }
      if (alignmentUnknown) markIncomplete(owner)
      ownerMap[call] = owner
      visitCallChildren(call, unknownContexts, owner, deferred)
      if (alignmentUnknown) {
        visitDeferred(deferred)
        return
      }

      val isInvoke = call is FirFunctionCall && isInvoke(call, callee)
      val receiverType = receiver?.resolvedType
      val target =
          (if (isInvoke || (receiverType != null && isSamComposable(receiverType)))
              receiver?.let { resolveExpressionOrNull(it) }
          else null) ?: CallTargetNode(call, callee)
      if (target.isOverlyWide()) {
        visitDeferred(deferred)
        return
      }
      target.referenceContainer?.let { container ->
        (container.element as? FirFunction)?.let {
          sharedCallees.getOrPut(owner) { HashSet() } += it
        }
      }
      val linked =
          if (
              !linkThroughCallTypes &&
                  target.function is CallTypeFunction &&
                  target.referenceContainer == null &&
                  sourceFunctionOf(callee) != null
          )
              emptyList()
          else included
      val calleeDeclaration = (target.referenceContainer?.element as? FirFunction)
      for (argument in linked) {
        (argument as? FirAnonymousFunctionExpression)?.let {
          lambdaArguments += it.anonymousFunction to calleeDeclaration
          if (it in deferred) deferredLambdas += it.anonymousFunction
        }
      }
      infer.visitCall(
          CallExpressionNode(call),
          target,
          linked.map { resolveExpressionOrNull(it) ?: inferenceNodeOf(it) },
      )
      visitDeferred(deferred)
    }

    /**
     * A call that does not resolve on this classpath, or resolves to a guessed stand-in. Its own
     * contribution is dropped; lambdas passed to it are not attributed, since whether they are
     * composable (and to which applier they bind) is unknown.
     */
    private fun handleUnknownCall(call: FirFunctionCall, owner: FirFunction) {
      unknownCalls += owner
      recordUnknownReference(
          owner,
          call,
          call.calleeReference.name.asString(),
          receiverRootOf(call),
          UnresolvedApplierReferenceKind.CALL,
      )
      val arguments = call.argumentList.arguments.map { unwrap(it) }
      for (expression in flatten(arguments)) {
        if (
            expression !is FirAnonymousFunctionExpression &&
                (resolveExpressionOrNull(expression) != null ||
                    isOrHasComposableLambda(expression.resolvedType))
        ) {
          markIncomplete(owner)
        }
      }
      visitCallChildren(call, arguments.toSet(), owner)
    }

    /**
     * Visits the receivers and arguments of [call]. An argument in [unknownContexts] flows into a
     * parameter whose type is unknown here, so lambdas in it (directly, or through `if`, `when` and
     * the like) have an unknown expected type: they are not attributed and leave [owner]
     * incomplete. Calls nested in such an argument are visited normally. Arguments in [deferred]
     * are skipped here and visited by [visitDeferred] after the call is inferred.
     */
    private fun visitCallChildren(
        call: FirQualifiedAccessExpression,
        unknownContexts: Set<FirExpression>,
        owner: FirFunction,
        deferred: Set<FirExpression> = emptySet(),
    ) {
      val receivers =
          listOfNotNull(call.explicitReceiver, call.dispatchReceiver, call.extensionReceiver)
      receivers
          .distinctBy { System.identityHashCode(it) }
          .forEach { if (it !in deferred) it.accept(this) }
      val argumentList = (call as? FirFunctionCall)?.argumentList ?: return
      for (argument in argumentList.arguments) {
        val expression = unwrap(argument)
        if (expression in unknownContexts) visitUnknownContext(expression, owner)
        else if (expression !in deferred) expression.accept(this)
      }
    }

    /** Visits lambdas [visitCallChildren] skipped, with the call inferred before them. */
    private fun visitDeferred(deferred: Set<FirExpression>) {
      deferred.forEach { it.accept(this) }
    }

    /**
     * Whether the backend hoists the lambda argument into a file-level singleton
     * (`ComposerLambdaMemoization`): a composable Unit-returning lambda that captures nothing,
     * passed to a non-inlined parameter outside a public inline function's scope. The backend reads
     * singletons after their call site, so the deferred body still finds the call unbound.
     */
    private fun isSingletonizable(
        lambda: FirAnonymousFunction,
        callee: FirCallableSymbol<*>,
        argIndex: Int,
        hasReceiver: Boolean,
    ): Boolean {
      if (!isComposableFunction(lambda)) return false
      if (lambda.returnTypeRef.coneTypeOrNull?.let { expand(it).classId != UNIT } != false) {
        return false
      }
      if (publicInlineDepth > 0) return false
      if (isInlineLambdaTarget(callee, argIndex, hasReceiver)) return false
      // A lambda passed to a nullable parameter is not deferred: the backend
      // bridges it so its use-site node is not a lambda and it never
      // captures the caller. (The exact backend condition is follow-up work;
      // this matches the observed use-site kind rule. Known over-broad:
      // direct calls with nullable params bind in the backend; narrow when
      // the true rule is derived.)
      val function = runCatching { callee.fir as? FirFunction }.getOrNull()
      if (function != null) {
        val paramIndex = argIndex - (if (hasReceiver) 1 else 0)
        function.valueParameters.getOrNull(paramIndex)?.returnTypeRef?.coneTypeOrNull?.let {
          if ((it as? ConeLookupTagBasedType)?.isMarkedNullable == true) return false
        }
      }
      return !lambdaCapturesOuterValues(lambda)
    }

    /** Whether the argument is passed to an inlined parameter of an inline call. */
    private fun isInlineLambdaTarget(
        callee: FirCallableSymbol<*>,
        argIndex: Int,
        hasReceiver: Boolean,
    ): Boolean {
      val function = runCatching { callee.fir as? FirFunction }.getOrNull() ?: return true
      if (!function.status.isInline) return false
      val paramIndex = argIndex - (if (hasReceiver) 1 else 0)
      if (paramIndex < 0) return false
      val parameter = function.valueParameters.getOrNull(paramIndex) ?: return true
      return !parameter.isNoinline
    }

    private fun visitUnknownContext(expression: FirExpression, owner: FirFunction) {
      val visitor = this
      expression.accept(
          object : FirVisitorVoid() {
            override fun visitElement(element: FirElement) {
              when (element) {
                is FirAnonymousFunctionExpression -> markIncomplete(owner)
                is FirQualifiedAccessExpression,
                is FirAnonymousObjectExpression,
                is FirDeclaration -> element.accept(visitor)
                else -> element.acceptChildren(this)
              }
            }
          },
      )
    }
  }

  // --- Inference nodes, mirroring ComposableTargetAnnotationsTransformer -----------------------

  private abstract inner class Node {
    abstract val element: Any
    abstract val kind: NodeKind
    abstract val function: InferenceFunction?
    open val referenceContainer: Node?
      get() = null

    open fun parameterIndex(node: Node): Int = -1

    open fun isOverlyWide(): Boolean = function?.isOverlyWide() == true

    override fun hashCode(): Int = element.hashCode() * 31

    override fun equals(other: Any?): Boolean = other is Node && other.element == element
  }

  private inner class FunctionDeclarationNode(override val element: FirFunction) : Node() {
    override val kind: NodeKind
      get() = NodeKind.Function

    override val function: InferenceFunction = DeclarationFunction(element.symbol)
    override val referenceContainer: Node?
      get() = this.takeIf { hasBody(element) }
  }

  private inner class FunctionExpressionNode(override val element: FirAnonymousFunctionExpression) :
      Node() {
    // The inferencer treats lambda and expression arguments alike except that only lambdas capture.
    override val kind: NodeKind
      get() =
          if (element.anonymousFunction in nonCapturing) NodeKind.Expression else NodeKind.Lambda

    override val function: InferenceFunction = DeclarationFunction(element.anonymousFunction.symbol)
    override val referenceContainer: Node
      get() = FunctionDeclarationNode(element.anonymousFunction)
  }

  private inner class CallExpressionNode(override val element: FirExpression) : Node() {
    override val kind: NodeKind
      get() = NodeKind.Expression

    override val function: InferenceFunction = TypeFunction(element.resolvedType)
  }

  private inner class ElementExpressionNode(override val element: FirExpression) : Node() {
    override val kind: NodeKind
      get() = NodeKind.Expression

    override val function: InferenceFunction = TypeFunction(element.resolvedType)
  }

  private inner class UnknownElementNode(override val element: FirElement) : Node() {
    override val kind: NodeKind
      get() = NodeKind.Expression

    override val function: InferenceFunction?
      get() = null
  }

  private inner class VariableNode(override val element: FirElement, val type: ConeKotlinType) :
      Node() {
    override val kind: NodeKind
      get() = NodeKind.Variable

    override val function: InferenceFunction
      get() = TypeFunction(type)
  }

  private inner class CallTargetNode(
      override val element: FirQualifiedAccessExpression,
      callee: FirCallableSymbol<*>,
  ) : Node() {
    override fun equals(other: Any?): Boolean = other is CallTargetNode && super.equals(other)

    override fun hashCode(): Int = super.hashCode() * 31

    override val kind: NodeKind
      get() = NodeKind.Function

    override val function: InferenceFunction =
        if (hasSchemeNow(callee)) DeclarationFunction(callee) else CallTypeFunction(element, callee)

    override val referenceContainer: Node? =
        if (isGenericFunction(callee)) null
        else
            sourceFunctionOf(callee)
                ?.takeIf { hasBody(it) && it.typeParameters.isEmpty() }
                ?.let { FunctionDeclarationNode(it) }
  }

  private inner class ResolvedParameterNode(
      override val element: FirExpression,
      override val function: InferenceFunction,
      val container: Node,
      val index: Int,
  ) : Node() {
    override val kind: NodeKind
      get() = NodeKind.ParameterReference

    override fun parameterIndex(node: Node): Int = if (node.function == function) index else -1

    override val referenceContainer: Node
      get() = container

    override fun equals(other: Any?): Boolean =
        other is ResolvedParameterNode && other.element == element

    override fun hashCode(): Int = element.hashCode() * 31 + 103
  }

  private fun inferenceNodeOf(element: FirElement): Node {
    (element as? FirExpression)?.let(::functionTypeConversionArgument)?.let {
      return inferenceNodeOf(it)
    }
    return when (element) {
      is FirFunction -> FunctionDeclarationNode(element)
      is FirAnonymousFunctionExpression -> FunctionExpressionNode(element)
      is FirWrappedArgumentExpression -> inferenceNodeOf(element.expression)
      is FirTypeOperatorCall ->
          element.argumentList.arguments.firstOrNull()?.let { inferenceNodeOf(it) }
              ?: ElementExpressionNode(element)
      is FirSmartCastExpression -> inferenceNodeOf(element.originalExpression)
      is FirFunctionCall -> CallExpressionNode(element)
      is FirExpression -> ElementExpressionNode(element)
      else -> UnknownElementNode(element)
    }
  }

  /** Resolves references to tracked composable-lambda parameters and local variables. */
  private fun resolveExpressionOrNull(expression: FirExpression): Node? =
      when (val e = unwrap(expression)) {
        is FirTypeOperatorCall ->
            if (e.operation == FirOperation.AS) {
              e.argumentList.arguments.firstOrNull()?.let { resolveExpressionOrNull(it) }
            } else null
        is FirSmartCastExpression -> resolveExpressionOrNull(e.originalExpression)
        is FirCheckedSafeCallSubject -> safeCallSubjects[e]
        is FirPropertyAccessExpression -> {
          val reference = e.calleeReference
          val symbol =
              (reference as? FirResolvedNamedReference)
                  ?.takeUnless { it is FirResolvedErrorReference }
                  ?.resolvedSymbol
          when (symbol) {
            is FirValueParameterSymbol ->
                parameterOwners[symbol]?.let { (function, index) ->
                  ResolvedParameterNode(
                      e,
                      DeclarationFunction(function.symbol),
                      FunctionDeclarationNode(function),
                      index,
                  )
                }
            is FirPropertySymbol -> variableDeclarations[symbol]
            else -> null
          }
        }
        else -> null
      }

  // --- Scheme construction -------------------------------------------------------------------

  private abstract inner class InferenceFunction {
    abstract val schemeIsUpdatable: Boolean

    abstract fun updateScheme(scheme: Scheme)

    abstract fun toDeclaredScheme(defaultTarget: Item = Open(0)): Scheme

    open fun isOverlyWide(): Boolean = false
  }

  private inner class DeclarationFunction(val symbol: FirCallableSymbol<*>) : InferenceFunction() {
    override val schemeIsUpdatable: Boolean
      get() = true

    override fun updateScheme(scheme: Scheme) {
      inferredSchemes[symbol] = scheme
    }

    override fun toDeclaredScheme(defaultTarget: Item): Scheme =
        writtenScheme(symbol)?.takeIf { it.parameters.isNotEmpty() }
            ?: declaredInferredScheme(symbol)
            ?: declarationScheme(symbol, defaultTarget)

    override fun hashCode(): Int = symbol.hashCode() * 31

    override fun equals(other: Any?): Boolean =
        other is DeclarationFunction && other.symbol == symbol
  }

  /** The scheme of a call to a callee without a declared scheme, from the substituted types. */
  private inner class CallTypeFunction(
      private val call: FirQualifiedAccessExpression,
      private val callee: FirCallableSymbol<*>,
  ) : InferenceFunction() {
    override val schemeIsUpdatable: Boolean
      get() = false

    override fun updateScheme(scheme: Scheme) {}

    override fun toDeclaredScheme(defaultTarget: Item): Scheme {
      val target =
          annotationTarget(annotationsOf(callee)).let {
            if (it === UNSPECIFIED) defaultTarget else it
          }
      val arguments =
          if (call is FirFunctionCall) targetArguments(call, callee)
          else listOf(call.extensionReceiver).takeIf { receiverTypeOf(callee) != null }.orEmpty()
      val parameters =
          if (!linkThroughCallTypes && sourceFunctionOf(callee) != null) emptyList()
          else
              arguments
                  .filterNotNull()
                  .map { unwrap(it).resolvedType }
                  .filter { isOrHasComposableLambda(it) }
                  .map { typeScheme(it, defaultTarget) }
      val callType = call.resolvedType
      val result =
          if (isOrHasComposableLambda(callType)) typeScheme(callType, defaultTarget) else null
      return Scheme(target, parameters, result)
    }

    override fun isOverlyWide(): Boolean = hasOverlyWideParameters(callee)
  }

  private inner class TypeFunction(private val type: ConeKotlinType) : InferenceFunction() {
    override val schemeIsUpdatable: Boolean
      get() = false

    override fun updateScheme(scheme: Scheme) {}

    override fun toDeclaredScheme(defaultTarget: Item): Scheme = typeScheme(type, defaultTarget)
  }

  private fun declaredInferredScheme(symbol: FirBasedSymbol<*>): Scheme? =
      annotationsOf(symbol)
          .firstOrNull { classIdOf(it) == COMPOSABLE_INFERRED_TARGET }
          ?.let { stringArgument(it, SCHEME) }
          ?.let { parseScheme(it) }

  private fun declarationScheme(symbol: FirCallableSymbol<*>, defaultTarget: Item): Scheme {
    var target = writtenScheme(symbol)?.target ?: annotationTarget(annotationsOf(symbol))
    if (target === UNSPECIFIED) {
      val file = fileOf[symbol]
      target = if (file == null) defaultTarget else annotationTarget(file.annotations)
    }
    val source = sourceFunctionOf(symbol)
    val effectiveDefault = if (source == null || !hasBody(source)) defaultTarget else UNSPECIFIED
    val returnType = expand(symbol.resolvedReturnType)
    val result =
        if (isOrHasComposableLambda(returnType)) typeScheme(returnType, effectiveDefault) else null
    val parameters =
        targetParameters(symbol)
            .map { it.second }
            .filter { isOrHasComposableLambda(it) }
            .map { parameterScheme(it, effectiveDefault) }
    // The backend merges the scheme of the last overridden declaration, but computes it from the
    // overriding declaration itself (InferenceFunctionDeclaration.toScheme reads `function`), so
    // the merge does not change the scheme.
    return Scheme(target, parameters, result)
  }

  private fun parameterScheme(type: ConeKotlinType, defaultTarget: Item): Scheme {
    val samAnnotations = samOwnerOf(type)?.let { annotationsOf(it) }.orEmpty()
    val target =
        annotationTarget(expand(type).customAnnotations + samAnnotations).let {
          if (it === UNSPECIFIED) defaultTarget else it
        }
    return typeScheme(type, target)
  }

  private fun typeScheme(rawType: ConeKotlinType, defaultTarget: Item): Scheme {
    val type = expand(rawType)
    val typeArguments: List<ConeTypeProjection> =
        if (isSchemeFunctionType(type)) type.typeArguments.toList() else emptyList()
    val target =
        annotationTarget(type.customAnnotations).let {
          if (it === UNSPECIFIED) defaultTarget else it
        }

    fun schemeOf(argument: ConeTypeProjection): Scheme? =
        (argument as? ConeKotlinTypeProjection)
            ?.type
            ?.takeIf { isOrHasComposableLambda(it) }
            ?.let { typeScheme(it, defaultTarget) }

    val parameters =
        typeArguments.take(maxOf(0, typeArguments.size - 1)).mapNotNull { schemeOf(it) }
    val result = typeArguments.lastOrNull()?.let { schemeOf(it) }
    return Scheme(target, parameters, result)
  }

  private fun annotationTarget(annotations: List<FirAnnotation>): Item =
      annotations
          .firstOrNull { classIdOf(it) == COMPOSABLE_TARGET }
          ?.let { annotation -> stringArgument(annotation, APPLIER)?.let { Token(it) } }
          ?: annotations
              .firstOrNull { classIdOf(it) == COMPOSABLE_OPEN_TARGET }
              ?.let { annotation -> intArgument(annotation, INDEX)?.let { Open(it) } }
          ?: annotations
              .firstOrNull { isTargetMarked(it) }
              ?.let { classIdOf(it)?.asSingleFqName()?.asString()?.let { name -> Token(name) } }
          ?: UNSPECIFIED

  /**
   * Reads a `@ComposableInferredTarget` scheme in the format the Compose compiler writes: `[target
   * (* | [parameter]...) (:result)?]`, where the target is an applier name (quoted when it holds
   * other characters), an index, or `_`. The backend's reader is internal to its module.
   */
  private fun parseScheme(value: String): Scheme? {
    var i = 0
    fun peek(): Char = if (i < value.length) value[i] else '\u0000'
    fun item(): Item? =
        when {
          peek() == '_' -> Open(-1).also { i++ }
          peek().isDigit() -> {
            val start = i
            while (peek().isDigit()) i++
            value.substring(start, i).toIntOrNull()?.let { Open(it) }
          }
          peek() == '"' -> {
            i++
            val token = StringBuilder()
            while (peek() != '"') {
              if (peek() == '\\') i++
              if (peek() == '\u0000') return null
              token.append(peek())
              i++
            }
            i++
            Token(token.toString())
          }
          peek().isLetter() -> {
            val start = i
            while (peek().isLetterOrDigit() || peek() == '.' || peek() == '_' || peek() == '$') i++
            Token(value.substring(start, i))
          }
          else -> null
        }
    fun scheme(): Scheme? {
      if (peek() != '[') return null
      i++
      val target = item() ?: return null
      val anyParameters = peek() == '*'
      val parameters = ArrayList<Scheme>()
      if (anyParameters) i++ else while (peek() == '[') parameters += scheme() ?: return null
      var result: Scheme? = null
      if (peek() == ':') {
        i++
        result = scheme() ?: return null
      }
      if (peek() != ']') return null
      i++
      return Scheme(target, parameters, result, anyParameters)
    }
    return scheme()?.takeIf { i == value.length }
  }

  /** No applier annotation; the backend's `Item.isUnspecified`, which is internal to its module. */
  private val UNSPECIFIED = Open(-1, isUnspecified = true)

  // --- Predicates, mirroring the backend's checks on lowered IR --------------------------------

  /**
   * The scheme the backend has written to an in-module declaration so far. It annotates a
   * declaration each time inference refines its scheme, and later checks read those annotations: a
   * generic callee visited before the call contributes its inferred scheme, one visited after
   * contributes only the call's argument types.
   */
  private fun writtenScheme(symbol: FirBasedSymbol<*>): Scheme? =
      inferredSchemes[symbol]?.takeIf {
        it.parameters.isNotEmpty() ||
            it.target is Token ||
            (it.target as? Open)?.let { target -> target.index >= 0 } == true
      }

  private fun hasSchemeNow(symbol: FirBasedSymbol<*>): Boolean =
      hasSchemeSpecified(symbol) || writtenScheme(symbol) != null

  private fun hasSchemeSpecified(symbol: FirBasedSymbol<*>): Boolean =
      annotationsOf(symbol).any {
        val classId = classIdOf(it)
        classId == COMPOSABLE_TARGET ||
            classId == COMPOSABLE_OPEN_TARGET ||
            classId == COMPOSABLE_INFERRED_TARGET ||
            isTargetMarked(it)
      }

  private fun isComposableFunction(function: FirFunction): Boolean =
      hasComposableAnnotation(function.symbol) ||
          (function is FirAnonymousFunction &&
              function.typeRef.coneTypeOrNull?.let { isComposableFunctionType(it) } == true)

  /**
   * Whether the lambda references a value declared outside it, as `ComposerLambdaMemoization`
   * counts captures: a value parameter or local resolves to its declaration, while a call to a
   * local function or the construction of a local class always captures. Unresolved references
   * cannot be values declared outside, so they capture nothing.
   */
  private fun lambdaCapturesOuterValues(lambda: FirAnonymousFunction): Boolean {
    val declared = HashSet<FirBasedSymbol<*>>()
    lambda.accept(
        object : FirVisitorVoid() {
          override fun visitElement(element: FirElement) {
            when (element) {
              is FirValueParameter -> declared += element.symbol
              is FirProperty -> declared += element.symbol
              is FirFunction -> declared += element.symbol
              is FirClass -> declared += element.symbol
              else -> {}
            }
            element.acceptChildren(this)
          }
        },
    )
    var captures = false
    lambda.accept(
        object : FirVisitorVoid() {
          override fun visitElement(element: FirElement) {
            if (captures) return
            when (element) {
              is FirThisReceiverExpression -> {
                val bound: FirBasedSymbol<*>? = element.calleeReference.boundSymbol
                if (bound == null || bound !in declared) captures = true
              }
              is FirSuperReceiverExpression -> captures = true
              is FirPropertyAccessExpression -> captures = captures || capturesValue(element)
              is FirFunctionCall -> captures = captures || capturesLocal(element)
              is FirCallableReferenceAccess ->
                  captures = captures || capturesValue(element) || capturesLocal(element)
              else -> {}
            }
            element.acceptChildren(this)
          }

          private fun capturesValue(access: FirQualifiedAccessExpression): Boolean {
            val symbol =
                (access.calleeReference as? FirResolvedNamedReference)?.resolvedSymbol
                    ?: return false
            return symbol is FirValueParameterSymbol && symbol !in declared ||
                symbol is FirPropertySymbol && symbol.isLocalCallable() && symbol !in declared
          }

          private fun capturesLocal(access: FirQualifiedAccessExpression): Boolean {
            val symbol =
                (access.calleeReference as? FirResolvedNamedReference)?.resolvedSymbol
                    ?: return false
            return symbol is FirNamedFunctionSymbol && symbol.isLocalCallable() ||
                symbol is FirConstructorSymbol && symbol.callableId?.classId?.isLocal == true
          }
        },
    )
    return captures
  }

  private fun hasComposableParameter(symbol: FirCallableSymbol<*>): Boolean =
      targetParameters(symbol).any { (_, type) ->
        isComposableLambda(type) || isSamComposable(type)
      }

  private fun hasOverlyWideParameters(symbol: FirCallableSymbol<*>): Boolean =
      targetParameters(symbol).any { (_, type) ->
        val bound = type.lowerBoundIfFlexible()
        bound.isAny || bound.isNullableAny
      }

  private fun hasOpenTypeParameters(symbol: FirCallableSymbol<*>): Boolean =
      targetParameters(symbol).any { (_, type) ->
        val bound = type.lowerBoundIfFlexible()
        bound is ConeTypeParameterType ||
            (bound is ConeDefinitelyNotNullType && bound.original is ConeTypeParameterType)
      }

  private fun signatureHasErrors(symbol: FirCallableSymbol<*>): Boolean =
      symbol.resolvedReturnType.hasError() ||
          targetParameters(symbol).any { (_, type) -> type.hasError() }

  private fun isGenericFunction(callee: FirCallableSymbol<*>): Boolean =
      callee.typeParameterSymbols.isNotEmpty() ||
          callee.dispatchReceiverType?.typeArguments?.isNotEmpty() == true

  /** The backend's `isInvoke`: an `invoke` of a function type, or any call through `invoke`. */
  private fun isInvoke(call: FirFunctionCall, callee: FirCallableSymbol<*>): Boolean {
    if (call is FirImplicitInvokeCall) return true
    if (callee.name != OperatorNameConventions.INVOKE) return false
    val owner = callee.callableId?.classId ?: return false
    return isKotlinFunctionClass(owner) || isComposableFunctionClass(owner)
  }

  private fun isExcluded(callee: FirCallableSymbol<*>): Boolean {
    val callableId = callee.callableId ?: return false
    val property = (callee as? FirPropertyAccessorSymbol)?.propertySymbol?.callableId
    return property == CURRENT_COMPOSER || callableId == COMPOSABLE_LAMBDA
  }

  /** Whether the backend counts the argument in `infer.visitCall`, or null if unknowable here. */
  private fun includeArgument(expression: FirExpression, parameterType: ConeKotlinType?): Boolean? {
    if (
        expression is FirAnonymousFunctionExpression &&
            isComposableFunction(expression.anonymousFunction)
    ) {
      return true
    }
    if (isComposableParameterReference(expression)) return true
    val parameterKnown = parameterType != null && !parameterType.isUnknown()
    if (isBlockLike(expression) || isNullLiteral(expression)) {
      return if (parameterKnown) isOrHasComposableLambda(parameterType!!) else null
    }
    val type = expression.resolvedType
    if (type.isUnknown()) {
      // The argument's real type is unknown, but it can only be a composable lambda where the
      // parameter accepts one.
      return if (parameterKnown && !mayHoldComposableLambda(parameterType!!)) false else null
    }
    return isOrHasComposableLambda(type)
  }

  private fun mayHoldComposableLambda(parameterType: ConeKotlinType): Boolean {
    val bound = expand(parameterType)
    return isOrHasComposableLambda(bound) ||
        bound.isAny ||
        bound.isNullableAny ||
        bound.contains { it is ConeTypeParameterType }
  }

  private fun isComposableParameterReference(expression: FirExpression): Boolean {
    val access = expression as? FirPropertyAccessExpression ?: return false
    val symbol =
        (access.calleeReference as? FirResolvedNamedReference)?.resolvedSymbol
            as? FirValueParameterSymbol ?: return false
    if (parameterOwners[symbol] == null) return false
    val type = symbol.resolvedReturnType
    return isComposableLambda(type) || isSamComposable(type)
  }

  /** Expressions the backend sees as an `IrBlock` (safe calls, elvis, `when` with a subject). */
  private fun isBlockLike(expression: FirExpression): Boolean =
      expression is FirSafeCallExpression ||
          expression is FirElvisExpression ||
          expression is FirBlock ||
          (expression is FirWhenExpression && expression.subjectVariable != null)

  private fun isNullLiteral(expression: FirExpression): Boolean =
      expression is FirLiteralExpression && expression.value == null

  private fun isOrHasComposableLambda(rawType: ConeKotlinType): Boolean {
    val type = expand(rawType)
    return isComposableLambda(type) ||
        isSamComposable(type) ||
        type.typeArguments.any {
          (it as? ConeKotlinTypeProjection)?.type?.let { arg -> isOrHasComposableLambda(arg) } ==
              true
        }
  }

  /**
   * The backend's `isComposableLambda` on lowered types: a composable function type (lowered to
   * `FunctionN<..., Composer, Int, R>`), `ComposableLambda`, or any type with a `Composer` type
   * argument.
   */
  private fun isComposableLambda(rawType: ConeKotlinType): Boolean {
    val type = expand(rawType)
    val classId = type.classId
    if (classId == COMPOSABLE_LAMBDA_TYPE) return true
    if (
        type.typeArguments.any {
          (it as? ConeKotlinTypeProjection)?.type?.let { arg -> expand(arg).classId } == COMPOSER
        }
    ) {
      return true
    }
    return isComposableFunctionType(type)
  }

  /** `@Composable (..) -> R`: a Compose function-type kind, or a function type annotated so. */
  private fun isComposableFunctionType(rawType: ConeKotlinType): Boolean {
    val type = expand(rawType)
    val classId = type.classId ?: return false
    if (isComposableFunctionClass(classId) || isKComposableFunctionClass(classId)) return true
    return (isKotlinFunctionClass(classId) || isKFunctionClass(classId)) &&
        type.customAnnotations.any { classIdOf(it) == COMPOSABLE }
  }

  /** Function types whose type arguments make up a scheme (the backend's `IrType.isFunction`). */
  private fun isSchemeFunctionType(type: ConeKotlinType): Boolean {
    val classId = type.classId ?: return false
    return isKotlinFunctionClass(classId) || isComposableFunctionClass(classId)
  }

  private fun isSamComposable(type: ConeKotlinType): Boolean =
      samOwnerOf(type)?.let { hasComposableAnnotation(it) } == true

  private fun samOwnerOf(type: ConeKotlinType): FirNamedFunctionSymbol? {
    val classSymbol = expand(type).toRegularClassSymbol(session) ?: return null
    if (classSymbol.classKind != ClassKind.INTERFACE || !classSymbol.resolvedStatus.isFun) {
      return null
    }
    return classSymbol.declarationSymbols.filterIsInstance<FirNamedFunctionSymbol>().singleOrNull {
      it.resolvedStatus.modality == Modality.ABSTRACT
    }
  }

  private fun isTargetMarked(annotation: FirAnnotation): Boolean {
    val classId = classIdOf(annotation) ?: return false
    if (classId == COMPOSABLE || classId == COMPOSABLE_TARGET) return false
    val annotationClass =
        expand(annotation.annotationTypeRef.coneTypeOrNull ?: return false)
            .toRegularClassSymbol(session) ?: return false
    return annotationClass.resolvedAnnotationClassIds.any { it == COMPOSABLE_TARGET_MARKER }
  }

  // --- FIR helpers ---------------------------------------------------------------------------

  /** Extension receiver followed by value parameters: the backend's `targetParameters`. */
  private fun targetParameters(
      symbol: FirCallableSymbol<*>,
  ): List<Pair<FirBasedSymbol<*>, ConeKotlinType>> {
    val result = ArrayList<Pair<FirBasedSymbol<*>, ConeKotlinType>>()
    receiverTypeOf(symbol)?.let { receiver ->
      result += (symbol.receiverParameterSymbol ?: symbol) to receiver
    }
    if (symbol is FirFunctionSymbol<*>) {
      for (parameter in symbol.valueParameterSymbols) result +=
          parameter to parameter.resolvedReturnType
    }
    return result
  }

  /** An accessor's extension receiver belongs to its property. */
  private fun receiverTypeOf(symbol: FirCallableSymbol<*>): ConeKotlinType? =
      if (symbol is FirPropertyAccessorSymbol) symbol.propertySymbol.resolvedReceiverType
      else symbol.resolvedReceiverType

  private fun targetParameterTypes(symbol: FirCallableSymbol<*>): List<ConeKotlinType?> =
      targetParameters(symbol).map { it.second }

  /** Extension receiver followed by one argument per value parameter (null when defaulted). */
  private fun targetArguments(
      call: FirFunctionCall,
      callee: FirCallableSymbol<*>,
  ): List<FirExpression?> {
    val result = ArrayList<FirExpression?>()
    if (receiverTypeOf(callee) != null) result += call.extensionReceiver
    val function = callee as? FirFunctionSymbol<*> ?: return result
    val mapping = call.resolvedArgumentMapping
    val byParameter = HashMap<FirValueParameterSymbol, FirExpression>()
    mapping?.forEach { (argument, parameter) ->
      byParameter.putIfAbsent(parameter.symbol, argument)
    }
    val byName = HashMap<Name, FirExpression>()
    mapping?.forEach { (argument, parameter) -> byName.putIfAbsent(parameter.name, argument) }
    for (parameter in function.valueParameterSymbols) {
      result += byParameter[parameter] ?: byName[parameter.name]
    }
    return result
  }

  private fun receiverOf(call: FirFunctionCall): FirExpression? =
      call.dispatchReceiver ?: call.explicitReceiver

  private fun resolvedCallee(call: FirFunctionCall): FirCallableSymbol<*>? {
    val reference = call.calleeReference
    if (reference !is FirResolvedNamedReference || reference is FirResolvedErrorReference) {
      return null
    }
    return reference.resolvedSymbol as? FirCallableSymbol<*>
  }

  /** The declaration in this module's sources, if [symbol] is one. */
  private fun sourceFunctionOf(symbol: FirCallableSymbol<*>): FirFunction? {
    if (fileOf[symbol] == null) return null
    return symbol.fir as? FirFunction
  }

  private fun hasBody(function: FirFunction): Boolean =
      function.body != null || function is FirDefaultPropertyAccessor

  /**
   * A declaration stubsgen fabricated for a missing dependency, or a member of such a class: its
   * signature is a guess, and a call to it may stand for a function (`WdsTheme(content)` resolves
   * to the constructor of a stand-in class named after the unresolved import).
   */
  private fun isStandIn(symbol: FirCallableSymbol<*>): Boolean {
    if (fileOf[symbol] != null) return false
    symbol.callableId?.classId?.let { if (isStandInClass(it)) return true }
    session.firProvider.getFirCallableContainerFile(symbol)?.sourceFile?.path?.let {
      return it in stubSourcePaths
    }
    val location =
        when (val source = symbol.containerSource) {
          is JvmPackagePartSource -> source.knownJvmBinaryClass?.location
          is KotlinJvmBinarySourceElement -> source.binaryClass.location
          else -> null
        }
    return isStubLocation(location)
  }

  private val standInClasses = HashMap<ClassId, Boolean>()

  private fun isStandInClass(classId: ClassId): Boolean =
      standInClasses.getOrPut(classId) {
        // Local classes are declared in this module's own sources.
        if (classId.isLocal) return@getOrPut false
        val symbol =
            session.symbolProvider.getClassLikeSymbolByClassId(classId) ?: return@getOrPut false
        session.firProvider.getFirClassifierContainerFileIfAny(classId)?.sourceFile?.path?.let {
          return@getOrPut it in stubSourcePaths
        }
        // Stubsgen compiles its stand-ins as Java classes into `__<target>_stubgen_stubs.jar`.
        isStubLocation(
            when (val source = symbol.sourceElement) {
              is KotlinJvmBinarySourceElement -> source.binaryClass.location
              is VirtualFileBasedSourceElement -> source.virtualFile.path
              else -> null
            },
        )
      }

  private fun isStubLocation(location: String?): Boolean {
    if (location == null) return false
    val path = location.substringBefore("!/")
    return "stubgen_stubs" in path || path.substringAfterLast('/') == "stubs.jar"
  }

  private fun FirBasedSymbol<*>.isLocalCallable(): Boolean =
      (this as? FirCallableSymbol<*>)?.let { it.isLocal || it.callableId?.isLocal == true } == true

  private fun annotationsOf(symbol: FirBasedSymbol<*>): List<FirAnnotation> =
      symbol.resolvedAnnotationsWithArguments

  private fun hasComposableAnnotation(symbol: FirBasedSymbol<*>): Boolean =
      annotationsOf(symbol).any { classIdOf(it) == COMPOSABLE }

  private fun classIdOf(annotation: FirAnnotation): ClassId? =
      annotation.toAnnotationClassIdSafe(session)

  private fun stringArgument(annotation: FirAnnotation, name: Name): String? =
      (annotation.argumentMapping.mapping[name] as? FirLiteralExpression)?.value as? String

  private fun intArgument(annotation: FirAnnotation, name: Name): Int? =
      ((annotation.argumentMapping.mapping[name] as? FirLiteralExpression)?.value as? Number)
          ?.toInt()

  private fun unwrap(expression: FirExpression): FirExpression =
      if (expression is FirWrappedArgumentExpression) unwrap(expression.expression) else expression

  /** Arguments with wrappers removed and varargs (as a stand-in's `vararg Any?`) spread out. */
  private fun flatten(arguments: List<FirExpression>): List<FirExpression> = arguments.flatMap {
    when (val expression = unwrap(it)) {
      is FirVarargArgumentsExpression -> flatten(expression.arguments)
      else -> listOf(expression)
    }
  }

  private fun expand(type: ConeKotlinType): ConeKotlinType =
      type.fullyExpandedType(session).lowerBoundIfFlexible()

  private fun ConeKotlinType.hasError(): Boolean = contains { it is ConeErrorType }

  /** An unresolved type, or a stubsgen stand-in whose real shape is unknown. */
  private fun ConeKotlinType.isUnknown(): Boolean = contains {
    it is ConeErrorType || it.classId?.let { classId -> isStandInClass(classId) } == true
  }

  private fun FirFunction.psiKey(): PsiElement? {
    val source = source ?: return null
    if (source.kind != KtRealSourceElementKind) return null
    return (source as? KtPsiSourceElement)?.psi
  }

  private fun FirFunction.displayName(): String =
      when (this) {
        is FirPropertyAccessor -> "${propertySymbol.callableId}.getter"
        is FirConstructor -> "${symbol.callableId}.<init>"
        else -> symbol.callableId.toString()
      }

  private companion object {
    private val RUNTIME = FqName("androidx.compose.runtime")
    private val INTERNAL = FqName("androidx.compose.runtime.internal")
    private val KOTLIN = FqName("kotlin")
    private val KOTLIN_REFLECT = FqName("kotlin.reflect")
    val COMPOSABLE = ClassId(RUNTIME, Name.identifier("Composable"))
    val COMPOSABLE_TARGET = ClassId(RUNTIME, Name.identifier("ComposableTarget"))
    val COMPOSABLE_OPEN_TARGET = ClassId(RUNTIME, Name.identifier("ComposableOpenTarget"))
    val COMPOSABLE_INFERRED_TARGET = ClassId(RUNTIME, Name.identifier("ComposableInferredTarget"))
    val COMPOSABLE_TARGET_MARKER = ClassId(RUNTIME, Name.identifier("ComposableTargetMarker"))
    val COMPOSER = ClassId(RUNTIME, Name.identifier("Composer"))
    val COMPOSABLE_LAMBDA_TYPE = ClassId(INTERNAL, Name.identifier("ComposableLambda"))
    val CURRENT_COMPOSER = CallableId(RUNTIME, Name.identifier("currentComposer"))
    val COMPOSABLE_LAMBDA = CallableId(INTERNAL, Name.identifier("composableLambda"))
    val APPLIER = Name.identifier("applier")
    val INDEX = Name.identifier("index")
    val SCHEME = Name.identifier("scheme")
    val UNIT = ClassId(KOTLIN, Name.identifier("Unit"))

    /** Stable order for reported references: file, then kind, then the reference itself. */
    val REF_ORDER = compareBy<UnresolvedApplierReference>(
        { it.file },
        { it.kind },
        { it.receiver },
        { it.name },
        { it.importFqn },
    )

    fun isKotlinFunctionClass(classId: ClassId): Boolean =
        classId.packageFqName == KOTLIN &&
            !classId.isNestedClass &&
            classId.shortClassName.asString().startsWith("Function")

    fun isKFunctionClass(classId: ClassId): Boolean =
        classId.packageFqName == KOTLIN_REFLECT &&
            classId.shortClassName.asString().startsWith("KFunction")

    fun isComposableFunctionClass(classId: ClassId): Boolean =
        classId.packageFqName == INTERNAL &&
            classId.shortClassName.asString().startsWith("ComposableFunction")

    fun isKComposableFunctionClass(classId: ClassId): Boolean =
        classId.packageFqName == INTERNAL &&
            classId.shortClassName.asString().startsWith("KComposableFunction")
  }
}
