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
@file:OptIn(SymbolInternals::class)

package com.facebook

import com.facebook.buck.jvm.kotlin.compilerplugins.common.isStub
import org.jetbrains.kotlin.KtPsiSourceElement
import org.jetbrains.kotlin.KtSourceElement
import org.jetbrains.kotlin.cli.common.messages.CompilerMessageLocation
import org.jetbrains.kotlin.cli.common.messages.CompilerMessageSeverity
import org.jetbrains.kotlin.cli.common.messages.MessageCollector
import org.jetbrains.kotlin.com.intellij.psi.JavaPsiFacade
import org.jetbrains.kotlin.com.intellij.psi.JavaRecursiveElementVisitor
import org.jetbrains.kotlin.com.intellij.psi.PsiClass
import org.jetbrains.kotlin.com.intellij.psi.PsiField
import org.jetbrains.kotlin.com.intellij.psi.PsiJavaFile
import org.jetbrains.kotlin.com.intellij.psi.PsiReferenceExpression
import org.jetbrains.kotlin.descriptors.Visibilities
import org.jetbrains.kotlin.descriptors.Visibility
import org.jetbrains.kotlin.fir.FirElement
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.declarations.FirDeclarationOrigin
import org.jetbrains.kotlin.fir.declarations.FirFile
import org.jetbrains.kotlin.fir.declarations.FirMemberDeclaration
import org.jetbrains.kotlin.fir.declarations.FirProperty
import org.jetbrains.kotlin.fir.declarations.FirValueParameter
import org.jetbrains.kotlin.fir.declarations.getRetention
import org.jetbrains.kotlin.fir.declarations.utils.isConst
import org.jetbrains.kotlin.fir.declarations.utils.isStatic
import org.jetbrains.kotlin.fir.declarations.utils.visibility
import org.jetbrains.kotlin.fir.expressions.FirAnnotation
import org.jetbrains.kotlin.fir.expressions.FirBlock
import org.jetbrains.kotlin.fir.expressions.FirExpression
import org.jetbrains.kotlin.fir.expressions.FirQualifiedAccessExpression
import org.jetbrains.kotlin.fir.java.declarations.FirJavaField
import org.jetbrains.kotlin.fir.references.FirResolvedNamedReference
import org.jetbrains.kotlin.fir.resolve.toRegularClassSymbol
import org.jetbrains.kotlin.fir.symbols.SymbolInternals
import org.jetbrains.kotlin.fir.symbols.impl.FirFieldSymbol
import org.jetbrains.kotlin.fir.types.classId
import org.jetbrains.kotlin.fir.types.coneType
import org.jetbrains.kotlin.fir.types.lowerBoundIfFlexible
import org.jetbrains.kotlin.fir.visitors.FirVisitorVoid
import org.jetbrains.kotlin.psi.KtFile

/** How a Kotlin declaration reads the Java constant: the two cases fail differently. */
internal enum class JavaConstantReader {
  /** A `const val` initializer: the constant ships without a ConstantValue. */
  CONST_VAL,
  /** An annotation argument: codegen crashes (`AnnotationCodegen.genCompileTimeValue`). */
  ANNOTATION_ARGUMENT,
}

/**
 * A Kotlin `const val` initializer or annotation argument, on a declaration that another module can
 * read in the source-only ABI, that reads a static final primitive or String field of a Java source
 * of this module whose value source-only ABI generation cannot compute.
 *
 * [providers] are the classes off the source-only ABI classpath that the field's initializer reads,
 * as fully qualified names known with certainty; [ambiguousProviders] are names an on-demand import
 * makes ambiguous, and [uncertainProviders] names whose class cannot be determined with certainty.
 * Only [providers] get a marker line.
 */
internal data class UnavailableJavaConstantRead(
    val kind: JavaConstantReader,
    val location: CompilerMessageLocation?,
    val reader: String,
    val field: String,
    val providers: List<String>,
    val ambiguousProviders: List<String>,
    val uncertainProviders: List<String>,
)

/**
 * Finds [UnavailableJavaConstantRead]s in the source files of [outputs]. Only declaration-level
 * positions are walked: function and accessor bodies are not part of the source-only ABI.
 */
