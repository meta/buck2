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
import com.facebook.kotlin.compilerplugins.kosabi.common.JavaLangDefaultImports
import com.facebook.kotlin.compilerplugins.kosabi.common.outerClassOnlyQualifier
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.PlainKTBuiltInTypes
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.PlainKTJavaTypeAlias
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.PlainKTStdlibTypes
import org.jetbrains.kotlin.psi.KtFile
import org.jetbrains.kotlin.psi.KtUserType
import org.jetbrains.kotlin.psi.psiUtil.getChildOfType

// Generate stubs for all unknown classes that are not declared from the module, nor from imported
// or class paths
// This is to handle cases like we have dependent module that use same package name as the current
// module. (Like a GraphQL model
// In this case explicit import for usage of those classes are not needed)
class SamePackageClassStubsGenerator : StubsGenerator {

  override fun generateStubs(context: GenerationContext) {

    val modulePkgName = context.packageName()
    val allKnownSymbols = mutableSetOf<String>()

    // Kotlin/Java built-in, stdlib types
    val sdkInternalTypes = setOf(
        PlainKTStdlibTypes,
        PlainKTBuiltInTypes,
        PlainKTJavaTypeAlias,
    )
        .flatMap { it.all() }

    allKnownSymbols.addAll(sdkInternalTypes)
    // `java.lang` default imports: the static list and the classpath's `java.lang` types. The
    // applicability checks read the same set (JavaLangDefaultImports).
    allKnownSymbols.addAll(JavaLangDefaultImports.knownNames(context.externalTypeReferences))

    // Imported types and declared types. Other classpath types (android.*, javax.*, ...) need
    // explicit imports and are not included here, so their simple names do not hide same-package
    // types.
    val generatedTypesInModulePackage =
        context.knownGeneratedTypes.filter { it.pkgAsString() == modulePkgName }
    // A qualifier that carries no package (`Outer.Inner`) and whose outer no import,
    // declaration, or classpath entry owns names a nested type of the module's own
    // package. FullQualifiedClassStubsGenerator drops it for having no package and
    // InnerClassStubsGenerator can only reach it through an import, so nothing creates its outer:
    // counting that outer as known here is what leaves the usage with no stub at all.
    val nestedSamePackageQualifiers =
        context.fullQualifierTypes.filter {
          it.pkg.isEmpty() && it.member == null && it.names.size > 1
        }
    allKnownSymbols.addAll(
        (context.declaredTypes +
                generatedTypesInModulePackage +
                (context.fullQualifierTypes - nestedSamePackageQualifiers.toSet()))
            .flatMap { it.segments + it.names },
    )
    allKnownSymbols.addAll(nestedSamePackageQualifiers.flatMap { it.names.drop(1) })

    // Alias & type parameter names
    allKnownSymbols.addAll(context.typeAliasSymbol + context.importAlias + context.parameterNames)

    // Annotations
    allKnownSymbols.addAll(
        context.annotationEntries
            .mapNotNull { it.typeReference?.getChildOfType<KtUserType>() }
            .mapNotNull { it.referencedName },
    )

    // Imported types. A name that a file's import gives up for its alias (`import a.B as Alias`)
    // is not known in that file through the import: there the plain `B` names a type of the file's
    // own package, which needs the stub below.
    val importedSymbols = context.importedTypes.flatMapTo(mutableSetOf()) { it.segments + it.names }
    fun isKnownIn(file: KtFile, name: String): Boolean =
        name in allKnownSymbols ||
            (name in importedSymbols && name !in context.aliasHiddenNamesByFile[file].orEmpty())

    // A stub belongs to the package of the file that uses the type: the module's files can span
    // packages (examples, generated sources), and the first file's package is the wrong owner for
    // any other file's same-package usages.
    val maybeUnknownByPkg: Map<String, Set<String>> =
        context.usedUserTypes
            .groupBy { it.containingKtFile }
            .map { (file, usages) ->
              val pkgSegments = context.packageSegmentsOf(file)
              val pkg = pkgSegments.joinToString(".")
              pkg to
                  usages
                      .mapNotNull { userType -> userType.referencedName }
                      .toSet()
                      .filterNot { name -> isKnownIn(file, name) }
                      .filter { it -> it.first().isUpperCase() }
                      .filterNot { name ->
                        // A stub shadows the real type (same FQN, sources win), so the ABI
                        // compiles against the stub: an open stub where the real type is final
                        // flips generic wildcard elision and corrupts signatures. The match is
                        // by exact FQN, so same-name types in other packages still stub.
                        val candidate =
                            FullTypeQualifier(pkgSegments + name).outerClassOnlyQualifier()
                        candidate != null && context.externalTypeReferences.contains(candidate)
                      }
                      .filterNot { name ->
                        // A same-package typealias of a dependency has no class file, so the
                        // check above misses it, and a class stub with its FQN wins over the
                        // typealias: the ABI then names the stub where kotlinc writes the
                        // expanded type.
                        context.kotlinClasspathDeclarations.isProvided(pkgSegments, name)
                      }
                      .filterNot { name ->
                        // A star import of a classpath or JDK package (`import java.util.*`)
                        // provides the name. A same-package stub would win over the star import,
                        // so the ABI would name the stub where kotlinc writes the imported type.
                        file.importDirectives.any { directive ->
                          directive.isAllUnder &&
                              directive.importedFqName?.let { fqName ->
                                val starPkg = fqName.pathSegments().map { it.asString() }
                                context.kotlinClasspathDeclarations.isProvided(starPkg, name) ||
                                    isJdkClass(starPkg, name)
                              } == true
                        }
                      }
                      .toSet()
            }
            .groupBy({ (pkg, _) -> pkg }, { (_, names) -> names })
            .mapValues { (_, names) -> names.flatten().toSet() }
            .filter { (pkg, names) -> pkg.isNotEmpty() && names.isNotEmpty() }

    maybeUnknownByPkg.forEach { (pkg, typeNames) ->
      typeNames.forEach { typeName ->
        if (context.stubsContainer.find(pkg, typeName) == null) {
          context.stubsContainer.add(KStub(pkg, typeName))
        }
      }
    }

    // The usage is `Outer.Inner`, so the outer stub fabricated above still has to own an Inner for
    // the reference to resolve. Only outers fabricated here are nested into: an outer resolved any
    // other way already has its own owner, and its nesting belongs to that owner's generator.
    context.fullQualifierTypesByFile.forEach { (file, qualifiers) ->
      val pkg = context.packageSegmentsOf(file).joinToString(".")
      val fabricated = maybeUnknownByPkg[pkg].orEmpty()
      qualifiers
          .filter { it.pkg.isEmpty() && it.member == null && it.names.size > 1 }
          .filter { it.names.first() in fabricated }
          .forEach { qualifier ->
            var stubToEdit =
                context.stubsContainer.find(pkg, qualifier.names.first()) ?: return@forEach
            var innerPkg = "$pkg.${stubToEdit.name}"
            for (innerName in qualifier.names.drop(1)) {
              val innerStub =
                  stubToEdit.innerStubs.find { it.name == innerName }
                      ?: KStub(innerPkg, innerName).also { stubToEdit.innerStubs += it }
              innerPkg = "$innerPkg.$innerName"
              stubToEdit = innerStub
            }
          }
    }
  }
}

// The JDK is not on the stubsgen classpath of a JVM compilation (kotlinc reads it from the JDK
// home),
// so its classes are looked up in the compiler's own JDK. The platform class loader sees the JDK
// modules only, not the compiler's classpath.
private fun isJdkClass(pkgSegments: List<String>, name: String): Boolean =
    pkgSegments.isNotEmpty() &&
        ClassLoader.getPlatformClassLoader()
            .getResource(pkgSegments.joinToString("/") + "/" + name + ".class") != null
