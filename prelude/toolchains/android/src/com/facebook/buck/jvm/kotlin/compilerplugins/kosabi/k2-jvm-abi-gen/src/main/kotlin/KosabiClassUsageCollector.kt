/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:Suppress("OPT_IN_USAGE_ERROR")
@file:OptIn(
    com.facebook.DirectDeclarationsAccessCompat::class,
)

package com.facebook

import java.io.BufferedWriter
import java.io.File
import java.io.FileWriter
import java.net.URI
import org.jetbrains.kotlin.cli.jvm.config.jvmClasspathRoots
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.config.CompilerConfigurationKey
import org.jetbrains.kotlin.descriptors.ClassKind
import org.jetbrains.kotlin.descriptors.Visibilities
import org.jetbrains.kotlin.fir.FirElement
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.declarations.FirAnonymousObject
import org.jetbrains.kotlin.fir.declarations.FirCallableDeclaration
import org.jetbrains.kotlin.fir.declarations.FirClassLikeDeclaration
import org.jetbrains.kotlin.fir.declarations.FirDeclarationOrigin
import org.jetbrains.kotlin.fir.declarations.FirEnumEntry
import org.jetbrains.kotlin.fir.declarations.FirReceiverParameter
import org.jetbrains.kotlin.fir.declarations.FirRegularClass
import org.jetbrains.kotlin.fir.declarations.FirTypeAlias
import org.jetbrains.kotlin.fir.declarations.FirTypeParameter
import org.jetbrains.kotlin.fir.declarations.FirTypeParameterRef
import org.jetbrains.kotlin.fir.declarations.FirValueParameter
import org.jetbrains.kotlin.fir.declarations.utils.classId
import org.jetbrains.kotlin.fir.declarations.utils.isConst
import org.jetbrains.kotlin.fir.declarations.utils.sourceElement
import org.jetbrains.kotlin.fir.expressions.FirAnnotation
import org.jetbrains.kotlin.fir.expressions.FirAnnotationCall
import org.jetbrains.kotlin.fir.expressions.FirQualifiedAccessExpression
import org.jetbrains.kotlin.fir.expressions.FirResolvedQualifier
import org.jetbrains.kotlin.fir.java.declarations.FirJavaClass
import org.jetbrains.kotlin.fir.references.symbol
import org.jetbrains.kotlin.fir.resolve.providers.symbolProvider
import org.jetbrains.kotlin.fir.symbols.SymbolInternals
import org.jetbrains.kotlin.fir.symbols.impl.FirCallableSymbol
import org.jetbrains.kotlin.fir.types.ConeClassLikeType
import org.jetbrains.kotlin.fir.types.ConeErrorType
import org.jetbrains.kotlin.fir.types.ConeKotlinType
import org.jetbrains.kotlin.fir.types.ConeKotlinTypeProjection
import org.jetbrains.kotlin.fir.types.FirTypeRef
import org.jetbrains.kotlin.fir.types.classId
import org.jetbrains.kotlin.fir.types.coneType
import org.jetbrains.kotlin.fir.types.resolvedType
import org.jetbrains.kotlin.fir.types.upperBoundIfFlexible
import org.jetbrains.kotlin.fir.visitors.FirDefaultVisitorVoid
import org.jetbrains.kotlin.load.java.structure.JavaClass
import org.jetbrains.kotlin.load.java.structure.JavaClassifier
import org.jetbrains.kotlin.load.java.structure.JavaTypeParameter
import org.jetbrains.kotlin.load.java.structure.impl.VirtualFileBoundJavaClass
import org.jetbrains.kotlin.load.kotlin.KotlinJvmBinarySourceElement
import org.jetbrains.kotlin.name.ClassId

private const val JAR_FILE_SEPARATOR = "!/"
private const val STUBSGEN_STUBS_JAR = "stubgen_stubs.jar"

/**
 * Collects classpath class usage from a resolved FIR tree for dep file tracking.
 *
 * This replaces the DependencyTracker compiler plugin for kosabi's inner compilation session, since
 * kosabi creates a separate project environment where the dep-tracker's FIR checker extensions are
 * not registered, and kosabi skips the checker phase entirely.
 *
 * Instead, we walk the already-resolved FIR tree post-analysis and extract class usage from
 * declaration-level type references (supertypes, return types, parameter types, property types,
 * type-parameter bounds, receiver types, type arguments, annotations, const val initializers, and
 * annotation-class constructor defaults).
 *
 * Imports are intentionally untracked: Kosabi stubs missing types by design, so unused-import
 * resolution never affects ABI bytes.
 */
class KosabiClassUsageCollector {

