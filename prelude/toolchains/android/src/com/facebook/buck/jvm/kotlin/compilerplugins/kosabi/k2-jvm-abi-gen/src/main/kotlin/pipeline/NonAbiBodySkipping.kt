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

import org.jetbrains.kotlin.builtins.StandardNames
import org.jetbrains.kotlin.fir.FirElement
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.declarations.FirAnonymousInitializer
import org.jetbrains.kotlin.fir.declarations.FirDeclaration
import org.jetbrains.kotlin.fir.declarations.FirFile
import org.jetbrains.kotlin.fir.declarations.FirFunction
import org.jetbrains.kotlin.fir.declarations.FirProperty
import org.jetbrains.kotlin.fir.declarations.FirRegularClass
import org.jetbrains.kotlin.fir.declarations.FirResolvePhase
import org.jetbrains.kotlin.fir.declarations.utils.isInline
import org.jetbrains.kotlin.fir.expressions.FirBlock
import org.jetbrains.kotlin.fir.expressions.builder.buildEmptyExpressionBlock
import org.jetbrains.kotlin.fir.references.FirNamedReference
import org.jetbrains.kotlin.fir.resolve.ScopeSession
import org.jetbrains.kotlin.fir.resolve.transformers.FirGlobalResolveProcessor
import org.jetbrains.kotlin.fir.resolve.transformers.FirTransformerBasedResolveProcessor
import org.jetbrains.kotlin.fir.resolve.transformers.createAllCompilerResolveProcessors
import org.jetbrains.kotlin.fir.types.hasResolvedType
import org.jetbrains.kotlin.fir.visitors.FirVisitorVoid
import org.jetbrains.kotlin.fir.withFileAnalysisExceptionWrapping

/**
 * Runs every compiler resolve phase over [firFiles], like `FirSession.runResolution`, but empties
 * the bodies that source-only ABI generation never reads before `BODY_RESOLVE` runs.
 *
 * Kosabi converts FIR to IR with `SKIP_BODIES`: fir2ir replaces each function, accessor,
 * constructor and anonymous-initializer body with a placeholder, and the IR stripper replaces the
 * placeholder with a stub. The resolved FIR of those bodies is never read, yet resolving it was
 * most of the frontend's time.
 *
 * The phases before `BODY_RESOLVE` already resolve everything the ABI needs from a declaration: its
 * signature, an implicit return type (whose body that phase resolves), a const initializer,
 * contracts and header annotation arguments. After them, only the bodies of inline functions and
 * inline accessors are still read ([InlineBodyStage] keeps some of them), so those are left alone,
 * as is every body that is already resolved. Property initializers, delegated properties (with
 * their accessors), accessors that use `field`, and default values keep their normal resolution.
 * Local declarations live inside bodies and are not visited.
 */
internal fun FirSession.runResolutionSkippingNonAbiBodies(
    firFiles: List<FirFile>,
): Pair<ScopeSession, List<FirFile>> {
  val scopeSession = ScopeSession()
  for (processor in createAllCompilerResolveProcessors(this, scopeSession)) {
    if (processor.phase == FirResolvePhase.BODY_RESOLVE) {
      for (file in firFiles) {
        emptyNonAbiBodies(file.declarations)
      }
    }
    processor.beforePhase()
    try {
      when (processor) {
        is FirTransformerBasedResolveProcessor ->
            for (file in firFiles) {
              withFileAnalysisExceptionWrapping(file) { processor.processFile(file) }
            }
        is FirGlobalResolveProcessor -> processor.process(firFiles)
      }
    } finally {
      processor.afterPhase()
    }
  }
  return scopeSession to firFiles
}

private fun emptyNonAbiBodies(declarations: List<FirDeclaration>) {
  for (declaration in declarations) {
    when (declaration) {
      is FirRegularClass -> emptyNonAbiBodies(declaration.declarations)
      is FirFunction -> if (!declaration.isInline) declaration.emptyUnresolvedBody()
      is FirProperty ->
          // A delegated property's accessors call `getValue`/`setValue`, and body resolution
          // infers the delegate's type from those calls, so they keep their bodies. So do the
          // accessors of a property when one of them uses `field`: resolving that use sets
          // `isReferredViaField`, which decides whether a property with custom accessors gets a
          // backing field (`FirProperty.hasBackingField`).
          if (
              !declaration.isInline &&
                  declaration.delegate == null &&
                  !declaration.getter?.body.mentionsField() &&
                  !declaration.setter?.body.mentionsField()
          ) {
            declaration.getter?.takeUnless { it.isInline }?.emptyUnresolvedBody()
            declaration.setter?.takeUnless { it.isInline }?.emptyUnresolvedBody()
          }
      is FirAnonymousInitializer ->
          if (declaration.body.isUnresolved()) {
            declaration.replaceBody(buildEmptyExpressionBlock())
          }
      else -> {}
    }
  }
}

private fun FirFunction.emptyUnresolvedBody() {
  if (body.isUnresolved()) {
    replaceBody(buildEmptyExpressionBlock())
  }
}

private fun FirBlock?.isUnresolved(): Boolean = this != null && !hasResolvedType

/** Whether this body names `field` anywhere, which inside an accessor is its backing field. */
private fun FirBlock?.mentionsField(): Boolean {
  if (this == null) return false
  var found = false
  accept(
      object : FirVisitorVoid() {
        override fun visitElement(element: FirElement) {
          if (found) return
          if (element is FirNamedReference && element.name == StandardNames.BACKING_FIELD) {
            found = true
          } else {
            element.acceptChildren(this)
          }
        }
      },
  )
  return found
}
