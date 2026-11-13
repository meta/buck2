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

import com.facebook.kotlin.compilerplugins.kosabi.common.Logger
import com.facebook.kotlin.compilerplugins.kosabi.common.filterDifferentOuterClassIn
import com.facebook.kotlin.compilerplugins.kosabi.common.outerClassOnlyQualifier
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub

class FullQualifiedClassStubsGenerator : StubsGenerator {

  override fun generateStubs(context: GenerationContext) {
    (context.fullQualifierTypes + context.javaFullQualifierTypes)
        .filterDifferentOuterClassIn(context.declaredTypes)
        .filterNot { context.externalTypeReferences.contains(it.outerClassOnlyQualifier()) }
        .filter { it.pkg.isNotEmpty() && it.names.isNotEmpty() }
        // As in PrimitiveStubsGenerator: a written `kotlinx.coroutines.CancellationException` names
        // a typealias, and a phantom class stub would shadow it in every file of the module.
        .filterNot { context.kotlinClasspathDeclarations.isProvided(it.pkg, it.names.first()) }
        .forEach { qualifier ->
          val pkg = qualifier.pkgAsString()
          // create a root stub or register a new one
          val stub =
              context.stubsContainer.find(pkg, qualifier.names.first())
                  ?: KStub(pkg, qualifier.names.first()).also { context.stubsContainer.add(it) }

          var stubToEdit = stub
          for (className in qualifier.names.drop(1)) {
            val innerStub =
                stubToEdit.innerStubs.find { it.name == className }
                    ?: KStub(stubToEdit.pkg + ".${stubToEdit.name}", className).also { innerStub ->
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
            stubToEdit = innerStub
          }
        }
  }
}
