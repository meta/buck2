/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.kosabi.stubsgen.generators

import com.facebook.kotlin.compilerplugins.kosabi.common.FullTypeQualifier
import com.facebook.kotlin.compilerplugins.kosabi.common.Logger
import com.facebook.kotlin.compilerplugins.kosabi.common.filterDifferentOuterClassIn
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.calculateQualifierList
import org.jetbrains.kotlin.psi.KtUserType

/** [InnerClassStubsGenerator] should be after [CtorStubsGenerator] in the generation pipeline. */
class InterfaceStubsGenerator : StubsGenerator {
  override fun generateStubs(context: GenerationContext) {
    // A usage resolves against the imports of ITS OWN file. Pooling every file's imports attributes
    // a simple name to whichever file imported it first, so when two files import different types
    // of the same name the wrong stub is retyped and the real supertype is left a class.
    val candidatesByFile =
        context.importedTypesByFile.mapValues { (_, imports) ->
          imports.filterDifferentOuterClassIn(context.declaredTypes)
        }
    val pooledCandidates = context.importedTypes.filterDifferentOuterClassIn(context.declaredTypes)
    fun candidatesFor(type: KtUserType): List<FullTypeQualifier> =
        candidatesByFile[type.containingKtFile] ?: pooledCandidates

    // Every stubbed bound of a multi-bound type parameter is emitted as an interface. Kotlin
    // permits at most one non-interface bound, so at most one bound of a group can be a class, and
    // the rules stacked with this one require that bound to be written first and to be on the
    // reduced classpath -- where it is the real type, is never stubbed, and holds the class slot on
    // its own. Interface is therefore the sound kind for every bound this pass can reach.
    //
    // A stub is born a class and carries no ClassKind, so leaving stubbed bounds alone corrupts a
    // group whose bounds are all off the classpath: kotlinc hands the single class slot to
    // whichever stub comes first and discards the other class-kinded bounds, which moves the
    // erasure of the parameter.
    //
    // A nested bound cannot be retyped either way -- its enclosing stub would stay a class whose
    // InnerClasses entry then describes an interface member, which javac rejects -- and the
    // decision is whole-group rather than per bound: on facecast's 36-bound clause, retyping the
    // group's OTHER tails still broke resolution of the nested bound at position 13, and dropping
    // any one earlier retype cleared it.
    val multiBoundStubs =
        context.multiBoundGroups
            .filterNot { group -> group.any { nested(context, candidatesFor(it), it) } }
            .flatMap { group -> group.filter { isStubbed(context, candidatesFor(it), it) } }

    // An ambiguous entry is a guess, and the module can settle it: a supertype it writes as a
    // constructor call anywhere is a class, so the guess must not turn that stub into an interface.
    val calledStubs =
        context.superclassCallTypes.mapNotNull {
          stubOf(context, candidatesFor(it), it, log = false)
        }

    for (iType in context.interfaceTypes + multiBoundStubs) {
      val stub = stubOf(context, candidatesFor(iType), iType, log = true) ?: continue
      if (iType in context.ambiguousInterfaceTypes && calledStubs.any { it === stub }) continue
      stub.type = KStub.Type.INTERFACE
    }
  }

  private fun stubOf(
      context: GenerationContext,
      candidates: Collection<FullTypeQualifier>,
      iType: KtUserType,
      log: Boolean,
  ): KStub? {
    val qualifierList = iType.calculateQualifierList()
    val imp = context.resolveImportedType(candidates, qualifierList.first(), iType.containingKtFile)
    val pkg: String
    val name: String
    val inners: List<String>

    if (imp == null) {
      // No import matches. A qualifier carrying its own package is written out in full; one
      // carrying none names a type of the using file's own package, and a trailing segment
      // here names a nested type whatever its case.
      val written = FullTypeQualifier(qualifierList).withMemberAsNestedName()
      val full =
          if (written.pkg.isEmpty()) {
            val filePkg = context.packageSegmentsOf(iType.containingKtFile)
            if (filePkg.isEmpty()) {
              if (log) {
                Logger.log(
                    """
        |  [Warning] ImportTypes not found
        |    - name: $qualifierList
      """
                        .trimMargin(),
                )
              }
              return null
            }
            FullTypeQualifier(filePkg + qualifierList).withMemberAsNestedName()
          } else written
      if (full.names.isEmpty()) return null
      pkg = full.pkgAsString()
      name = full.names.first()
      inners = full.names.drop(1)
    } else {
      pkg = imp.pkgAsString()
      name = imp.names.first()
      inners = imp.names.drop(1) + qualifierList.drop(1)
    }

    val stub = context.stubsContainer.find(pkg, name, inners)
    if (stub == null && log) {
      Logger.log(
          """
          |  [Warning] stub not found
          |    - name: $pkg:$name
          |    - inners: $inners
        """
              .trimMargin(),
      )
    }
    return stub
  }

  // Retyping a nested stub would leave its enclosing stub a class whose InnerClasses entry
  // describes an interface member, and javac rejects that pair while completing the outer.
  private fun nested(
      context: GenerationContext,
      candidates: Collection<FullTypeQualifier>,
      type: KtUserType,
  ): Boolean {
    val qualifierList = type.calculateQualifierList()
    val imp = context.resolveImportedType(candidates, qualifierList.first(), type.containingKtFile)
    return when {
      imp != null ->
          (imp.names.drop(1) + qualifierList.drop(1)).isNotEmpty() ||
              imp.pkgAsString().isNestedOwner()
      qualifierList.size > 1 ->
          FullTypeQualifier(qualifierList).let {
            it.names.drop(1).isNotEmpty() || it.pkgAsString().isNestedOwner()
          }
      else -> false
    }
  }

  // A stub exists exactly when the type is off the reduced classpath. A bound that is not stubbed
  // is the real type and already carries its own kind, so retyping it is neither needed nor safe.
  private fun isStubbed(
      context: GenerationContext,
      candidates: Collection<FullTypeQualifier>,
      first: KtUserType,
  ): Boolean {
    val qualifierList = first.calculateQualifierList()
    val imp = context.resolveImportedType(candidates, qualifierList.first(), first.containingKtFile)
    val pkg: String
    val name: String
    val inners: List<String>
    if (imp != null) {
      pkg = imp.pkgAsString()
      name = imp.names.first()
      inners = imp.names.drop(1) + qualifierList.drop(1)
    } else if (qualifierList.size > 1) {
      val full = FullTypeQualifier(qualifierList)
      pkg = full.pkgAsString()
      name = full.names.first()
      inners = full.names.drop(1)
    } else {
      // A single segment with no import is either declared in this module -- never stubbed -- or
      // a builtin, which is treated as stubbed because that only costs the retyping.
      return context.declaredTypes.none { it.names.last() == qualifierList.first() }
    }
    return context.stubsContainer.find(pkg, name, inners) != null
  }
}

// StubBytecodeRender.internalName treats an uppercase trailing package segment as an outer
// class, so a stub owned by one renders as `Outer$Inner`.
private fun String.isNestedOwner(): Boolean =
    substringAfterLast('.').firstOrNull()?.isUpperCase() == true