  private val uriUsages: MutableSet<URI> = mutableSetOf()
  private val visitedClasses = mutableSetOf<String>()
  private val missingClassIds = mutableSetOf<ClassId>()
  private val visitedShallow = mutableSetOf<String>()
  private val visitedHierarchy = mutableSetOf<String>()
  private var shallowClassUsage = false

  /**
   * Walk the resolved FIR tree and collect all referenced classpath classes. Writes results to the
   * dep-tracker output path if available in the configuration.
   */
  fun collectAndDump(analysisResults: FirResultCompat, configuration: CompilerConfiguration) {
    val outputPath = getDepTrackerOutputPath(configuration) ?: return
    shallowClassUsage = configuration.get(K2JvmAbiConfigurationKeys.SHALLOW_CLASS_USAGE) == true

    for (output in analysisResults.outputs) {
      val session = output.session
      for (firFile in output.fir) {
        firFile.accept(ClassUsageVisitor(session))
      }
    }

    addMissingTypeGuards(configuration.jvmClasspathRoots)
    dump(outputPath)
  }

  @OptIn(SymbolInternals::class)
  private fun recordType(type: ConeKotlinType, session: FirSession, hierarchy: Boolean = false) {
    if (type is ConeErrorType) {
      recordMissingClassIdFromError(type)
      return
    }

    val upper = type.upperBoundIfFlexible()
    if (upper is ConeErrorType) {
      recordMissingClassIdFromError(upper)
      return
    }
    (upper as? ConeClassLikeType)?.typeArguments?.forEach {
      if (it is ConeKotlinTypeProjection) recordType(it.type, session)
    }

    val classId = upper.classId ?: return

    // Skip builtins and local classes
    if (classId.isLocal) return
    val pkg = classId.packageFqName.asString()
    if (isStdlibOrJdkPackage(pkg)) return

    val symbol = session.symbolProvider.getClassLikeSymbolByClassId(classId)
    if (symbol == null) {
      missingClassIds.add(classId)
      return
    }
    recordClassDeclaration(symbol.fir, session, hierarchy)
  }

  @OptIn(SymbolInternals::class)
  private fun recordMissingClassIdFromError(type: ConeErrorType) {
    try {
      val classId = type.lookupTag.classId
      if (classId.isLocal) return
      if (isStdlibOrJdkPackage(classId.packageFqName.asString())) return
      missingClassIds.add(classId)
    } catch (_: Exception) {
      // ClassId not recoverable from the error type; nothing recordable.
    }
  }

  /**
   * Records a missing-type guard entry for every missing type under every jar on the classpath.
   * Dep-file checking only compares listed entries, so a type that appears in a jar after the build
   * would otherwise go unnoticed and reuse a stale ABI shaped by its absence.
   */
  private fun addMissingTypeGuards(classpathRoots: List<File>) {
    if (missingClassIds.isEmpty()) return
    // Error-sentinel ids like <error> name no class file.
    val recordableIds = missingClassIds.filterNot {
      it.relativeClassName.pathSegments().any { name -> name.isSpecial }
    }
    if (recordableIds.isEmpty()) return
    val jarPrefixes =
        (uriUsages.map { it.rawSchemeSpecificPart.substringBefore(JAR_FILE_SEPARATOR) } +
                classpathRoots
                    .filter { it.isFile && it.extension == "jar" && !isStubJarPath(it.path) }
                    .map { "file://${it.absolutePath}" })
            .toSet()
    for (prefix in jarPrefixes) {
      for (classId in recordableIds) {
        try {
          uriUsages.add(URI("jar:$prefix$JAR_FILE_SEPARATOR${classFilePath(classId)}"))
        } catch (_: Exception) {
          // Unusable as a dep entry; leave the type unrecorded.
        }
      }
    }
  }

  private fun isStubJarPath(jarPath: String): Boolean {
    val name = jarPath.substringAfterLast('/')
    return name.endsWith(STUBSGEN_STUBS_JAR) || name == "stubs.jar"
  }

  private fun classFilePath(classId: ClassId): String {
    val pkg = classId.packageFqName.asString()
    val names = classId.relativeClassName.pathSegments().joinToString("$") { it.asString() }
    return (if (pkg.isEmpty()) "" else pkg.replace('.', '/') + "/") + names + ".class"
  }

  private fun recordAnnotation(annotation: FirAnnotation, session: FirSession) {
    recordType(annotation.annotationTypeRef.coneType, session)
    if (annotation is FirAnnotationCall) {
      annotation.argumentList.arguments.forEach { it.accept(ExpressionUsageVisitor(session)) }
    }
  }