internal fun findUnavailableJavaConstantReads(
    outputs: List<Pair<FirSession, List<FirFile>>>,
): List<UnavailableJavaConstantRead> {
  val reads = linkedSetOf<UnavailableJavaConstantRead>()
  for ((session, files) in outputs) {
    for (file in files) {
      DeclarationWalker(session, reads).walk(file, visible = true)
    }
  }
  return reads.toList()
}

/**
 * Reports each read. A `const val` read follows the shared escape hatch (warn mode reports a
 * warning, as for `KOSABI_MISSING_CONST_DEP`); an annotation argument is always an error, because
 * codegen cannot emit it. Returns true when an error was reported, so the caller must not run the
 * backend.
 */
internal fun reportUnavailableJavaConstantReads(
    reads: List<UnavailableJavaConstantRead>,
    policy: AbiRepairPolicy,
    messageCollector: MessageCollector,
): Boolean {
  var reportedError = false
  for (read in reads) {
    val severity =
        if (read.kind == JavaConstantReader.CONST_VAL && policy == AbiRepairPolicy.WARN) {
          CompilerMessageSeverity.WARNING
        } else {
          reportedError = true
          CompilerMessageSeverity.ERROR
        }
    val cause =
        if (
            read.providers.isEmpty() &&
                read.ambiguousProviders.isEmpty() &&
                read.uncertainProviders.isEmpty()
        ) {
          "whose value source-only ABI generation cannot compute."
        } else {
          "whose value needs an unavailable provider during source-only ABI generation."
        }
    val fix = buildString {
      append(" Without it the source-only ABI loses the constant.")
      if (read.providers.isNotEmpty()) {
        append(" Add the target that provides ")
        append(read.providers.joinToString { "`$it`" })
        append(" to THIS target's `source_only_abi_deps`.")
      }
      if (read.ambiguousProviders.isNotEmpty()) {
        append(" Provider ambiguous (on-demand import): ")
        append(read.ambiguousProviders.joinToString { "`$it`" })
        append("; import the class explicitly to name it.")
      }
      if (read.uncertainProviders.isNotEmpty()) {
        append(" Provider uncertain: ")
        append(read.uncertainProviders.joinToString { "`$it`" })
        append(" does not name a class with certainty; import it explicitly to name it.")
      }
      if (
          read.providers.isEmpty() &&
              read.ambiguousProviders.isEmpty() &&
              read.uncertainProviders.isEmpty()
      ) {
        append(" The provider could not be determined from the Java initializer.")
      }
      for (provider in read.providers) {
        append("\nSOURCE_ONLY_ABI_MISSING_CONSTANT_PROVIDERS: ")
        append(provider)
      }
    }
    messageCollector.report(
        severity,
        "${read.reader} reads ${read.field}, $cause$fix",
        read.location,
    )
  }
  return reportedError
}

