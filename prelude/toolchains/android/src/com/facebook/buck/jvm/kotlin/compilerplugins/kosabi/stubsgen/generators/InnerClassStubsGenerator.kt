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
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.ClassReferenceUtil
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.UserTypeUtil

class InnerClassStubsGenerator : StubsGenerator {

  override fun generateStubs(context: GenerationContext) {
    // Declarations like `A.B.C`
    // UserType => val x: A.B.C = ...
    // ClassReference => @Throws(A.B.C::class)
    val typeNameSegments =
        UserTypeUtil.collectMultiSegmentQualifiers(context) +
            ClassReferenceUtil.collectMultiSegmentQualifiers(context)

    val undeclaredImports =
        (context.importedTypes + context.javaImportedTypes) - context.declaredTypes

    // import: com.A.B
    // usage: B
    val candidatesWithComplexImport = undeclaredImports.filter { it.names.size >= 2 }

    // Promote locally, not in importedTypes: equality is segments-only, so a promoted copy
    // dedupes against the raw import and loses the Outer binding.
    val provenNestedImports =
        undeclaredImports
            .filter { context.isProvenNestedClassImport(it) }
            .map { it.withMemberAsNestedName() }

    // import: com.A
    // usage: A.B
    val candidatesWithComplexUsage =
        typeNameSegments
            .map { segment ->
              // Here we have a match:
              // imp:           com.foo.T1.T2
              // segment:       T2.T3...
              val imp: FullTypeQualifier? =
                  context.resolveImportedType(context.importedTypes, segment.first())
              if (imp == null) {
                Logger.log("  [Warning] ImportedType matches $segment not found!")
              }
              imp to segment
            }
            .filter { (imp, _) -> imp != null }
            .map { (imp, segment) ->
              // Extend the type path, not the raw import: a member-carrying import (a const or
              // fun) would otherwise nest the usage under the member instead of its owner.
              val base = if (imp!!.member != null) imp.segments.dropLast(1) else imp.segments
              FullTypeQualifier(base + segment.drop(1)).withMemberAsNestedName()
            }

    // The member filter runs before the dedupe: equality is segments-only, so deduping first
    // would collapse a promoted nested import against its raw twin and lose the promotion.
    val candidatesToGenerateInnerClass =
        (candidatesWithComplexImport + provenNestedImports + candidatesWithComplexUsage)
            .filter { it.member == null }
            .toSet()
            .filterNot { context.isCompanionCallableImport(it) }
            // We don't want to stub types if they are declared in compilation context
            .filterDifferentOuterClassIn(context.declaredTypes)
            .filterNot { context.externalTypeReferences.contains(it.outerClassOnlyQualifier()) }

    candidatesToGenerateInnerClass.forEach { qualifier ->
      // We should process with:
      // pkg    = com.foo
      // name   = T1
      // inners = T2.T3
      val pkg = qualifier.pkgAsString()
      val name = qualifier.names.first()
      val inners = qualifier.names.drop(1)
      val stub = context.stubsContainer.find(pkg, name)

      stub?.let {
        var stubToEdit = it
        var innerQualifier = stub.name
        for (qualifierPart in inners) {
          // find or create and register
          val innerStub =
              stubToEdit.innerStubs.find { it.name == qualifierPart }
                  ?: KStub(pkg + ".${innerQualifier}", qualifierPart).also { innerStub ->
                    stubToEdit.innerStubs += innerStub
                    Logger.log(
                        """
                      |  [Update stub]
                      |    - name: ${stubToEdit.pkg}:${stubToEdit.name}
                      |    - type: ${stubToEdit.type.name}
                      |    - innerStub:
                      |      - name: ${innerStub.pkg ?: ""}:${innerStub.name}
                      |      - type: ${innerStub.type.name}
                    """
                            .trimMargin(),
                    )
                  }
          innerQualifier = "${innerQualifier}.${qualifierPart}"
          stubToEdit = innerStub
        }
      }
          ?: Logger.log(
              "  [Error] An error occurs to qualifier: $qualifier. We should have an outer $pkg:$name stub at this point",
          )
    }
  }
}