  private fun recordTypeRef(typeRef: FirTypeRef, session: FirSession, hierarchy: Boolean = false) {
    if (shallowClassUsage && hierarchy) {
      typeRef.annotations.forEach { recordAnnotation(it, session) }
      recordType(typeRef.coneType, session, hierarchy = true)
      return
    }
    typeRef.annotations.forEach { recordAnnotation(it, session) }
    recordType(typeRef.coneType, session, hierarchy)
  }

  private fun recordValueParameter(
      valueParameter: FirValueParameter,
      session: FirSession,
      recordDefault: Boolean = false,
  ) {
    recordTypeRef(valueParameter.returnTypeRef, session)
    valueParameter.annotations.forEach { recordAnnotation(it, session) }
    if (recordDefault) {
      valueParameter.defaultValue?.accept(ExpressionUsageVisitor(session))
    }
  }

  @OptIn(SymbolInternals::class)
  private fun recordTypeParameters(
      typeParameters: List<FirTypeParameterRef>,
      session: FirSession,
  ) {
    typeParameters.forEach { ref ->
      val typeParameter = (ref as? FirTypeParameter) ?: ref.symbol.fir
      typeParameter.bounds.forEach { recordTypeRef(it, session) }
      typeParameter.annotations.forEach { recordAnnotation(it, session) }
    }
  }

  private fun recordReceiverAndContext(
      receiverParameter: FirReceiverParameter?,
      contextParameters: List<FirValueParameter>,
      session: FirSession,
      skipReceiver: Boolean = false,
  ) {
    if (!skipReceiver) {
      receiverParameter?.typeRef?.let { recordTypeRef(it, session) }
    }
    contextParameters.forEach { recordValueParameter(it, session) }
  }

  // Private members are stripped from the ABI, so types appearing solely in their
  // return/receiver positions need no recording. Value-parameter types stay recorded: a
  // defaulted private fun's synthetic `$default` stub is kept with its real descriptor.
  private fun isStrippedFromAbi(member: FirCallableDeclaration): Boolean {
    val visibility = member.status.visibility
    return visibility == Visibilities.Private ||
        visibility == Visibilities.PrivateToThis ||
        visibility == Visibilities.Local
  }

  @OptIn(SymbolInternals::class)
  private fun recordClassDeclaration(
      firClass: FirClassLikeDeclaration,
      session: FirSession,
      hierarchy: Boolean = false,
  ) {
    when (firClass) {
      is FirJavaClass -> {
        when (firClass.origin) {
          is FirDeclarationOrigin.Java.Library ->
              recordJavaClass(firClass.toBinaryJavaClassCompat(), hierarchy)
          else -> firClass.sourceElement?.let(::recordSource)
        }
      }
      is FirRegularClass -> {
        if (firClass.origin is FirDeclarationOrigin.BuiltIns) return
        val className = firClass.symbol.classId.asSingleFqName().asString()
        if (!shallowClassUsage) {
          if (!visitedClasses.add(className)) return
          firClass.sourceElement?.let(::recordSource)
          firClass.superTypeRefs.forEach { recordType(it.coneType, session) }
          firClass.annotations
              .map { it.annotationTypeRef }
              .forEach { recordType(it.coneType, session) }
        } else if (hierarchy) {
          if (!visitedHierarchy.add(className)) return
          visitedShallow.add(className)
          firClass.sourceElement?.let(::recordSource)
          firClass.superTypeRefs.forEach { recordType(it.coneType, session, hierarchy = true) }
        } else {
          if (className in visitedHierarchy || !visitedShallow.add(className)) return
          firClass.sourceElement?.let(::recordSource)
        }
      }
      is FirAnonymousObject -> {
        firClass.superTypeRefs.forEach { recordType(it.coneType, session) }
      }
      is FirTypeAlias -> {
        recordType(firClass.expandedTypeRef.coneType, session)
      }
      else -> {}
    }
  }

  private fun recordJavaClass(clazz: JavaClass, hierarchy: Boolean = false) {
    val className = clazz.fqName?.asString() ?: return
    if (!shallowClassUsage) {
      if (!visitedClasses.add(className)) return
    } else if (hierarchy) {
      if (!visitedHierarchy.add(className)) return
      visitedShallow.add(className)
    } else {
      if (className in visitedHierarchy || !visitedShallow.add(className)) return
    }

    if (clazz !is VirtualFileBoundJavaClass) {
      error("Unsupported kind of a JavaClass: ${clazz::class}")
    }
    clazz.virtualFile?.path?.let(::addFile)

    if (!shallowClassUsage) {
      clazz.supertypes.mapNotNull { it.classifier }.forEach(::recordJavaClassifier)

      clazz.annotations.mapNotNull { it.resolve() }.forEach(::recordJavaClass)
    } else if (hierarchy) {
      clazz.supertypes
          .mapNotNull { it.classifier }
          .forEach {
            recordJavaClassifier(it, hierarchy = true)
          }
    }
  }

