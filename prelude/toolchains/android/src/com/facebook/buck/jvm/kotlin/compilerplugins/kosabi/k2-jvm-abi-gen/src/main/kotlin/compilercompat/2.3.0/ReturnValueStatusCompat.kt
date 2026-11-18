/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:Suppress("PackageLocationMismatch")

package com.facebook

import org.jetbrains.kotlin.config.AnalysisFlags
import org.jetbrains.kotlin.config.ReturnValueCheckerMode
import org.jetbrains.kotlin.descriptors.EffectiveVisibility
import org.jetbrains.kotlin.descriptors.Modality
import org.jetbrains.kotlin.descriptors.Visibilities
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.declarations.FirCallableDeclaration
import org.jetbrains.kotlin.fir.declarations.FirClass
import org.jetbrains.kotlin.fir.declarations.FirProperty
import org.jetbrains.kotlin.fir.declarations.FirResolvedDeclarationStatus
import org.jetbrains.kotlin.fir.declarations.delegateFieldsMap
import org.jetbrains.kotlin.fir.declarations.impl.FirResolvedDeclarationStatusImpl
import org.jetbrains.kotlin.fir.declarations.mustUseReturnValueStatusComponent
import org.jetbrains.kotlin.fir.delegatedWrapperData
import org.jetbrains.kotlin.fir.languageVersionSettings
import org.jetbrains.kotlin.fir.originalForSubstitutionOverride
import org.jetbrains.kotlin.fir.resolve.ScopeSession
import org.jetbrains.kotlin.fir.resolve.scope
import org.jetbrains.kotlin.fir.resolve.transformers.FirStatusResolver
import org.jetbrains.kotlin.fir.scopes.CallableCopyTypeCalculator
import org.jetbrains.kotlin.fir.scopes.unsubstitutedScope
import org.jetbrains.kotlin.fir.symbols.SymbolInternals
import org.jetbrains.kotlin.fir.symbols.impl.FirCallableSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirIntersectionCallableSymbol
import org.jetbrains.kotlin.name.Name
import org.jetbrains.kotlin.resolve.ReturnValueStatus

/**
 * The members [member] overrides, found the way the compiler's status resolution finds them
 * (`FirStatusResolver.getOverriddenFunctions` / `getOverriddenProperties`), so an empty list means
 * the compiler resolved [member]'s status as overriding nothing. `null` for other declarations.
 */
fun overriddenMembersCompat(
    session: FirSession,
    scopeSession: ScopeSession,
    member: FirCallableDeclaration,
    containingClass: FirClass,
): List<FirCallableDeclaration>? {
  val resolver = FirStatusResolver(session, scopeSession)
  return when (member) {
    is FirNamedFunctionCompat -> resolver.getOverriddenFunctions(member, containingClass)
    is FirProperty -> resolver.getOverriddenProperties(member, containingClass)
    else -> null
  }
}

/** The return-value status the compiler resolved for [member], or `null` if it has none. */
fun returnValueStatusCompat(member: FirCallableDeclaration): String? =
    (member.status as? FirResolvedDeclarationStatus)?.returnValueStatus?.name

/** Every [ReturnValueStatus] entry name, for [returnValueStatusOverridingCompat]. */
val RETURN_VALUE_STATUS_NAMES: List<String> = ReturnValueStatus.entries.map { it.name }

/**
 * The return-value status the compiler gives [member] when the member it overrides has the status
 * [overriddenStatusName] (a [ReturnValueStatus] entry name): an ignorable-like annotation on
 * [member] gives `ExplicitlyIgnorable`, a `MustUse` overridden member or a must-use-like annotation
 * on [member], its classes or its file gives `MustUse`, else the overridden status.
 */
fun returnValueStatusOverridingCompat(
    session: FirSession,
    member: FirCallableDeclaration,
    containingClass: FirClass,
    overriddenStatusName: String,
): String {
  val overridden = FirResolvedDeclarationStatusImpl(
      Visibilities.Public,
      Modality.OPEN,
      EffectiveVisibility.Public,
  )
      .apply { returnValueStatus = ReturnValueStatus.valueOf(overriddenStatusName) }
  return session.mustUseReturnValueStatusComponent
      .computeMustUseReturnValueForCallable(
          session,
          member.symbol,
          false,
          containingClass.symbol,
          (member as? FirProperty)?.symbol,
          listOf(overridden),
      )
      .name
}

/**
 * The return-value status the compiler gives [member] when it overrides [overridden] with their
 * current statuses, or `null` when one of them has no resolved status.
 */
