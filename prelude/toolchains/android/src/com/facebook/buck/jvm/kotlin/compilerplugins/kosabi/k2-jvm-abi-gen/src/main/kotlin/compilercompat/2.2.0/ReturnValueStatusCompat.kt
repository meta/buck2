/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:Suppress("PackageLocationMismatch", "UNUSED_PARAMETER")

package com.facebook

import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.declarations.FirCallableDeclaration
import org.jetbrains.kotlin.fir.declarations.FirClass
import org.jetbrains.kotlin.fir.resolve.ScopeSession
import org.jetbrains.kotlin.name.Name

// ReturnValueStatusStage needs the status component of Kotlin 2.3+; on 2.2 it leaves every status
// as the compiler resolved it.

fun overriddenMembersCompat(
    session: FirSession,
    scopeSession: ScopeSession,
    member: FirCallableDeclaration,
    containingClass: FirClass,
): List<FirCallableDeclaration>? = null

fun returnValueStatusCompat(member: FirCallableDeclaration): String? = null

val RETURN_VALUE_STATUS_NAMES: List<String> = emptyList()

fun returnValueStatusOverridingCompat(
    session: FirSession,
    member: FirCallableDeclaration,
    containingClass: FirClass,
    overriddenStatusName: String,
): String = overriddenStatusName

fun returnValueStatusOverridingMembersCompat(
    session: FirSession,
    member: FirCallableDeclaration,
    containingClass: FirClass,
    overridden: List<FirCallableDeclaration>,
): String? = null

fun replaceReturnValueStatusCompat(member: FirCallableDeclaration, statusName: String): Boolean =
    false

fun delegatedMembersCompat(
    session: FirSession,
    scopeSession: ScopeSession,
    klass: FirClass,
    names: Collection<Name>,
): List<Pair<FirCallableDeclaration, List<FirCallableDeclaration>>> = emptyList()

fun isIgnorableByOwnAnnotationCompat(session: FirSession, member: FirCallableDeclaration): Boolean =
    false

fun delegatedWrappedMemberCompat(member: FirCallableDeclaration): FirCallableDeclaration? = null

fun delegatedCopiesCompat(
    session: FirSession,
    scopeSession: ScopeSession,
    klass: FirClass,
    names: Collection<Name>,
): List<FirCallableDeclaration> = emptyList()