  private fun recordJavaClassifier(classifier: JavaClassifier, hierarchy: Boolean = false) {
    when (classifier) {
      is JavaTypeParameter -> return
      is JavaClass -> recordJavaClass(classifier, hierarchy)
      else -> error("Unsupported kind of a JavaClassifier: ${classifier::class}")
    }
  }

  private fun recordSource(source: org.jetbrains.kotlin.descriptors.SourceElement) {
    when (source) {
      is KotlinJvmBinarySourceElement -> addFile(source.binaryClass.location)
      is org.jetbrains.kotlin.load.kotlin.JvmPackagePartSource ->
          source.knownJvmBinaryClass?.location?.let(::addFile)
      is org.jetbrains.kotlin.load.java.sources.JavaSourceElement -> {
        val javaElement = source.javaElement
        if (
            javaElement is org.jetbrains.kotlin.load.java.structure.impl.VirtualFileBoundJavaClass
        ) {
          javaElement.virtualFile?.path?.let(::addFile)
        }
      }
    }
  }

  private fun addFile(path: String) {
    if (
        path.contains(JAR_FILE_SEPARATOR) &&
            !isStubJarPath(path.substringBefore(JAR_FILE_SEPARATOR))
    ) {
      uriUsages.add(URI("jar:file://$path"))
    }
  }

  private fun dump(path: String) {
    BufferedWriter(FileWriter(File(path), /* append= */ true)).use { writer ->
      for (uri in uriUsages) {
        writer.appendLine(uri.toString())
      }
    }
  }

  /**
   * Visitor that walks expression trees to find class references. Used for visiting const val
   * initializers and other expression contexts where types need to be tracked.
   */
  private inner class ExpressionUsageVisitor(private val session: FirSession) :
      FirDefaultVisitorVoid() {
    override fun visitElement(element: FirElement) {
      element.acceptChildren(this)
    }

    override fun visitResolvedQualifier(resolvedQualifier: FirResolvedQualifier) {
      val classId =
          resolvedQualifier.relativeClassFqName?.let {
            ClassId(resolvedQualifier.packageFqName, it, isLocal = false)
          }
      if (classId != null) {
        @OptIn(SymbolInternals::class)
        val symbol = session.symbolProvider.getClassLikeSymbolByClassId(classId)
        if (symbol != null) {
          @OptIn(SymbolInternals::class) recordClassDeclaration(symbol.fir, session)
        }
      }
      resolvedQualifier.acceptChildren(this)
    }

    override fun visitQualifiedAccessExpression(
        qualifiedAccessExpression: FirQualifiedAccessExpression,
    ) {
      // Record the type of the expression itself
      try {
        recordType(qualifiedAccessExpression.resolvedType, session)
      } catch (_: Exception) {
        // resolvedType may throw on unresolved expressions
      }
      // Record the dispatch receiver type (e.g., B.Companion in B.bConst)
      val calleeSymbol = qualifiedAccessExpression.calleeReference.symbol
      if (calleeSymbol is FirCallableSymbol<*>) {
        val dispatchClassId = calleeSymbol.callableId?.classId
        if (dispatchClassId != null) {
          @OptIn(SymbolInternals::class)
          val classSymbol = session.symbolProvider.getClassLikeSymbolByClassId(dispatchClassId)
          if (classSymbol != null) {
            @OptIn(SymbolInternals::class) recordClassDeclaration(classSymbol.fir, session)
          }
        }
      }
      qualifiedAccessExpression.acceptChildren(this)
    }

    override fun visitAnnotationCall(annotationCall: FirAnnotationCall) {
      recordAnnotation(annotationCall, session)
    }
  }