fun returnValueStatusOverridingMembersCompat(
    session: FirSession,
    member: FirCallableDeclaration,
    containingClass: FirClass,
    overridden: List<FirCallableDeclaration>,
): String? {
  val statuses = overridden.map { it.status as? FirResolvedDeclarationStatus ?: return null }
  return session.mustUseReturnValueStatusComponent
      .computeMustUseReturnValueForCallable(
          session,
          member.symbol,
          false,
          containingClass.symbol,
          (member as? FirProperty)?.symbol,
          statuses,
      )
      .name
}

/**
 * Sets the return-value status of [member]'s resolved status to [statusName] (a [ReturnValueStatus]
 * entry name), as the compiler's status resolution does, together with that of a property's getter.
 * Returns `false` when [member] has no resolved status to update.
 */
fun replaceReturnValueStatusCompat(member: FirCallableDeclaration, statusName: String): Boolean {
  val status = ReturnValueStatus.valueOf(statusName)
  val resolved = member.status as? FirResolvedDeclarationStatusImpl ?: return false
  resolved.returnValueStatus = status
  if (member is FirProperty) {
    (member.getter?.status as? FirResolvedDeclarationStatusImpl)?.returnValueStatus = status
  }
  return true
}

/**
 * Whether the compiler resolves [member] as ignorable because of its own annotation, whatever it
 * overrides. Without the return-value checker the compiler ignores ignorability annotations: an
 * override inherits the status of the member it overrides.
 */
fun isIgnorableByOwnAnnotationCompat(session: FirSession, member: FirCallableDeclaration): Boolean =
    session.languageVersionSettings.getFlag(AnalysisFlags.returnValueCheckerMode) !=
        ReturnValueCheckerMode.DISABLED &&
        session.mustUseReturnValueStatusComponent.hasIgnorableLikeAnnotation(
            member.symbol.resolvedAnnotationClassIds,
        )

/** The member [member] wraps if the class implements it by delegation, otherwise `null`. */
fun delegatedWrappedMemberCompat(member: FirCallableDeclaration): FirCallableDeclaration? =
    member.delegatedWrapperData?.wrapped

/**
 * The members [klass] implements by delegation (`by`) under those of [names] that its delegated
 * interfaces declare, read from its use-site scope, the scope its metadata is serialized from, each
 * with the declarations the member it wraps stands for: the wrapped member itself, or the members a
 * substitution or intersection override copies.
 */
@OptIn(SymbolInternals::class)
fun delegatedMembersCompat(
    session: FirSession,
    scopeSession: ScopeSession,
    klass: FirClass,
    names: Collection<Name>,
): List<Pair<FirCallableDeclaration, List<FirCallableDeclaration>>> =
    delegatedCopiesCompat(session, scopeSession, klass, names).map { member ->
      member to wrappedDeclarations(checkNotNull(member.delegatedWrapperData).wrapped)
    }

/** The members of [delegatedMembersCompat] alone, without reading the members they wrap. */
@OptIn(SymbolInternals::class)
fun delegatedCopiesCompat(
    session: FirSession,
    scopeSession: ScopeSession,
    klass: FirClass,
    names: Collection<Name>,
): List<FirCallableDeclaration> {
  val delegateFields = klass.delegateFieldsMap?.values.orEmpty()
  if (delegateFields.isEmpty()) return emptyList()
  val delegatedNames =
      delegateFields.flatMapTo(HashSet()) { field ->
        field.resolvedReturnType
            .scope(session, scopeSession, CallableCopyTypeCalculator.DoNothing, null)
            ?.getCallableNames()
            .orEmpty()
      }
  val wanted = names.filter { it in delegatedNames }
  if (wanted.isEmpty()) return emptyList()
  val scope =
      klass.unsubstitutedScope(
          session,
          scopeSession,
          withForcedTypeCalculator = false,
          memberRequiredPhase = null,
      )
  val delegated = mutableListOf<FirCallableDeclaration>()
  val visit = { symbol: FirCallableSymbol<*> ->
    val member = symbol.fir
    if (member.delegatedWrapperData?.containingClass?.classId == klass.symbol.classId) {
      delegated += member
    }
  }
  for (name in wanted) {
    scope.processFunctionsByName(name, visit)
    scope.processPropertiesByName(name, visit)
  }
  return delegated
}

@OptIn(SymbolInternals::class)
private fun wrappedDeclarations(member: FirCallableDeclaration): List<FirCallableDeclaration> {
  val symbol = member.symbol
  if (symbol is FirIntersectionCallableSymbol) {
    return symbol.intersections.flatMap { wrappedDeclarations(it.fir) }
  }
  return member.originalForSubstitutionOverride?.let(::wrappedDeclarations) ?: listOf(member)
}
