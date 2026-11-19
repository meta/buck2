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
@file:OptIn(DirectDeclarationsAccess::class, SymbolInternals::class)

package com.facebook

import java.util.Collections
import java.util.IdentityHashMap
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.backend.FirMetadataSource
import org.jetbrains.kotlin.fir.declarations.DirectDeclarationsAccess
import org.jetbrains.kotlin.fir.declarations.FirCallableDeclaration
import org.jetbrains.kotlin.fir.declarations.FirDeclaration
import org.jetbrains.kotlin.fir.declarations.FirFunction
import org.jetbrains.kotlin.fir.declarations.FirProperty
import org.jetbrains.kotlin.fir.declarations.FirRegularClass
import org.jetbrains.kotlin.fir.declarations.FirTypeAlias
import org.jetbrains.kotlin.fir.declarations.FirTypeParameter
import org.jetbrains.kotlin.fir.declarations.FirTypeParameterRef
import org.jetbrains.kotlin.fir.resolve.substitution.AbstractConeSubstitutor
import org.jetbrains.kotlin.fir.symbols.SymbolInternals
import org.jetbrains.kotlin.fir.types.ConeErrorType
import org.jetbrains.kotlin.fir.types.ConeKotlinType
import org.jetbrains.kotlin.fir.types.FirResolvedTypeRef
import org.jetbrains.kotlin.fir.types.FirTypeRef
import org.jetbrains.kotlin.fir.types.builder.buildResolvedTypeRef
import org.jetbrains.kotlin.fir.types.constructClassLikeType
import org.jetbrains.kotlin.fir.types.contains
import org.jetbrains.kotlin.fir.types.typeContext
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.name.ClassId
import org.jetbrains.kotlin.name.FqName
import org.jetbrains.kotlin.name.Name

/**
 * Meta's patched Kotlin 2.2 and 2.3 compilers write every error type into metadata as
 * `error/NonExistentClass` (`FirJvmSerializerExtension.serializeErrorType`); the stock 2.4.21
 * serializer throws on them. This rewrites the error types in the declarations metadata is
 * serialized from, so codegen emits the same metadata and `ValidationStage` still reports API
 * members that reach the ABI as `error/NonExistentClass`.
 */
internal fun replaceErrorTypesForMetadata(irModuleFragment: IrModuleFragment) {
  for (irFile in irModuleFragment.files) {
    val firFile = (irFile.metadata as? FirMetadataSource.File)?.fir ?: continue
    val replacer = ErrorTypeReplacer(firFile.moduleData.session)
    firFile.declarations.forEach(replacer::replaceIn)
  }
}

private val NON_EXISTENT_CLASS_ID = ClassId(FqName("error"), Name.identifier("NonExistentClass"))

private class ErrorTypeReplacer(session: FirSession) {
  private val visited: MutableSet<FirDeclaration> = Collections.newSetFromMap(IdentityHashMap())

  private val substitutor =
      object : AbstractConeSubstitutor(session.typeContext) {
        override fun substituteType(type: ConeKotlinType): ConeKotlinType? =
            if (type is ConeErrorType) {
              NON_EXISTENT_CLASS_ID.constructClassLikeType(emptyArray(), type.isMarkedNullable)
            } else {
              null
            }
      }

  fun replaceIn(declaration: FirDeclaration) {
    if (!visited.add(declaration)) return
    when (declaration) {
      is FirRegularClass -> {
        declaration.replaceSuperTypeRefs(declaration.superTypeRefs.map(::replaced))
        declaration.typeParameters.forEach(::replaceInTypeParameter)
        declaration.declarations.forEach(::replaceIn)
        // The serializer reads members from the class scope, which can still hold members that
        // IR stripping removed from `declarations`.
        declaration.symbol.declarationSymbols.forEach { replaceIn(it.fir) }
      }
      is FirTypeAlias -> {
        declaration.replaceExpandedTypeRef(replaced(declaration.expandedTypeRef))
        declaration.typeParameters.forEach(::replaceInTypeParameter)
      }
      is FirCallableDeclaration -> {
        declaration.replaceReturnTypeRef(replaced(declaration.returnTypeRef))
        declaration.receiverParameter?.let { it.replaceTypeRef(replaced(it.typeRef)) }
        declaration.contextParameters.forEach(::replaceIn)
        declaration.typeParameters.forEach(::replaceInTypeParameter)
        if (declaration is FirFunction) declaration.valueParameters.forEach(::replaceIn)
        if (declaration is FirProperty) {
          declaration.getter?.let(::replaceIn)
          declaration.setter?.let(::replaceIn)
          declaration.backingField?.let(::replaceIn)
        }
      }
      else -> {}
    }
  }

  private fun replaceInTypeParameter(typeParameterRef: FirTypeParameterRef) {
    val typeParameter = typeParameterRef as? FirTypeParameter ?: return
    typeParameter.replaceBounds(typeParameter.bounds.map(::replaced))
  }

  private fun replaced(typeRef: FirTypeRef): FirTypeRef {
    val resolved = typeRef as? FirResolvedTypeRef ?: return typeRef
    if (!resolved.coneType.contains { it is ConeErrorType }) return typeRef
    return buildResolvedTypeRef {
      source = resolved.source
      coneType = substitutor.substituteOrSelf(resolved.coneType)
      annotations += resolved.annotations
    }
  }
}