  private inner class ClassUsageVisitor(private val session: FirSession) :
      FirDefaultVisitorVoidCompat() {
    override fun visitElement(element: FirElement) {
      // Default: do not recurse into expressions/bodies
    }

    override fun visitFile(file: org.jetbrains.kotlin.fir.declarations.FirFile) {
      for (annotation in file.annotations) {
        recordAnnotation(annotation, session)
      }
      for (declaration in file.declarations) {
        declaration.accept(this)
      }
    }

    override fun visitRegularClass(regularClass: FirRegularClass) {
      for (superTypeRef in regularClass.superTypeRefs) {
        recordTypeRef(superTypeRef, session, hierarchy = true)
      }
      recordTypeParameters(regularClass.typeParameters, session)
      for (annotation in regularClass.annotations) {
        recordAnnotation(annotation, session)
      }
      for (declaration in regularClass.declarations) {
        declaration.accept(this)
      }
    }

    override fun visitNamedFunctionCompat(simpleFunction: FirNamedFunctionCompat) {
      val stripped = isStrippedFromAbi(simpleFunction)
      if (!stripped) {
        recordTypeRef(simpleFunction.returnTypeRef, session)
      }
      recordTypeParameters(simpleFunction.typeParameters, session)
      recordReceiverAndContext(
          simpleFunction.receiverParameter,
          simpleFunction.contextParameters,
          session,
          skipReceiver = stripped,
      )
      for (valueParameter in simpleFunction.valueParameters) {
        recordValueParameter(valueParameter, session)
      }
      for (annotation in simpleFunction.annotations) {
        recordAnnotation(annotation, session)
      }
    }

    override fun visitProperty(property: org.jetbrains.kotlin.fir.declarations.FirProperty) {
      val stripped = isStrippedFromAbi(property)
      if (!stripped) {
        recordTypeRef(property.returnTypeRef, session)
      }
      recordReceiverAndContext(
          property.receiverParameter,
          property.contextParameters,
          session,
          skipReceiver = stripped,
      )
      for (annotation in property.annotations) {
        recordAnnotation(annotation, session)
      }
      property.getter?.annotations.orEmpty().forEach { recordAnnotation(it, session) }
      property.setter?.annotations.orEmpty().forEach { recordAnnotation(it, session) }
      property.backingField?.annotations.orEmpty().forEach { recordAnnotation(it, session) }
      // Walk const val initializers — these are part of the ABI (inlined constants)
      // and may reference classes not visible from declaration-level types alone.
      // Example: const val x: Int = B.bConst  references B.Companion
      if (property.isConst) {
        property.initializer?.accept(ExpressionUsageVisitor(session))
      }
    }

    override fun visitConstructor(
        constructor: org.jetbrains.kotlin.fir.declarations.FirConstructor,
    ) {
      // Only annotation-class constructor defaults reach the ABI (as AnnotationDefault);
      // every other default is regenerated during stripping and needs no recording.
      val recordDefaults = isAnnotationClassConstructor(constructor, session)
      for (valueParameter in constructor.valueParameters) {
        recordValueParameter(valueParameter, session, recordDefault = recordDefaults)
      }
      for (annotation in constructor.annotations) {
        recordAnnotation(annotation, session)
      }
    }

    @OptIn(SymbolInternals::class)
    private fun isAnnotationClassConstructor(
        constructor: org.jetbrains.kotlin.fir.declarations.FirConstructor,
        session: FirSession,
    ): Boolean {
      val classId = constructor.symbol.callableId?.classId ?: return false
      val firClass = session.symbolProvider.getClassLikeSymbolByClassId(classId)?.fir
      return (firClass as? FirRegularClass)?.classKind == ClassKind.ANNOTATION_CLASS
    }

    override fun visitEnumEntry(enumEntry: FirEnumEntry) {
      recordTypeRef(enumEntry.returnTypeRef, session)
      for (annotation in enumEntry.annotations) {
        recordAnnotation(annotation, session)
      }
    }

    override fun visitTypeAlias(typeAlias: FirTypeAlias) {
      recordTypeRef(typeAlias.expandedTypeRef, session)
      recordTypeParameters(typeAlias.typeParameters, session)
      for (annotation in typeAlias.annotations) {
        recordAnnotation(annotation, session)
      }
    }
  }

  companion object {
    /**
     * Gets the dep-tracker output path from the compiler configuration. Uses reflection to access
     * DependencyTrackerCommandLineProcessor.ARG_OUTPUT_PATH since the k2-jvm-abi-gen module does
     * not have a direct dependency on the dep-tracker module.
     */
    @Suppress("SwallowedException")
    private fun getDepTrackerOutputPath(configuration: CompilerConfiguration): String? {
      return try {
        val clpClass =
            Class.forName(
                "com.facebook.kotlin.compilerplugins.usedclasses.cli.DependencyTrackerCommandLineProcessor",
            )
        val companionField = clpClass.getDeclaredField("Companion")
        val companion = companionField.get(null)
        val method = companion.javaClass.getDeclaredMethod("getARG_OUTPUT_PATH")
        @Suppress("UNCHECKED_CAST")
        val key = method.invoke(companion) as CompilerConfigurationKey<String>
        configuration.get(key)
      } catch (_: Exception) {
        null
      }
    }
  }
}
