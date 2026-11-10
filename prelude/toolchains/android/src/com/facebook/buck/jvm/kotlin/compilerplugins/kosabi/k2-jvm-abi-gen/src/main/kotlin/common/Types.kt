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
)

package com.facebook

import com.facebook.kotlin.compilercompat.compatValueParameters
import org.jetbrains.kotlin.GeneratedDeclarationKey
import org.jetbrains.kotlin.descriptors.Modality
import org.jetbrains.kotlin.fir.types.ConeKotlinType
import org.jetbrains.kotlin.ir.declarations.IrSimpleFunction
import org.jetbrains.kotlin.ir.types.classFqName
import org.jetbrains.kotlin.name.Name

data object JvmAbiGenPlugin : GeneratedDeclarationKey()

data class InterfaceMethodInfo(
    val name: Name,
    val returnType: ConeKotlinType,
    val valueParameters: List<Pair<Name, ConeKotlinType>>,
    val modality: Modality,
)

internal data class MethodSignature(
    val name: String,
    val parameterTypes: List<String>,
)

internal fun IrSimpleFunction.methodSignature() = MethodSignature(
    name.asString(),
    compatValueParameters.map { it.type.classFqName?.asString() ?: it.type.toString() },
)

// Toolchain-provided packages dep tracking may skip. The trailing dots are load-bearing: `javax.*`
// and `kotlinx.*` are dependency namespaces (`javax.inject`, `kotlinx.coroutines`), not stdlib/JDK.
internal fun isStdlibOrJdkPackage(packageName: String): Boolean =
    packageName == "kotlin" ||
        packageName.startsWith("kotlin.") ||
        packageName == "java" ||
        packageName.startsWith("java.")
