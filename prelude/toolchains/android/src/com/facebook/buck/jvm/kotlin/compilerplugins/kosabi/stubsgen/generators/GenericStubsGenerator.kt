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
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.calculateQualifierList
import java.util.Collections
import java.util.IdentityHashMap
import org.jetbrains.kotlin.psi.KtBinaryExpressionWithTypeRHS
import org.jetbrains.kotlin.psi.KtCallExpression
import org.jetbrains.kotlin.psi.KtElement
import org.jetbrains.kotlin.psi.KtFile
import org.jetbrains.kotlin.psi.KtIsExpression
import org.jetbrains.kotlin.psi.KtNameReferenceExpression
import org.jetbrains.kotlin.psi.KtNamedFunction
import org.jetbrains.kotlin.psi.KtNullableType
import org.jetbrains.kotlin.psi.KtQualifiedExpression
import org.jetbrains.kotlin.psi.KtTypeArgumentList
import org.jetbrains.kotlin.psi.KtTypeReference
import org.jetbrains.kotlin.psi.KtUserType
import org.jetbrains.kotlin.psi.KtWhenConditionIsPattern
import org.jetbrains.kotlin.psi.psiUtil.anyDescendantOfType
import org.jetbrains.kotlin.psi.psiUtil.collectDescendantsOfType

class GenericStubsGenerator : StubsGenerator {
  override fun generateStubs(context: GenerationContext) {
    // find all the types in the project files, and the calls that write type arguments
    val elements =
        context.projectFiles.flatMap { file ->
          file.collectDescendantsOfType<KtElement> {
            it is KtUserType || (it is KtCallExpression && it.typeArgumentList != null)
          }
        }
    val userTypes = elements.filterIsInstance<KtUserType>()
    val usedGenericTypes: List<KtUserType> =
        userTypes
            // check if this is generic one
            .filter { it.anyDescendantOfType<KtTypeArgumentList>() }
            // Deduplication is per file: the same text in two files can name two different types,
            // and each file's usage has to reach its own stub.
            .distinctBy { it.containingKtFile to it.text }

    // TODO: Do not apply for SDK classes
    // A usage resolves against the imports of ITS OWN file. Pooling every file's imports attributes
    // a simple name to whichever file imported it first, so two files importing different types of
    // the same name give one stub both arities and leave the other bare.
    val candidatesByFile =
        context.importedTypesByFile.mapValues { (_, imports) -> imports - context.declaredTypes }
    val pooledCandidates = context.importedTypes - context.declaredTypes
    for (genType in usedGenericTypes) {
      val target =
          context.stubTarget(
              genType.containingKtFile,
              genType.calculateQualifierList(),
              candidatesByFile,
              pooledCandidates,
          ) ?: continue

      val stub =
          context.stubsContainer.find(
              target.pkg,
              target.name,
              // Case 1:
              // name = com.A.B
              // genFullQualifier = B
              // Case 2:
              // name = com.A
              // genFullQualifier = A.B
              target.innerClassNames,
          )

      if (stub != null) {
        stub.genericTypes = genType.typeArguments.size
      } else {
        Logger.log(
            """
          |  [Warning] stub not found
          |    - name: ${target.pkg}:${target.name}
          |    - inner class names: ${target.innerClassNames}
        """
                .trimMargin(),
        )
      }
    }

    // An inferred member type such as `val KEY = Key<Gateway>("key")` writes its class's arity only
    // as the explicit type arguments of the constructor call. Written types decide. A type written
    // with arguments, in any position, gave its stub an arity above. A type written without them
    // proves the class declares no type parameters, except as an `is`/`as` target (which may omit
    // inferable arguments) or as the qualifier of a nested type. A call gives an arity only to a
    // stub that no written type names, and only when all such calls agree: two calls that disagree
    // prove one of them is a same-named factory, so the stub keeps what it gets without calls. A
    // call whose callee names a top-level function of the module that the calling file sees (in its
    // own package, or imported) may call that function.
    fun findStub(file: KtFile, writtenQualifier: List<String>): KStub? =
        context.stubTarget(file, writtenQualifier, candidatesByFile, pooledCandidates)?.let {
          context.stubsContainer.find(it.pkg, it.name, it.innerClassNames)
        }
    val writtenStubs = Collections.newSetFromMap(IdentityHashMap<KStub, Boolean>())
    userTypes
        .filter {
          it.typeArgumentList != null || (it.parent !is KtUserType && !it.isCastOrCheckTarget())
        }
        .mapNotNullTo(writtenStubs) { findStub(it.containingKtFile, it.calculateQualifierList()) }
    val functionPackages = HashMap<String, MutableSet<String>>()
    for (file in context.projectFiles) {
      for (function in file.declarations.filterIsInstance<KtNamedFunction>()) {
        val name = function.name ?: continue
        functionPackages.getOrPut(name) { HashSet() }.add(file.packageFqName.asString())
      }
    }
    fun mayCallFunction(file: KtFile, name: String): Boolean {
      if (file.packageFqName.asString() in functionPackages[name].orEmpty()) return true
      return file.importDirectives.any { import ->
        val imported = import.importedFqName ?: return@any false
        if (import.isAllUnder) {
          imported.asString() in functionPackages[name].orEmpty()
        } else {
          val simpleName = imported.shortName().asString()
          (import.aliasName ?: simpleName) == name &&
              imported.parent().asString() in functionPackages[simpleName].orEmpty()
        }
      }
    }
    val callArities = IdentityHashMap<KStub, MutableSet<Int>>()
    for (call in elements.filterIsInstance<KtCallExpression>()) {
      val callee = call.calleeExpression as? KtNameReferenceExpression ?: continue
      // Only an unqualified callee resolves through the file's imports and package.
      if ((call.parent as? KtQualifiedExpression)?.selectorExpression == call) continue
      val file = call.containingKtFile
      val name = callee.getReferencedName()
      if (mayCallFunction(file, name)) continue
      val stub = findStub(file, listOf(name)) ?: continue
      if (stub !in writtenStubs) {
        callArities.getOrPut(stub) { HashSet() }.add(call.typeArguments.size)
      }
    }
    for ((stub, arities) in callArities) {
      if (arities.size == 1) stub.genericTypes = arities.single()
    }
  }
}

