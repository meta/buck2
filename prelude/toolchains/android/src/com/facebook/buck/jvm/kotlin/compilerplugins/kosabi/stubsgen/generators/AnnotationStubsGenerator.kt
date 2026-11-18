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
import com.facebook.kotlin.compilerplugins.kosabi.common.outerClassOnlyQualifier
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.calculateQualifierList
import org.jetbrains.kotlin.psi.KtUserType
import org.jetbrains.kotlin.psi.psiUtil.getChildOfType

/**
 * Tuned stub type to Annotation Class
 * 1. ClassDeclaration's AnnotationEntry
 * 2. (TODO) function parameters
 * 3. (TODO) primary constructor
 */
class AnnotationStubsGenerator : StubsGenerator {
  override fun generateStubs(context: GenerationContext) {
    val candidates =
        context.importedTypes.filterDifferentOuterClassIn(context.declaredTypes).filterNot {
          context.externalTypeReferences.contains(it.outerClassOnlyQualifier())
        }
    val modulePkg = context.packageName()?.split(".").orEmpty()

    context.annotationEntries
        .mapNotNull { it.typeReference?.getChildOfType<KtUserType>() }
        .forEach { annotationType ->
          val genFullQualifier = annotationType.calculateQualifierList()
          val imp =
              context.resolveImportedType(
                  candidates,
                  genFullQualifier.first(),
                  annotationType.containingKtFile,
              )

          val pkg: String
          val name: String
          val inners: List<String>
          if (imp != null) {
            pkg = imp.pkgAsString()
            name = imp.names.first()
            inners = imp.names.drop(1) + genFullQualifier.drop(1)
          } else {
            // No import matches. A qualifier carrying its own package is written out in full; one
            // that carries none names a type of the module's own package. Either way this only
            // retypes a stub that already exists, so it cannot invent an annotation: an annotation
            // left as a plain class is what makes the use site unresolvable.
            val written = FullTypeQualifier(genFullQualifier)
            val qualifier =
                if (written.pkg.isEmpty() && modulePkg.isNotEmpty()) {
                  FullTypeQualifier(modulePkg + genFullQualifier)
                } else {
                  written
                }
            if (qualifier.names.isEmpty()) return@forEach
            pkg = qualifier.pkgAsString()
            name = qualifier.names.first()
            inners = qualifier.names.drop(1)
          }

          val stub =
              context.stubsContainer.find(pkg, name, inners)
                  ?: nestUnderStubbedOuter(context, pkg, name, inners)
          if (stub != null) {
            stub.type = KStub.Type.ANNOTATION
          } else {
            Logger.log(
                """
          |  [Warning] stub not found
          |    - name: $pkg:$name
          |    - inners: $inners
        """
                    .trimMargin(),
            )
          }
        }
  }

  // A nested annotation written only as `@Outer.Inner`, same-package and unimported, reaches no
  // other generator: there is no import for InnerClassStubsGenerator to walk, and
  // SamePackageClassStubsGenerator counts the annotation's own referencedName as a known symbol, so
  // nothing fabricates the inner. Nesting it is only safe when the outer is itself a stub: an outer
  // that is stubbed is off the reduced classpath, so everything nested in it is too, and the inner
  // cannot shadow a type that would otherwise resolve.
  private fun nestUnderStubbedOuter(
      context: GenerationContext,
      pkg: String,
      name: String,
      inners: List<String>,
  ): KStub? {
    if (inners.isEmpty()) return null
    var stubToEdit = context.stubsContainer.find(pkg, name) ?: return null
    var innerPkg = "$pkg.$name"
    for (innerName in inners) {
      val innerStub =
          stubToEdit.innerStubs.find { it.name == innerName }
              ?: KStub(innerPkg, innerName).also { stubToEdit.innerStubs += it }
      innerPkg = "$innerPkg.$innerName"
      stubToEdit = innerStub
    }
    return stubToEdit
  }
}