private class DeclarationWalker(
    private val session: FirSession,
    private val reads: MutableSet<UnavailableJavaConstantRead>,
) {
  /**
   * Visits [element] knowing whether its enclosing declarations survive into the source-only ABI
   * where another module can read them. Private and local declarations, and everything inside them,
   * do not; internal ones do (friend modules read them).
   */
  fun walk(element: FirElement, visible: Boolean) {
    when (element) {
      is FirAnnotation -> if (visible) checkAnnotation(element)
      is FirBlock,
      is FirExpression -> return
      else -> {
        val ownVisible =
            visible &&
                (element !is FirMemberDeclaration ||
                    element is FirValueParameter ||
                    !element.visibility.isHidden())
        element.acceptChildren(
            object : FirVisitorVoid() {
              override fun visitElement(element: FirElement) {
                walk(element, ownVisible)
              }
            },
        )
        if (element is FirProperty && element.isConst && ownVisible) {
          element.initializer?.let {
            collect(it, JavaConstantReader.CONST_VAL, "const val ${element.name}", element.source)
          }
        }
      }
    }
  }

  private fun Visibility.isHidden(): Boolean =
      this == Visibilities.Private ||
          this == Visibilities.PrivateToThis ||
          this == Visibilities.Local

  private fun checkAnnotation(annotation: FirAnnotation) {
    val annotationClass =
        annotation.annotationTypeRef.coneType.toRegularClassSymbol(session) ?: return
    val retention = runCatching { annotationClass.getRetention(session) }.getOrNull()
    if (retention == AnnotationRetention.SOURCE) return
    collect(
        annotation,
        JavaConstantReader.ANNOTATION_ARGUMENT,
        "annotation @${annotationClass.name} argument",
        annotation.source,
    )
  }

  private fun collect(
      root: FirElement,
      kind: JavaConstantReader,
      reader: String,
      readerSource: KtSourceElement?,
  ) {
    root.accept(
        object : FirVisitorVoid() {
          override fun visitElement(element: FirElement) {
            if (element is FirQualifiedAccessExpression) {
              unavailableJavaConstant(element)?.let { field ->
                val providers = providersOf(field)
                reads +=
                    UnavailableJavaConstantRead(
                        kind,
                        location(element.source ?: readerSource),
                        reader,
                        "${field.symbol.callableId.asSingleFqName()}",
                        providers.resolved.toList(),
                        providers.ambiguous.toList(),
                        providers.uncertain.toList(),
                    )
              }
            }
            element.acceptChildren(this)
          }
        },
    )
  }

  /**
   * The Java field [access] reads when it is a static final primitive or String field declared in a
   * Java source of this module (origin `Java.Source`: only this compilation's sources have it) and
   * has no evaluated value.
   */
  private fun unavailableJavaConstant(access: FirQualifiedAccessExpression): FirJavaField? {
    val symbol =
        (access.calleeReference as? FirResolvedNamedReference)?.resolvedSymbol as? FirFieldSymbol
            ?: return null
    val field = symbol.fir as? FirJavaField ?: return null
    if (field.origin != FirDeclarationOrigin.Java.Source) return null
    if (!field.isStatic || !field.isVal) return null
    val type = field.returnTypeRef.coneType.lowerBoundIfFlexible().classId?.asString()
    if (type !in CONSTANT_TYPES) return null
    return field.takeIf { it.initializer == null }
  }
}

/** The JVM types a Java compile-time constant field can have (JLS 4.12.4). */
private val CONSTANT_TYPES = setOf(
    "kotlin/Boolean",
    "kotlin/Byte",
    "kotlin/Char",
    "kotlin/Short",
    "kotlin/Int",
    "kotlin/Long",
    "kotlin/Float",
    "kotlin/Double",
    "kotlin/String",
)

private fun location(source: KtSourceElement?): CompilerMessageLocation? {
  val psi = (source as? KtPsiSourceElement)?.psi ?: return null
  val file = psi.containingFile ?: return null
  val path = file.virtualFile?.path ?: file.name
  val document = file.viewProvider.document ?: return CompilerMessageLocation.create(path)
  val offset = psi.textRange.startOffset
  val line = document.getLineNumber(offset)
  val column = offset - document.getLineStartOffset(line)
  return CompilerMessageLocation.create(path, line + 1, column + 1, null)
}

private class Providers {
  /** Fully qualified names known with certainty. */
  val resolved = linkedSetOf<String>()
  /** Source names an on-demand import makes ambiguous. */
  val ambiguous = linkedSetOf<String>()
  /** Source names whose class cannot be determined with certainty. */
  val uncertain = linkedSetOf<String>()
}

/**
 * The classes off the source-only ABI classpath that [field]'s initializer reads, following
 * same-module Java constants whose value is also lost.
 */
private fun providersOf(field: FirJavaField): Providers {
  val providers = Providers()
  val psi = (field.source as? KtPsiSourceElement)?.psi as? PsiField ?: return providers
  collectProviders(psi, providers, mutableSetOf())
  return providers
}

private fun collectProviders(field: PsiField, out: Providers, seen: MutableSet<PsiField>) {
  if (!seen.add(field)) return
  val initializer = field.initializer ?: return
  val file = field.containingFile as? PsiJavaFile
  initializer.accept(
      object : JavaRecursiveElementVisitor() {
        override fun visitReferenceExpression(expression: PsiReferenceExpression) {
          when (val target = expression.resolve()) {
            null -> {
              providerOfUnresolved(expression, file, out)
              return
            }
            is PsiField ->
                if (target.computeConstantValue() == null) {
                  if (target.containingFile?.virtualFile?.extension == "java") {
                    collectProviders(target, out, seen)
                  } else {
                    target.containingClass?.qualifiedName?.let(out.resolved::add)
                  }
                }
          }
          super.visitReferenceExpression(expression)
        }
      },
  )
}