private fun KtUserType.isCastOrCheckTarget(): Boolean {
  val typeReference = (parent as? KtNullableType)?.parent ?: parent
  val owner = (typeReference as? KtTypeReference)?.parent
  return owner is KtIsExpression ||
      owner is KtWhenConditionIsPattern ||
      owner is KtBinaryExpressionWithTypeRHS
}

private class StubTarget(val pkg: String, val name: String, val innerClassNames: List<String>)

/** The stub a type usage written as [writtenQualifier] in [file] names, if it names one. */
private fun GenerationContext.stubTarget(
    file: KtFile,
    writtenQualifier: List<String>,
    candidatesByFile: Map<KtFile, Set<FullTypeQualifier>>,
    pooledCandidates: Set<FullTypeQualifier>,
): StubTarget? {
  val candidates = candidatesByFile[file] ?: pooledCandidates
  val imported = resolveImportedType(candidates, writtenQualifier.first())
  val imp: FullTypeQualifier
  val innerClassNames: List<String>
  if (imported != null) {
    imp = imported
    innerClassNames =
        if (writtenQualifier == imp.segments) emptyList()
        else (imp.names.drop(1) + writtenQualifier.drop(1))
  } else {
    // No import matches. A qualifier carrying its own package is written out in full; one
    // that carries none names a type of the using file's own package. Either way the type
    // arguments belong to the innermost name, and a trailing segment in this type slot names
    // a nested type whatever its case.
    val written = FullTypeQualifier(writtenQualifier).withMemberAsNestedName()
    val resolved =
        if (written.pkg.isEmpty()) {
          val filePkg = packageSegmentsOf(file)
          if (filePkg.isEmpty()) return null
          FullTypeQualifier(filePkg + writtenQualifier).withMemberAsNestedName()
        } else written
    imp = resolved
    innerClassNames = resolved.names.drop(1)
  }
  val name = imp.names
  // An all-caps simple name reads as a static-const member, so the qualifier carries no class
  // name to look up.
  if (name.isEmpty()) return null
  return StubTarget(imp.pkgAsString(), name.first(), innerClassNames)
}