/**
 * Records the class an unresolved `Prov.Y`, `Outer.Inner.Y`, `pkg.Prov.Y` or statically imported
 * `Y` reads. Only resolution steps with a certain answer name a class: a single-type import of the
 * qualifier's first name, a single static import of it (a nested type), a qualifier that resolves
 * to a real class, or a prefix that resolves to a package. Anything else is uncertain (an inherited
 * member type, a same-package class, a fabricated stub) or ambiguous (an on-demand import): no
 * class is named for it, so no marker is printed.
 */
private fun providerOfUnresolved(
    expression: PsiReferenceExpression,
    file: PsiJavaFile?,
    out: Providers,
) {
  val imports = file?.importList
  val onDemand =
      imports?.importStatements?.any { it.isOnDemand } == true ||
          imports?.importStaticStatements?.any { it.isOnDemand } == true
  val qualifier = expression.qualifierExpression as? PsiReferenceExpression
  if (qualifier == null) {
    val name = expression.referenceName ?: return
    val importedClass =
        imports
            ?.importStaticStatements
            ?.firstOrNull { !it.isOnDemand && it.referenceName == name }
            ?.importReference
            ?.qualifier
            ?.text
    when {
      importedClass != null -> out.resolved += importedClass.withoutWhitespace()
      onDemand -> out.ambiguous += name
      else -> out.uncertain += name
    }
    return
  }
  val text = qualifier.text.withoutWhitespace()
  val head = text.substringBefore('.')
  val rest = text.substringAfter('.', "")
  fun withRest(name: String) = if (rest.isEmpty()) name else "$name.$rest"

  val singleTypeImport =
      imports
          ?.importStatements
          ?.firstOrNull { !it.isOnDemand && it.qualifiedName?.substringAfterLast('.') == head }
          ?.qualifiedName
  if (singleTypeImport != null) {
    out.resolved += withRest(singleTypeImport)
    return
  }
  val staticNestedType =
      imports
          ?.importStaticStatements
          ?.firstOrNull { !it.isOnDemand && it.referenceName == head }
          ?.importReference
          ?.text
  if (staticNestedType != null) {
    out.resolved += withRest(staticNestedType.withoutWhitespace())
    return
  }
  val resolvedClass = qualifier.resolve() as? PsiClass
  if (resolvedClass != null && !resolvedClass.isKosabiStub()) {
    resolvedClass.qualifiedName?.let {
      out.resolved += it
      return
    }
  }
  packageQualifiedClass(text, file)?.let {
    out.resolved += it
    return
  }
  if (onDemand) out.ambiguous += text else out.uncertain += text
}

/**
 * `a.b.Prov.Inner` as the class `a.b.Prov.Inner` when its longest proper prefix that names a
 * package is `a.b`: Java reads a qualified name as package-qualified only through a package that
 * exists.
 */
private fun packageQualifiedClass(text: String, file: PsiJavaFile?): String? {
  val project = file?.project ?: return null
  val segments = text.split('.')
  val facade = JavaPsiFacade.getInstance(project)
  for (end in segments.size - 1 downTo 1) {
    val prefix = segments.subList(0, end).joinToString(".")
    if (runCatching { facade.findPackage(prefix) }.getOrNull() != null) return text
  }
  return null
}

/** A class Kosabi's stub generator wrote: its package may be a guess. */
private fun PsiClass.isKosabiStub(): Boolean {
  val ktFile = navigationElement?.containingFile as? KtFile
  if (ktFile != null && ktFile.isStub()) return true
  val path = containingFile?.virtualFile?.path ?: return false
  return path.contains("stubgen_stubs") || path.contains("/stubs.jar!")
}

private fun String.withoutWhitespace(): String = filterNot { it.isWhitespace() }
