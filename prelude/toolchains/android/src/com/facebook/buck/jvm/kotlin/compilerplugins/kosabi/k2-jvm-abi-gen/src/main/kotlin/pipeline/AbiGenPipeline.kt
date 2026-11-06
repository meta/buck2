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
    com.facebook.DirectDeclarationsAccessCompat::class,
)

package com.facebook

import com.facebook.kotlin.compilercompat.callableNameOrNullCompat
import com.facebook.kotlin.compilercompat.packageNameOrNullCompat
import java.io.File
import java.util.jar.JarFile
import org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension
import org.jetbrains.kotlin.cli.common.messages.CompilerMessageSeverity
import org.jetbrains.kotlin.cli.common.messages.MessageCollector
import org.jetbrains.kotlin.descriptors.ClassKind
import org.jetbrains.kotlin.descriptors.Visibilities
import org.jetbrains.kotlin.fir.FirElement
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.backend.FirMetadataSource
import org.jetbrains.kotlin.fir.declarations.FirCallableDeclaration
import org.jetbrains.kotlin.fir.declarations.FirDeclaration
import org.jetbrains.kotlin.fir.declarations.FirDeclarationOrigin
import org.jetbrains.kotlin.fir.declarations.FirFile
import org.jetbrains.kotlin.fir.declarations.FirFunction
import org.jetbrains.kotlin.fir.declarations.FirProperty
import org.jetbrains.kotlin.fir.declarations.FirRegularClass
import org.jetbrains.kotlin.fir.declarations.utils.isConst
import org.jetbrains.kotlin.fir.expressions.FirAnnotation
import org.jetbrains.kotlin.fir.expressions.FirAnnotationCall
import org.jetbrains.kotlin.fir.expressions.FirErrorExpression
import org.jetbrains.kotlin.fir.expressions.FirGetClassCall
import org.jetbrains.kotlin.fir.expressions.FirNamedArgumentExpression
import org.jetbrains.kotlin.fir.expressions.FirQualifiedAccessExpression
import org.jetbrains.kotlin.fir.expressions.FirVarargArgumentsExpression
import org.jetbrains.kotlin.fir.expressions.FirWrappedArgumentExpression
import org.jetbrains.kotlin.fir.expressions.impl.FirResolvedArgumentList
import org.jetbrains.kotlin.fir.moduleData
import org.jetbrains.kotlin.fir.references.FirErrorNamedReference
import org.jetbrains.kotlin.fir.references.FirResolvedNamedReference
import org.jetbrains.kotlin.fir.resolve.providers.symbolProvider
import org.jetbrains.kotlin.fir.serialization.providedDeclarationsForMetadataService
import org.jetbrains.kotlin.fir.symbols.SymbolInternals
import org.jetbrains.kotlin.fir.symbols.impl.FirCallableSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirClassSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirNamedFunctionSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirPropertySymbol
import org.jetbrains.kotlin.fir.types.ConeErrorType
import org.jetbrains.kotlin.fir.types.ConeKotlinType
import org.jetbrains.kotlin.fir.types.ConeTypeProjection
import org.jetbrains.kotlin.fir.types.coneType
import org.jetbrains.kotlin.fir.types.constructType
import org.jetbrains.kotlin.fir.types.resolvedType
import org.jetbrains.kotlin.fir.types.type
import org.jetbrains.kotlin.fir.visitors.FirDefaultVisitorVoid
import org.jetbrains.kotlin.ir.IrElement
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrConstructor
import org.jetbrains.kotlin.ir.declarations.IrDeclarationBase
import org.jetbrains.kotlin.ir.declarations.IrFile
import org.jetbrains.kotlin.ir.declarations.IrMetadataSourceOwner
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.declarations.IrProperty
import org.jetbrains.kotlin.ir.declarations.IrSimpleFunction
import org.jetbrains.kotlin.ir.util.kotlinFqName
import org.jetbrains.kotlin.name.CallableId
import org.jetbrains.kotlin.name.ClassId
import org.jetbrains.kotlin.name.FqName
import org.jetbrains.kotlin.psi.KtFile
import org.jetbrains.org.objectweb.asm.AnnotationVisitor
import org.jetbrains.org.objectweb.asm.ClassReader
import org.jetbrains.org.objectweb.asm.ClassVisitor
import org.jetbrains.org.objectweb.asm.ConstantDynamic
import org.jetbrains.org.objectweb.asm.FieldVisitor
import org.jetbrains.org.objectweb.asm.Handle
import org.jetbrains.org.objectweb.asm.Label
import org.jetbrains.org.objectweb.asm.MethodVisitor
import org.jetbrains.org.objectweb.asm.ModuleVisitor
import org.jetbrains.org.objectweb.asm.Opcodes
import org.jetbrains.org.objectweb.asm.RecordComponentVisitor
import org.jetbrains.org.objectweb.asm.Type
import org.jetbrains.org.objectweb.asm.TypePath
import org.jetbrains.org.objectweb.asm.signature.SignatureReader
import org.jetbrains.org.objectweb.asm.signature.SignatureVisitor

fun errorTypedApiPositions(
    isProperty: Boolean,
    returnTypeHasError: Boolean,
    propertyTypeHasError: Boolean,
    receiverHasError: Boolean,
    contextParameterErrors: List<Boolean>,
    valueParameterErrors: List<Boolean>,
): List<String> = buildList {
  if (isProperty) {
    if (propertyTypeHasError) add("property type")
  } else if (returnTypeHasError) {
    add("return type")
  }
  if (receiverHasError) add("receiver")
  contextParameterErrors.forEachIndexed { index, hasError ->
    if (hasError) add("context parameter $index")
  }
  valueParameterErrors.forEachIndexed { index, hasError ->
    if (hasError) add("parameter $index")
  }
}

/**
 * FIR metadata sanitization stage.
 *
 * Handles both pre-IR FIR tree cleanup and post-IR FIR metadata source cleanup:
 * - Strip annotations with errors and fix property initializers (FIR tree, pre-IR, single pass)
 * - Strip @Throws from FIR metadata sources (post-IR)
 * - Strip annotations with errors from FIR metadata sources (post-IR)
 * - Strip private supertypes + fake override conversion (post-IR)
 */
internal class FirMetadataSanitizerStage(private val repairLog: AbiGenRepairLog) : AbiGenStage {
  override val name = "FirMetadataSanitizer"

  /**
   * Pre-IR: cleanup the FIR tree before FIR-to-IR conversion in a single pass.
   *
   * Combines two operations that both walk the FIR tree:
   * 1. Strip ALL annotations that have error expressions in their arguments.
   * 2. Fix property initializers containing error expressions (clear them).
   */
  fun cleanupFirTree(firResult: FirResultCompat) {
    for (output in firResult.outputs) {
      for (firFile in output.fir) {
        firFile.accept(FirSanitizingVisitor())
      }
    }
  }

  /**
   * Post-IR: cleanup FIR metadata sources attached to IR declarations in a single IR tree walk.
   *
   * Combines three operations that all walk the IR module visiting classes, functions, properties,
   * and constructors to access FirMetadataSource:
   * 1. Strip @Throws annotations with error types from FIR metadata sources.
   * 2. Strip ALL annotations with error expressions from FIR metadata sources.
   * 3. Strip PRIVATE supertypes from FIR metadata sources.
   */
  fun cleanupFirMetadataSources(moduleFragment: IrModuleFragment, session: FirSession) {
    val THROWS_FQ_NAME = FqName("kotlin.jvm.Throws")
    val THROWS_KOTLIN_FQ_NAME = FqName("kotlin.Throws")

    moduleFragment.accept(
        object : IrElementVisitorVoidCompat() {
          override fun visitElement(element: IrElement) {
            element.acceptChildren(this, null)
          }

          override fun visitFile(declaration: IrFile) {
            degradeErrorTypedFileMemberTypes(declaration)
            super.visitFile(declaration)
          }

          override fun visitClass(declaration: IrClass) {
            stripThrowsAndErrorAnnotationsFromDeclaration(declaration)
            stripPrivateSupertypesFromDeclaration(declaration)
            stripErrorTypedPrivateMembersFromDeclaration(declaration)
            super.visitClass(declaration)
          }

          override fun visitSimpleFunction(declaration: IrSimpleFunction) {
            stripThrowsAndErrorAnnotationsFromDeclaration(declaration)
            degradeErrorTypedMetadataSource(declaration, session)
            super.visitSimpleFunction(declaration)
          }

          override fun visitProperty(declaration: IrProperty) {
            stripThrowsAndErrorAnnotationsFromDeclaration(declaration)
            degradeErrorTypedMetadataSource(declaration, session)
            super.visitProperty(declaration)
          }

          override fun visitConstructor(declaration: IrConstructor) {
            stripThrowsAndErrorAnnotationsFromDeclaration(declaration)
            degradeErrorTypedMetadataSource(declaration, session)
            super.visitConstructor(declaration)
          }

          private fun stripThrowsAndErrorAnnotationsFromDeclaration(
              declaration: IrDeclarationBase,
          ) {
            val metadataSourceOwner = declaration as? IrMetadataSourceOwner ?: return
            val metadataSource = metadataSourceOwner.metadata ?: return
            val firMetadataSource = metadataSource as? FirMetadataSource ?: return

            stripThrowsFromFirDeclaration(firMetadataSource.fir)
            stripAnnotationsWithErrorsFromFirDeclaration(firMetadataSource.fir)
          }

          // Degrade error-typed NON-API (private/local) members of a class to `Any?` on the FIR
          // metadata source. In source-only ABI, a private member with an inferred type that
          // depends on an unresolvable dependency symbol (e.g.
          // `private val x = Dep.getInstance()` or `private val y = setOf(Dep.CONST)`) gets an
          // error return type, which FirElementSerializer.propertyProto cannot serialize into
          // @Metadata ("Cannot serialize error type").
          //
          // The metadata serializer iterates the class member SYMBOLS
          // (firClass.symbol.declarationSymbols), which can still include members that earlier IR
          // stripping removed from firClass.declarations, so the return type has to be replaced on
          // the symbols' FIR — not just on the declarations list.
          //
          // API members are deliberately left alone: they are part of the ABI, so degrading their
          // type would produce an ABI that lies to consumers (metadata says `Any?` while the
          // bytecode descriptor still says `error/NonExistentClass`), turning a loud build failure
          // into a silent one. Those targets must fix the missing dependency / add an explicit
          // type instead.
          @OptIn(SymbolInternals::class)
          private fun stripErrorTypedPrivateMembersFromDeclaration(declaration: IrClass) {
            val metadataSourceOwner = declaration as? IrMetadataSourceOwner ?: return
            val firMetadataSource = metadataSourceOwner.metadata as? FirMetadataSource ?: return
            val firClass = firMetadataSource.fir as? FirRegularClass ?: return

            // A member of a private/local class is not ABI surface even when it is declared
            // public: no consumer can reference it, so degrading its unserializable type cannot
            // make the ABI lie to anyone. This covers the common
            // `private object Utils { fun get() = Dep.somethingUnresolvable() }` shape, where the
            // member's own visibility is public but the enclosing object is private.
            val classIsNonApi = isClassEffectivelyPrivate(declaration)

            firClass.symbol.declarationSymbols.forEach { symbol ->
              val decl = (symbol as? FirCallableSymbol<*>)?.fir ?: return@forEach
              if (!classIsNonApi && !isNonApiVisibility(decl)) {
                recordErrorTypedApiMember(firClass.symbol.classId.asString(), decl)
                return@forEach
              }
              // The metadata serializer collects functions/constructors from the class member
              // SCOPE, which references these same symbols' FIR. An error type in any position the
              // serializer reads — the return type, a value parameter (e.g. a parameter typed by a
              // nested enum of a stubbed dependency such as `CdsNavigationBar.Action`), or a
              // property's accessors/backing field — crashes FirElementSerializer the same way, so
              // degrade all of them to `Any?` here. The IR-visitor pass only reaches these via
              // firClass.declarations, which can be missing members served only from the scope.
              degradeErrorTypedPositions(decl, session)
            }

            // Belt-and-suspenders: also cover anything present only in firClass.declarations.
            // Mirror the symbol loop's asymmetry - degrade non-API members, record API ones - so an
            // error-typed API member reachable only via this path is not silently missed by
            // Assertion 5. recordErrorTypedApiMember de-duplicates, so members served from both
            // paths are recorded once.
            firClass.declarations.forEach { decl ->
              if (decl !is FirCallableDeclaration) return@forEach
              if (!classIsNonApi && !isNonApiVisibility(decl)) {
                recordErrorTypedApiMember(firClass.symbol.classId.asString(), decl)
                return@forEach
              }
              degradeErrorTypedPositions(decl, session)
            }
          }

          private fun degradeErrorTypedMetadataSource(
              declaration: IrDeclarationBase,
              session: FirSession,
          ) {
            val metadataSourceOwner = declaration as? IrMetadataSourceOwner ?: return
            val firMetadataSource = metadataSourceOwner.metadata as? FirMetadataSource ?: return
            val fir = firMetadataSource.fir as? FirCallableDeclaration ?: return
            if (!isNonApiVisibility(fir)) return
            degradeErrorTypedPositions(fir, session)
          }

          // The file facade's @Metadata is serialized by FirElementSerializer.packagePartProto,
          // which iterates firFile.declarations. A private top-level member is deleted from the IR
          // by the IR sanitizer before this pass runs, so visitSimpleFunction/visitProperty never
          // reach it, yet the serializer still reads it from the FIR file and crashes on its error
          // type (e.g. `private fun f() = Dep.unresolved()`).
          private fun degradeErrorTypedFileMemberTypes(declaration: IrFile) {
            val firFile = (declaration.metadata as? FirMetadataSource)?.fir as? FirFile ?: return
            firFile.declarations.forEach { decl ->
              if (decl !is FirCallableDeclaration) return@forEach
              if (isNonApiVisibility(decl)) {
                degradeErrorTypedPositions(decl, session)
              } else {
                recordErrorTypedApiMember(
                    // Null callableId (2.3+) degrades to "<unknown>"; a legitimate
                    // root package stays "". Version detail lives in the buckets
                    // via packageNameOrNullCompat().
                    decl.symbol.packageNameOrNullCompat() ?: "<unknown>",
                    decl,
                )
              }
            }
          }

          // Degrade every error-typed position the serializer reads for a callable, not just its
          // own return type. propertyProto serializes the setter's value parameter whenever the
          // accessors are non-default, which is the case for a delegated property
          // (`private var x by AtomicReference(...)` with an unresolved `getValue`), so degrading
          // only the property return type leaves the crash in place.
          private fun degradeErrorTypedPositions(
              decl: FirCallableDeclaration,
              session: FirSession,
          ) {
            decl.receiverParameter?.let { receiver ->
              if (
                  runCatching { receiver.typeRef.coneType.containsErrorType() }.getOrDefault(true)
              ) {
                runCatching { receiver.replaceTypeRef(session.builtinTypes.nullableAnyType) }
                    .onFailure { failure ->
                      repairLog.recordFailedRepair(
                          decl.symbol.callableId.toString(),
                          "could not degrade unresolved receiver: " +
                              "${failure.javaClass.simpleName}: ${failure.message}",
                      )
                    }
              }
            }
            decl.contextParameters.forEachIndexed { index, parameter ->
              if (hasErrorReturnType(parameter)) {
                runCatching {
                  parameter.replaceReturnTypeRef(session.builtinTypes.nullableAnyType)
                }
                    .onFailure { failure ->
                      repairLog.recordFailedRepair(
                          decl.symbol.callableId.toString(),
                          "could not degrade unresolved context parameter $index: " +
                              "${failure.javaClass.simpleName}: ${failure.message}",
                      )
                    }
              }
            }

            if (decl is FirProperty) {
              // The property declaration, getter return and setter value parameter describe the
              // same JVM property type and must be degraded together. A delegated property's
              // backing field has the delegate type instead, so handle it independently.
              val accessorTypePositions =
                  buildList<FirCallableDeclaration> {
                    add(decl)
                    decl.getter?.let { add(it) }
                    decl.setter?.let { addAll(it.valueParameters) }
                  }
              if (accessorTypePositions.any { hasErrorReturnType(it) }) {
                accessorTypePositions.forEach { replaceReturnTypeWithAny(it, session) }
              }
              decl.backingField?.let { degradeReturnTypeIfError(it, session) }
              return
            }
            // A function's return type and each of its value parameters are independent, so they
            // are degraded one by one to keep the degradation as narrow as possible.
            degradeReturnTypeIfError(decl, session)
            if (decl is FirFunction) {
              decl.valueParameters.forEach { degradeReturnTypeIfError(it, session) }
            }
          }

          private fun degradeReturnTypeIfError(
              decl: FirCallableDeclaration,
              session: FirSession,
          ) {
            if (!hasErrorReturnType(decl)) return
            replaceReturnTypeWithAny(decl, session)
          }

          private fun replaceReturnTypeWithAny(
              decl: FirCallableDeclaration,
              session: FirSession,
          ) {
            runCatching { decl.replaceReturnTypeRef(session.builtinTypes.nullableAnyType) }
                .onFailure { failure ->
                  repairLog.recordFailedRepair(
                      decl.symbol.callableId.toString(),
                      "could not degrade unresolved type: " +
                          "${failure.javaClass.simpleName}: ${failure.message}",
                  )
                }
          }

          // --- @Throws stripping helpers ---

          private fun stripThrowsFromFirDeclaration(
              declaration: org.jetbrains.kotlin.fir.declarations.FirDeclaration?,
          ) {
            if (declaration == null) return

            try {
              val annotationsField = declaration.javaClass.getDeclaredField("annotations")
              annotationsField.isAccessible = true
              val annotationsWrapper = annotationsField.get(declaration) ?: return

              val listField = annotationsWrapper.javaClass.getDeclaredField("list")
              listField.isAccessible = true
              @Suppress("UNCHECKED_CAST")
              val annotations =
                  listField.get(annotationsWrapper) as? MutableList<FirAnnotation> ?: return

              val toRemove = annotations.filter { annotation ->
                hasErrorTypeInThrowsAnnotation(
                    annotation,
                    THROWS_FQ_NAME,
                    THROWS_KOTLIN_FQ_NAME,
                )
              }

              if (toRemove.isNotEmpty()) {
                annotations.removeAll(toRemove)
              }
            } catch (_: Exception) {
              // If reflection fails, skip this declaration
            }
          }

          private fun hasErrorTypeInThrowsAnnotation(
              annotation: FirAnnotation,
              throwsFqName: FqName,
              throwsKotlinFqName: FqName,
          ): Boolean {
            val annotationType = annotation.annotationTypeRef.coneType
            val fqName =
                (annotationType as? org.jetbrains.kotlin.fir.types.ConeClassLikeType)
                    ?.lookupTag
                    ?.classId
                    ?.asSingleFqName()

            if (fqName != throwsFqName && fqName != throwsKotlinFqName) {
              return false
            }

            val annotationCall = annotation as? FirAnnotationCall ?: return false
            val argumentList = annotationCall.argumentList
            if (argumentList is FirResolvedArgumentList) {
              for ((argument, _) in argumentList.mapping) {
                if (hasErrorTypeInFirClassReference(argument)) {
                  return true
                }
              }
            }
            return false
          }

          private fun hasErrorTypeInFirClassReference(element: FirElement): Boolean {
            return when (element) {
              is FirVarargArgumentsExpression ->
                  element.arguments.any { hasErrorTypeInFirClassReference(it) }
              is FirGetClassCall -> {
                try {
                  val argument = element.argument
                  if (argument is FirQualifiedAccessExpression) {
                    argument.resolvedType is ConeErrorType
                  } else {
                    element.resolvedType is ConeErrorType
                  }
                } catch (_: Exception) {
                  false
                }
              }
              is FirQualifiedAccessExpression -> {
                try {
                  element.resolvedType is ConeErrorType ||
                      element.calleeReference is FirErrorNamedReference
                } catch (_: Exception) {
                  false
                }
              }
              is FirErrorExpression -> true
              else -> false
            }
          }

          // --- Annotation error stripping helpers ---

          private fun stripAnnotationsWithErrorsFromFirDeclaration(
              declaration: org.jetbrains.kotlin.fir.declarations.FirDeclaration?,
          ) {
            if (declaration == null) return

            try {
              val annotationsField = declaration.javaClass.getDeclaredField("annotations")
              annotationsField.isAccessible = true
              val annotationsWrapper = annotationsField.get(declaration) ?: return

              val listField = annotationsWrapper.javaClass.getDeclaredField("list")
              listField.isAccessible = true
              @Suppress("UNCHECKED_CAST")
              val annotations =
                  listField.get(annotationsWrapper) as? MutableList<FirAnnotation> ?: return

              val toRemove = annotations.filter { annotation ->
                hasErrorExpressionInFirAnnotation(annotation)
              }

              if (toRemove.isNotEmpty()) {
                annotations.removeAll(toRemove)
              }
            } catch (_: Exception) {
              // If reflection fails, skip this declaration
            }
          }

          private fun hasErrorExpressionInFirAnnotation(annotation: FirAnnotation): Boolean {
            val annotationCall = annotation as? FirAnnotationCall ?: return false
            val argumentList = annotationCall.argumentList
            if (argumentList is FirResolvedArgumentList) {
              for ((argument, _) in argumentList.mapping) {
                if (hasErrorExpressionInFirElement(argument)) {
                  return true
                }
              }
            }
            return false
          }

          private fun hasErrorExpressionInFirElement(element: FirElement): Boolean {
            return when (element) {
              is FirErrorExpression -> true
              is FirNamedArgumentExpression -> hasErrorExpressionInFirElement(element.expression)
              is FirWrappedArgumentExpression -> hasErrorExpressionInFirElement(element.expression)
              is FirVarargArgumentsExpression ->
                  element.arguments.any { hasErrorExpressionInFirElement(it) }
              is FirQualifiedAccessExpression -> {
                try {
                  element.resolvedType is ConeErrorType ||
                      element.calleeReference is FirErrorNamedReference
                } catch (_: Exception) {
                  false
                }
              }
              is FirGetClassCall -> {
                try {
                  val argument = element.argument
                  if (argument is FirQualifiedAccessExpression) {
                    argument.resolvedType is ConeErrorType
                  } else {
                    element.resolvedType is ConeErrorType
                  }
                } catch (_: Exception) {
                  false
                }
              }
              is org.jetbrains.kotlin.fir.expressions.FirFunctionCall -> {
                try {
                  element.resolvedType is ConeErrorType ||
                      element.calleeReference is FirErrorNamedReference
                } catch (_: Exception) {
                  false
                }
              }
              else -> false
            }
          }

          // --- Private supertype stripping helpers ---

          @OptIn(SymbolInternals::class)
          private fun stripPrivateSupertypesFromDeclaration(declaration: IrClass) {
            val metadataSourceOwner = declaration as? IrMetadataSourceOwner ?: return
            val metadataSource = metadataSourceOwner.metadata ?: return
            val firMetadataSource = metadataSource as? FirMetadataSource ?: return
            val firClass = firMetadataSource.fir as? FirRegularClass ?: return

            val strippedSupertypeClassIds = mutableSetOf<ClassId>()

            try {
              val superTypeRefsField = firClass.javaClass.getDeclaredField("superTypeRefs")
              superTypeRefsField.isAccessible = true
              val superTypeRefsValue = superTypeRefsField.get(firClass) ?: return

              @Suppress("UNCHECKED_CAST")
              val superTypeRefs: MutableList<org.jetbrains.kotlin.fir.types.FirTypeRef> =
                  when (superTypeRefsValue) {
                    is MutableList<*> ->
                        superTypeRefsValue as MutableList<org.jetbrains.kotlin.fir.types.FirTypeRef>
                    else -> {
                      val listField =
                          superTypeRefsValue.javaClass.declaredFields.find { it.name == "list" }
                              ?: return
                      listField.isAccessible = true
                      listField.get(superTypeRefsValue)
                          as? MutableList<org.jetbrains.kotlin.fir.types.FirTypeRef> ?: return
                    }
                  }

              val toRemove = superTypeRefs.filter { typeRef ->
                isPrivateSupertype(typeRef, firClass)
              }

              for (typeRef in toRemove) {
                val classId = getPrivateClassIdFromTypeRef(typeRef)
                if (classId != null) {
                  strippedSupertypeClassIds.add(classId)
                }
              }

              if (toRemove.isNotEmpty()) {
                superTypeRefs.removeAll(toRemove)

                for (classId in strippedSupertypeClassIds) {
                  repairLog.recordStrippedSupertype(
                      firClass.symbol.classId.asString(),
                      "private supertype ${classId.asString()} removed from ABI",
                  )
                }

                convertFirFakeOverridesFromStrippedPrivateSupertypes(
                    firClass,
                    strippedSupertypeClassIds,
                )
              }
            } catch (e: Exception) {
              repairLog.recordFailedRepair(
                  firClass.symbol.classId.asString(),
                  "could not strip private supertypes: ${e.javaClass.simpleName}: ${e.message}",
              )
            }
          }

          private fun getPrivateClassIdFromTypeRef(
              typeRef: org.jetbrains.kotlin.fir.types.FirTypeRef,
          ): ClassId? {
            val coneType =
                (typeRef as? org.jetbrains.kotlin.fir.types.FirResolvedTypeRef)?.coneType
                    ?: return null
            return (coneType as? org.jetbrains.kotlin.fir.types.ConeClassLikeType)
                ?.lookupTag
                ?.classId
          }

          @OptIn(SymbolInternals::class)
          private fun convertFirFakeOverridesFromStrippedPrivateSupertypes(
              firClass: FirRegularClass,
              strippedSupertypeClassIds: Set<ClassId>,
          ) {
            if (strippedSupertypeClassIds.isEmpty()) return

            val interfaceMethods = collectMethodsFromPrivateInterfaces(
                firClass.moduleData.session,
                strippedSupertypeClassIds,
            )

            if (interfaceMethods.isEmpty()) return

            val existingMethodNames =
                firClass.declarations
                    .filterIsInstance<FirNamedFunctionCompat>()
                    .map { it.name.asString() }
                    .toSet()

            for (interfaceMethod in interfaceMethods) {
              val methodName = interfaceMethod.name.asString()
              if (methodName in existingMethodNames) continue

              val copiedMethod = copyPrivateInterfaceMethodToClass(interfaceMethod, firClass)
              if (copiedMethod != null) {
                (firClass.declarations as MutableList<FirDeclaration>).add(copiedMethod)
                try {
                  firClass.moduleData.session.providedDeclarationsForMetadataService
                      .registerDeclarationCompat(copiedMethod, firClass)
                } catch (e: Exception) {
                  repairLog.recordFailedRepair(
                      "${firClass.symbol.classId.asString()}.$methodName",
                      "could not register materialized interface method: " +
                          "${e.javaClass.simpleName}: ${e.message}",
                  )
                }
              } else {
                repairLog.recordFailedRepair(
                    "${firClass.symbol.classId.asString()}.$methodName",
                    "could not copy method from stripped private interface",
                )
              }
            }
          }

          @OptIn(SymbolInternals::class)
          private fun copyPrivateInterfaceMethodToClass(
              interfaceMethod: FirNamedFunctionCompat,
              targetClass: FirRegularClass,
          ): FirNamedFunctionCompat? {
            return try {
              val targetClassId = targetClass.symbol.classId
              val newCallableId = CallableId(
                  targetClassId.packageFqName,
                  targetClassId.relativeClassName,
                  interfaceMethod.name,
              )
              buildNamedFunctionCopyCompat(interfaceMethod) {
                origin = FirDeclarationOrigin.Source
                symbol = FirNamedFunctionSymbol(newCallableId)
                dispatchReceiverType =
                    targetClass.symbol.constructType(
                        ConeTypeProjection.EMPTY_ARRAY,
                        isMarkedNullable = false,
                    )
              }
            } catch (e: Exception) {
              null
            }
          }

          @OptIn(SymbolInternals::class)
          private fun collectMethodsFromPrivateInterfaces(
              session: FirSession,
              interfaceClassIds: Set<ClassId>,
          ): List<FirNamedFunctionCompat> {
            val methods = mutableListOf<FirNamedFunctionCompat>()
            for (classId in interfaceClassIds) {
              val classSymbol =
                  session.symbolProvider.getClassLikeSymbolByClassId(classId) as? FirClassSymbol<*>
                      ?: continue
              val firClass = classSymbol.fir as? FirRegularClass ?: continue
              if (firClass.classKind != ClassKind.INTERFACE) continue

              for (decl in firClass.declarations) {
                if (decl is FirNamedFunctionCompat) {
                  val visibility = decl.status.visibility
                  if (visibility == Visibilities.Public || visibility == Visibilities.Protected) {
                    methods.add(decl)
                  }
                }
              }
            }
            return methods
          }

          private fun isPrivateSupertype(
              typeRef: org.jetbrains.kotlin.fir.types.FirTypeRef,
              firClass: FirRegularClass,
          ): Boolean {
            try {
              val coneType =
                  (typeRef as? org.jetbrains.kotlin.fir.types.FirResolvedTypeRef)?.coneType
                      ?: return false
              val classId =
                  (coneType as? org.jetbrains.kotlin.fir.types.ConeClassLikeType)
                      ?.lookupTag
                      ?.classId ?: return false

              val session = firClass.moduleData.session
              val classSymbol =
                  session.symbolProvider.getClassLikeSymbolByClassId(classId) as? FirClassSymbol<*>
                      ?: return false

              if (classSymbol.resolvedStatus.visibility == Visibilities.Local) {
                return true
              }

              // Only a top-level private class is dropped from the ABI; nested private classes
              // are kept so their InnerClasses references resolve. So what matters is whether the
              // outermost enclosing class is private, not whether any enclosing class is. Walk the
              // classId chain by name -- which needs no symbol resolution -- and resolve only the
              // outermost class. Resolving each intermediate enclosing symbol would let one that
              // fails to resolve mask a top-level-private outermost class and leave the supertype
              // dangling.
              var outermostClassId = classId
              while (outermostClassId.outerClassId != null) {
                outermostClassId = outermostClassId.outerClassId!!
              }

              val outermost =
                  if (outermostClassId == classId) {
                    classSymbol
                  } else {
                    session.symbolProvider.getClassLikeSymbolByClassId(outermostClassId)
                        as? FirClassSymbol<*> ?: return false
                  }

              return isClassPrivate(outermost)
            } catch (e: Exception) {
              return false
            }
          }

          private fun isClassEffectivelyPrivate(irClass: IrClass): Boolean {
            var current: IrClass? = irClass
            while (current != null) {
              if (
                  current.visibility ==
                      org.jetbrains.kotlin.descriptors.DescriptorVisibilities.PRIVATE ||
                      current.visibility ==
                          org.jetbrains.kotlin.descriptors.DescriptorVisibilities.LOCAL
              ) {
                return true
              }
              current = current.parent as? IrClass
            }
            return false
          }

          private fun isClassPrivate(classSymbol: FirClassSymbol<*>): Boolean {
            val visibility = classSymbol.resolvedStatus.visibility
            return visibility == Visibilities.Private || visibility == Visibilities.Local
          }
        },
        null,
    )
  }

  // --- Public utility methods ---

  // Recursively check if a FIR element contains error expressions.
  // Used by K2JvmAbiFirAnalysisHandlerExtension to check const val initializer resolvability.
  @OptIn(SymbolInternals::class)
  fun hasErrorExpressionRecursive(element: FirElement): Boolean {
    return when (element) {
      is FirErrorExpression -> true
      is FirQualifiedAccessExpression -> {
        try {
          if (
              element.resolvedType is ConeErrorType ||
                  element.calleeReference is FirErrorNamedReference
          ) {
            return true
          }

          val calleeReference = element.calleeReference
          if (calleeReference is FirResolvedNamedReference) {
            val symbol = calleeReference.resolvedSymbol
            if (symbol is FirPropertySymbol && symbol.isConst) {
              val initializer = symbol.fir.initializer
              if (initializer == null) {
                return true
              }
              if (isConstValInitializerUnresolvable(initializer)) {
                return true
              }
            }
          }
          false
        } catch (_: Exception) {
          false
        }
      }
      else -> {
        var hasError = false
        element.acceptChildren(
            object : FirDefaultVisitorVoid() {
              override fun visitElement(childElement: FirElement) {
                if (!hasError && hasErrorExpressionRecursive(childElement)) {
                  hasError = true
                }
              }
            },
        )
        hasError
      }
    }
  }

  private fun isConstValInitializerUnresolvable(initializer: FirElement): Boolean {
    return when (initializer) {
      is FirErrorExpression -> true
      is org.jetbrains.kotlin.fir.expressions.FirFunctionCall -> {
        try {
          val calleeReference = initializer.calleeReference
          if (calleeReference is FirResolvedNamedReference) {
            val name = calleeReference.name.asString()
            if (name == "TODO") {
              return true
            }
          }
          initializer.resolvedType is ConeErrorType
        } catch (_: Exception) {
          false
        }
      }
      is FirQualifiedAccessExpression -> {
        try {
          initializer.resolvedType is ConeErrorType ||
              initializer.calleeReference is FirErrorNamedReference
        } catch (_: Exception) {
          false
        }
      }
      else -> false
    }
  }

  // --- FIR tree sanitizing visitor for cleanupFirTree ---

  // An API member with an unresolved type reaches the descriptor as `error/NonExistentClass`,
  // which no consumer can link against. Degrading it would only move the lie from the descriptor
  // into the metadata, so it is recorded for ValidationStage to report instead.
  @OptIn(SymbolInternals::class)
  private fun recordErrorTypedApiMember(owner: String, decl: FirCallableDeclaration) {
    val member =
        runCatching {
          decl.symbol.callableNameOrNullCompat()?.asString()
        }
            .getOrNull() ?: "<unknown>"

    val isProperty = decl is FirProperty
    val propertyTypeHasError =
        if (decl is FirProperty) {
          // A property's declaration, getter return type and setter value parameter describe the
          // same JVM type. The backing field is deliberately excluded: for a delegated property
          // it has the delegate type and is stripped before consumer ABI emission.
          buildList<FirCallableDeclaration> {
                add(decl)
                decl.getter?.let { add(it) }
                decl.setter?.let { addAll(it.valueParameters) }
              }
              .any { hasErrorReturnType(it) }
        } else {
          false
        }
    val positions = errorTypedApiPositions(
        isProperty = isProperty,
        returnTypeHasError = !isProperty && hasErrorReturnType(decl),
        propertyTypeHasError = propertyTypeHasError,
        receiverHasError =
            decl.receiverParameter?.let { receiver ->
              runCatching { receiver.typeRef.coneType.containsErrorType() }.getOrDefault(true)
            } ?: false,
        contextParameterErrors = decl.contextParameters.map(::hasErrorReturnType),
        valueParameterErrors =
            if (decl is FirFunction) decl.valueParameters.map(::hasErrorReturnType)
            else emptyList(),
    )
    positions.forEach { where ->
      repairLog.recordErrorTypedApiMember(decl, owner, member, where)
    }
  }

  // A type ref left in an unresolved/inconsistent state after failed inference can throw on
  // coneType access; treat that as an error too.
  private fun hasErrorReturnType(decl: FirCallableDeclaration): Boolean = runCatching {
    decl.returnTypeRef.coneType.containsErrorType()
  }
      .getOrDefault(true)

  // Detect error types anywhere in a type, including nested type arguments. Inference failures
  // often leave the outer type resolved but a type argument as an error type
  // (e.g. `private val x = AtomicReference(Dep.UNRESOLVED)` -> `AtomicReference<ERROR>`), which
  // FirElementSerializer still cannot serialize.
  private fun ConeKotlinType.containsErrorType(): Boolean {
    if (this is ConeErrorType) return true
    return typeArguments.any { projection -> projection.type?.containsErrorType() == true }
  }

  // Only non-API members may have an unserializable error type degraded to `Any?`: they are
  // stripped from the ABI bytecode anyway, so metadata and bytecode stay consistent. Degrading an
  // API member would make the ABI lie to consumers.
  private fun isNonApiVisibility(decl: FirCallableDeclaration): Boolean {
    val visibility = decl.status.visibility
    return visibility == Visibilities.Private ||
        visibility == Visibilities.PrivateToThis ||
        visibility == Visibilities.Local
  }

  /**
   * Single-pass visitor that sanitizes FIR tree:
   * 1. Strips ALL annotations that have error expressions in their arguments.
   * 2. Clears property initializers containing error expressions.
   */
  private inner class FirSanitizingVisitor : FirDefaultVisitorVoid() {
    override fun visitElement(element: FirElement) {
      if (element is org.jetbrains.kotlin.fir.declarations.FirDeclaration) {
        stripAnnotationsWithErrors(element)
      }
      element.acceptChildren(this)
    }

    override fun visitProperty(property: org.jetbrains.kotlin.fir.declarations.FirProperty) {
      val initializer = property.initializer
      if (initializer != null && hasErrorExpressionRecursive(initializer)) {
        val owner = property.symbol.callableId.toString()
        try {
          val initializerField = property.javaClass.getDeclaredField("initializer")
          initializerField.isAccessible = true
          initializerField.set(property, null)
          // A const val that loses its initializer is emitted without a ConstantValue attribute,
          // so consumers that constant-fold it fail during their own compile rather than here.
          // That reasoning only applies to a constant a consumer can actually see, hence the
          // visibility, which [ValidationStage] filters on.
          repairLog.recordClearedPropertyInitializer(
              owner,
              if (property.isConst) "const val, no ConstantValue will be emitted"
              else "val initializer discarded",
              isConst = property.isConst,
              consumerVisible =
                  property.status.visibility != Visibilities.Private &&
                      property.status.visibility != Visibilities.PrivateToThis &&
                      property.status.visibility != Visibilities.Local,
          )
        } catch (e: Exception) {
          repairLog.recordFailedRepair(
              owner,
              "could not clear unresolvable initializer: ${e.javaClass.simpleName}: ${e.message}",
          )
        }
      }
      super.visitProperty(property)
    }

    override fun visitRegularClass(regularClass: FirRegularClass) {
      degradeErrorTypedNonApiMemberReturnTypes(regularClass)
      super.visitRegularClass(regularClass)
    }

    // Degrade non-API (private/local) members whose (inferred) type resolved to an error type to
    // `Any?`. Under source-only ABI, an inferred private member whose type flows through a stubbed
    // dependency member can get a ConeErrorType (e.g. `private val x = Dep.factory(...)`,
    // `private val y = AtomicReference(Dep.COMPANION_FIELD)`, or a SAM like
    // `private val z = Dep.Listener { ... }` where the nested type is absent from the stub). Such a
    // member is still serialized into JVM @Metadata (private members are kept when
    // produceHeaderKlib=false), and FirElementSerializer crashes on the error type ("Cannot
    // serialize error type ...").
    //
    // The metadata serializer collects non-static members from the class's UNSUBSTITUTED MEMBER
    // SCOPE (FirElementSerializer.memberDeclarations -> processAllProperties/processAllFunctions),
    // NOT from firClass.declarations. That scope is built during the frontend and references the
    // same FirProperty/FirFunction instances that back firClass.declarations. Merely removing a
    // member from firClass.declarations here does NOT remove it from the already-built scope, so
    // the serializer still sees it (with its error type) and crashes. Instead we DEGRADE the return
    // type on the shared FIR instance to `Any?`; because the scope references the same instance,
    // the
    // serializer then serializes `Any?` and succeeds. This is non-lossy: the member is
    // private/local
    // (not part of the ABI) and is stripped from IR/bytecode by the IR sanitizer and from @Metadata
    // by PrivateMetadataStripper.
    private fun degradeErrorTypedNonApiMemberReturnTypes(firClass: FirRegularClass) {
      val session = firClass.moduleData.session
      firClass.declarations.forEach { decl ->
        if (
            decl is FirCallableDeclaration && isNonApiVisibility(decl) && hasErrorReturnType(decl)
        ) {
          runCatching { decl.replaceReturnTypeRef(session.builtinTypes.nullableAnyType) }
              .onFailure { failure ->
                repairLog.recordFailedRepair(
                    decl.symbol.callableId.toString(),
                    "could not degrade unresolved pre-IR return type: " +
                        "${failure.javaClass.simpleName}: ${failure.message}",
                )
              }
        }
      }
    }

    private fun findFieldInHierarchy(
        clazz: Class<*>,
        fieldName: String,
    ): java.lang.reflect.Field? {
      var current: Class<*>? = clazz
      while (current != null) {
        val field = current.declaredFields.find { it.name == fieldName }
        if (field != null) return field
        current = current.superclass
      }
      return null
    }

    private fun stripAnnotationsWithErrors(
        declaration: org.jetbrains.kotlin.fir.declarations.FirDeclaration,
    ) {
      try {
        val annotationsField = findFieldInHierarchy(declaration.javaClass, "annotations") ?: return
        annotationsField.isAccessible = true
        @Suppress("UNCHECKED_CAST")
        val annotations =
            annotationsField.get(declaration) as? MutableList<FirAnnotationCall> ?: return

        val toRemove = annotations.filter { annotation ->
          hasErrorExpressionInAnnotation(annotation)
        }

        if (toRemove.isNotEmpty()) {
          annotations.removeAll(toRemove)
        }
      } catch (e: Exception) {
        // If reflection fails, skip this declaration
      }
    }

    private fun hasErrorExpressionInAnnotation(annotation: FirAnnotationCall): Boolean {
      val argumentList = annotation.argumentList
      if (argumentList is FirResolvedArgumentList) {
        for ((argument, _) in argumentList.mapping) {
          if (hasErrorExpression(argument)) {
            return true
          }
        }
      }
      return false
    }

    @OptIn(SymbolInternals::class)
    private fun hasErrorExpression(element: FirElement): Boolean {
      return when (element) {
        is FirErrorExpression -> true
        is FirNamedArgumentExpression -> hasErrorExpression(element.expression)
        is FirWrappedArgumentExpression -> hasErrorExpression(element.expression)
        is org.jetbrains.kotlin.fir.expressions.FirSpreadArgumentExpression -> {
          hasErrorExpression(element.expression)
        }
        is FirVarargArgumentsExpression -> {
          element.arguments.any { hasErrorExpression(it) }
        }
        is FirQualifiedAccessExpression -> {
          try {
            if (
                element.resolvedType is ConeErrorType ||
                    element.calleeReference is FirErrorNamedReference
            ) {
              return true
            }

            val calleeRef = element.calleeReference
            if (calleeRef is FirResolvedNamedReference) {
              val symbol = calleeRef.resolvedSymbol
              if (symbol is FirPropertySymbol) {
                val prop = symbol.fir
                if (prop.isConst) {
                  val initializer = prop.initializer
                  if (initializer == null || hasErrorExpression(initializer)) {
                    return true
                  }
                  if (isTodoCall(initializer)) {
                    return true
                  }
                  return false
                }
              }
            }
            false
          } catch (_: Exception) {
            true
          }
        }
        is FirGetClassCall -> {
          try {
            val argument = element.argument
            if (argument is FirQualifiedAccessExpression) {
              argument.resolvedType is ConeErrorType
            } else {
              element.resolvedType is ConeErrorType
            }
          } catch (_: Exception) {
            true
          }
        }
        is FirCollectionLiteralCompat -> {
          try {
            element.argumentList.arguments.any { hasErrorExpression(it) }
          } catch (_: Exception) {
            true
          }
        }
        is org.jetbrains.kotlin.fir.expressions.FirFunctionCall -> {
          try {
            element.resolvedType is ConeErrorType ||
                element.calleeReference is FirErrorNamedReference
          } catch (_: Exception) {
            true
          }
        }
        else -> false
      }
    }

    private fun isTodoCall(element: FirElement): Boolean {
      if (element !is org.jetbrains.kotlin.fir.expressions.FirFunctionCall) {
        return false
      }
      val calleeRef = element.calleeReference
      if (calleeRef is FirResolvedNamedReference) {
        val name = calleeRef.name.asString()
        return name == "TODO"
      }
      return false
    }
  }
}

/**
 * IR sanitization stage.
 *
 * Wraps the NonAbiDeclarationsStrippingIrExtension which:
 * - Removes stub source files from IR
 * - Strips SOURCE retention annotations
 * - Strips private declarations (removeNonPublicApi)
 * - Strips private supertypes and converts fake overrides
 * - Stubs method bodies
 */
internal class IrSanitizerStage(
    private val repairLog: AbiGenRepairLog,
    private val inlineBodies: InlineBodyStage,
) : AbiGenStage {
  override val name = "IrSanitizer"

  /** Create the IR generation extension to be registered during FIR-to-IR conversion. */
  fun createExtension(sourceFiles: List<KtFile>): IrGenerationExtension {
    return NonAbiDeclarationsStrippingIrExtension(
        sourceFiles,
        repairLog,
        inlineBodies.restoredIrFunctions,
    )
  }
}

/**
 * Bytecode sanitization stage.
 *
 * ASM-based post-processing of generated .class files:
 * - Strip @Throws annotations from RuntimeInvisibleAnnotations
 * - Strip private declarations from @Metadata annotation
 * - Strip line numbers from method bodies that consumers do not inline
 */
internal class BytecodeSanitizerStage : AbiGenStage {
  override val name = "BytecodeSanitizer"

  private val transformers =
      listOf(ThrowsAnnotationStripper(), PrivateMetadataStripper(), StubLineNumberStripper())

  /** Transform class bytecode in-memory, applying all sanitization transformers. */
  fun transform(bytes: ByteArray): ByteArray {
    var result = bytes
    for (transformer in transformers) {
      val transformed = transformer.transform(result)
      if (transformed != null) {
        result = transformed
      }
    }
    return result
  }

  /** Process all .class files in the output directory (disk-based). */
  fun process(outputDir: File) {
    outputDir
        .walkTopDown()
        .filter { it.extension == "class" }
        .forEach { classFile ->
          var bytes = classFile.readBytes()
          var modified = false
          for (transformer in transformers) {
            val result = transformer.transform(bytes)
            if (result != null) {
              bytes = result
              modified = true
            }
          }
          if (modified) {
            classFile.writeBytes(bytes)
          }
        }
  }
}

internal data class AbiValidationInputs(
    val outputFiles: List<AbiValidationOutputFile>,
    val classpathRoots: List<File>,
)

internal class AbiValidationOutputFile(
    val relativePath: String,
    val bytes: ByteArray,
)

private data class EmittedTypeReference(
    val owner: String,
    val site: String,
    val internalName: String,
    val eligibleForStubOnlyDetection: Boolean,
)

private const val NON_EXISTENT_CLASS_INTERNAL_NAME = "error/NonExistentClass"

/**
 * Validation stage.
 *
 * Answers a single question: does the ABI about to be published differ from what a class-ABI build
 * would have produced, in a way the jar itself cannot reveal?
 *
 * The failure mode this guards against is a *green* build that emits a subtly wrong ABI - a
 * constant holding a placeholder instead of its real value, or a `const val` emitted with no
 * `ConstantValue` attribute at all. Both produce well-formed bytecode, so nothing downstream of
 * here can detect them; the only evidence is what the earlier stages recorded in [AbiGenRepairLog].
 *
 * Assertion 6 extends that to fabricated stub-only types. If stubgen fabricates a class to keep the
 * producer compiling, but no emitted class and no non-stub classpath root actually ships that type,
 * then any surviving descriptor that mentions it is a broken ABI even though the descriptor itself
 * is well-formed. Same-package phantom classes are the motivating case.
 *
 * Rollout: [AbiRepairPolicy.OFF] is the default, so this stage emits nothing unless a target asks
 * for it via `abiValidationMode` - with one exception. The literal-`error/NonExistentClass` check
 * runs unconditionally, because an ABI carrying that sentinel is unlinkable under every policy
 * setting, so gating it on opt-in only decides whether the corruption ships loudly or silently.
 * Warnings are not a softer setting for the rest - fbsource builds Kotlin with `-Werror`, so `warn`
 * fails the compile just as `error` does. Enabling those repo-wide therefore has to wait until the
 * repairs they name have been driven out, not the other way round.
 *
 * The cleared-initializer check carries that visibility filter (Assertion 2 below): a `private
 * const val` is not part of any consumer's constant folding, so repairing one silently is not the
 * defect this stage is looking for. [unsoundConstants] needs no equivalent filter - a fabricated
 * constant only ever reaches that path as an annotation argument, which is by construction
 * consumer-visible.
 */
internal class ValidationStage(private val repairLog: AbiGenRepairLog) : AbiGenStage {
  override val name = "Validation"

  fun validate(
      moduleFragment: IrModuleFragment,
      messageCollector: MessageCollector,
      policy: AbiRepairPolicy,
      inputs: AbiValidationInputs,
  ) {
    // The literal-NEC invariant is not a repair-policy diagnostic and is checked whatever
    // `abiValidationMode` says. An ABI that ships `error/NonExistentClass` cannot be linked
    // against in any configuration, so there is no setting under which emitting one is the
    // intended outcome - and leaving it behind an opt-in mode means the corruption ships
    // silently on every target that has not opted in, which is the default.
    //
    // A clean build pays one substring scan of the emitted class bytes: the sentinel's name
    // reaches the constant pool as ASCII, so a jar that never mentions it cannot need the walk.
    val emittedTypeReferences =
        if (policy != AbiRepairPolicy.OFF || mentionsLiteralErrorType(inputs.outputFiles)) {
          collectEmittedTypeReferences(inputs.outputFiles)
        } else {
          emptySet()
        }

    // The final class bytes are authoritative: source-stage repair bookkeeping and stub classpath
    // candidates can both miss a literal error type that survives a transform or hides in metadata.
    for (reference in emittedTypeReferences) {
      if (reference.internalName != NON_EXISTENT_CLASS_INTERNAL_NAME) continue
      messageCollector.report(
          CompilerMessageSeverity.ERROR,
          "Kosabi ABI validation: `${reference.owner}` emits `${reference.site}` referencing " +
              "literal `$NON_EXISTENT_CLASS_INTERNAL_NAME`. The final ABI bytecode contains an " +
              "unresolved type and no consumer can link against it. " +
              "If the type comes from an explicit reference, add the target that provides it " +
              "to this target's `source_only_abi_deps`, or declare that target " +
              "`required_for_source_only_abi = True`. " +
              "If it was inferred from an expression body, spell the type explicitly " +
              "instead. If it is a nested generic arg of an inherited default method, " +
              "add an explicit override spelling the full signature (see T290092261).",
      )
    }

    if (policy == AbiRepairPolicy.OFF) return

    // Always emitted, including at zero, so that "no repairs happened" is distinguishable from
    // "validation did not run" when aggregating across a build.
    messageCollector.report(CompilerMessageSeverity.INFO, repairLog.counterLine())

    val severity =
        if (policy == AbiRepairPolicy.ERROR) CompilerMessageSeverity.ERROR
        else CompilerMessageSeverity.WARNING

    // Assertion 1: every synthesised constant has a type consistent with a real declaration.
    // Constants that reached ASSUMED_STRING have a fabricated type, not merely a fabricated value.
    for (constant in repairLog.unsoundConstants()) {
      messageCollector.report(
          severity,
          "Kosabi ABI validation: constant `${constant.classId}.${constant.name}` was emitted " +
              "with an assumed type of `String`. Neither its type nor its value could be " +
              "established, so if the real constant is not a String this ABI is wrong in a way " +
              "that will not surface until a consumer compiles against it.",
      )
    }

    // Assertion 2: no *consumer-visible* const val silently lost its ConstantValue attribute.
    //
    // The visibility filter is not a convenience. Every one of the 12 constants this assertion
    // named on its first repo-wide run was a `private const val` initialised from a constant
    // declared in another buck target. A private companion constant takes part in no consumer's
    // constant folding, so the message's own justification - "consumers that constant-fold it fail
    // during their own compile" - cannot apply to it. Reporting those is a false positive, and
    // since `warn` is fatal under `-Werror` a false positive here is a broken build. What they do
    // expose is real but narrower, and belongs to a different check: Kosabi's const resolver is
    // source-local and cannot read a constant's value off a dependency's ABI.
    for (cleared in repairLog.clearedPropertyInitializers) {
      if (!cleared.isConst) continue
      if (!cleared.consumerVisible) continue
      messageCollector.report(
          severity,
          "Kosabi ABI validation: `${cleared.owner}` is a const val whose initializer could not " +
              "be resolved and was discarded. It will be emitted without a ConstantValue " +
              "attribute, so consumers that constant-fold it fail during their own compile.",
      )
    }

    // Assertion 3: a repair that threw leaves the tree in an unknown state. Unlike a placeholder
    // value there is no claim that can be made about the result at all, which makes this the first
    // check that should be promoted to a hard error once the rate is known to be zero.
    for (failure in repairLog.failedRepairs) {
      messageCollector.report(
          severity,
          "Kosabi ABI validation: repair of `${failure.owner}` failed and was previously " +
              "swallowed: ${failure.detail}. The emitted ABI cannot be trusted.",
      )
    }

    // Assertion 4: no supertype survived that the ABI jar will not contain a class file for.
    // Nested private classes are kept in the ABI, so only a top-level private supertype dangles.
    // Unlike the checks above this one is verifiable from the module itself, so it is checked
    // directly rather than trusted.
    val leakedSupertypes = mutableListOf<String>()
    moduleFragment.accept(
        object : IrElementVisitorVoidCompat() {
          override fun visitElement(element: IrElement) {
            element.acceptChildren(this, null)
          }

          override fun visitClass(declaration: IrClass) {
            for (superType in declaration.superTypes) {
              val superClass =
                  (superType as? org.jetbrains.kotlin.ir.types.IrSimpleType)?.classifier?.owner
                      as? IrClass ?: continue
              if (
                  superClass.parent !is IrClass &&
                      superClass.visibility ==
                          org.jetbrains.kotlin.descriptors.DescriptorVisibilities.PRIVATE
              ) {
                leakedSupertypes.add(
                    "${declaration.kotlinFqName.asString()} -> ${superClass.kotlinFqName.asString()}",
                )
              }
            }
            super.visitClass(declaration)
          }
        },
        null,
    )
    for (leaked in leakedSupertypes) {
      messageCollector.report(
          severity,
          "Kosabi ABI validation: dangling private supertype survived stripping: $leaked",
      )
    }

    // Assertion 5: no consumer-visible member reached the descriptor with an unresolved type. A
    // non-API member in that state is degraded to `Any?`; an API member cannot be, so it ships as
    // `Lerror/NonExistentClass;` and no consumer can link against it.
    for (m in repairLog.errorTypedApiMembers) {
      messageCollector.report(
          severity,
          "Kosabi ABI validation: `${m.owner}.${m.member}` has an unresolved ${m.where}, so its " +
              "descriptor was emitted with `error/NonExistentClass` and no consumer can link " +
              "against it. The referenced type is not on the source-only ABI classpath. Add the " +
              "target that provides it to this target's `source_only_abi_deps`, or declare that " +
              "target `required_for_source_only_abi = True`.",
      )
    }

    // Assertion 6: no descriptor in the emitted ABI may mention a stub-only class that is absent
    // from both the emitted jar and the non-stub classpath. That pattern means stubgen preserved
    // compilation by fabricating a class, but source-only ABI is about to publish a phantom type.
    for (leak in
        findStubOnlyDescriptorLeaks(
            inputs,
            emittedTypeReferences,
            messageCollector,
            severity,
        )) {
      messageCollector.report(
          severity,
          "Kosabi ABI validation: `${leak.owner}` emits `${leak.site}` with stub-only type " +
              "`${leak.internalName.replace('/', '.')}`. Kosabi generated a stub for that class, " +
              "but neither this ABI jar nor any non-stub classpath root provides it. This usually " +
              "means stubgen fabricated a same-package phantom from an unresolved simple name. " +
              "Add the real provider to THIS target's `source_only_abi_deps`, or fix the unresolved " +
              "type so source-only ABI does not publish a phantom descriptor.",
      )
    }
  }

  private fun findStubOnlyDescriptorLeaks(
      inputs: AbiValidationInputs,
      emittedTypeReferences: Set<EmittedTypeReference>,
      messageCollector: MessageCollector,
      severity: CompilerMessageSeverity,
  ): List<EmittedTypeReference> {
    val (stubRoots, realRoots) = inputs.classpathRoots.partition { it.isStubClasspathRoot() }
    if (stubRoots.isEmpty()) {
      messageCollector.report(
          severity,
          "Kosabi ABI validation: same-package phantom detection could not complete because no " +
              "stub classpath root (stubgen_stubs.jar / stubs.jar) was found.",
      )
      return emptyList()
    }
    val stubDeclaredClasses =
        collectClassesFromClasspath(stubRoots, messageCollector, severity) ?: return emptyList()
    if (stubDeclaredClasses.isEmpty()) return emptyList()

    val emittedClasses =
        inputs.outputFiles
            .asSequence()
            .filter { it.relativePath.endsWith(".class") }
            .map { classNameFromRelativePath(it.relativePath) }
            .toSet()

    // Subtract the emitted classes first: a stub-declared class that this jar ships itself is never
    // a phantom, and that subtraction needs no classpath I/O. Only if candidates survive do we pay
    // to enumerate the (potentially large) non-stub classpath, keeping the added compile cost
    // proportional to the number of stub-declared candidates rather than the whole classpath.
    val stubOnlyCandidates = stubDeclaredClasses.filterTo(linkedSetOf()) { it !in emittedClasses }
    if (stubOnlyCandidates.isEmpty()) return emptyList()

    // Materialize the non-stub classpath once so each jar is opened at most one time; the phantom
    // check below is then an O(1) set lookup per candidate rather than a jar reopen per candidate.
    val realClasspathClasses =
        collectClassesFromClasspath(realRoots, messageCollector, severity) ?: return emptyList()

    val phantomClasses = stubOnlyCandidates.filterTo(linkedSetOf()) { it !in realClasspathClasses }
    if (phantomClasses.isEmpty()) return emptyList()

    // The shared walk also covers code/debug-only references for the literal NEC invariant.
    // Preserve
    // the existing phantom check's ABI-surface scope so implementation details cannot become new
    // policy failures.
    return emittedTypeReferences.filter { reference ->
      reference.eligibleForStubOnlyDetection &&
          reference.internalName.replace('/', '.') in phantomClasses
    }
  }

  /**
   * True when any emitted class file mentions the error-type sentinel at all. The name reaches the
   * constant pool as ASCII, in descriptors, signatures and metadata alike, so a jar whose bytes do
   * not contain it cannot reference it - which lets the unconditional invariant skip the ASM walk
   * on every clean build.
   */
  private fun mentionsLiteralErrorType(outputFiles: List<AbiValidationOutputFile>): Boolean {
    val needle = NON_EXISTENT_CLASS_INTERNAL_NAME.toByteArray(Charsets.US_ASCII)
    return outputFiles.any { outputFile ->
      outputFile.relativePath.endsWith(".class") && containsBytes(outputFile.bytes, needle)
    }
  }

  private fun containsBytes(haystack: ByteArray, needle: ByteArray): Boolean {
    if (needle.isEmpty() || haystack.size < needle.size) return false
    val last = haystack.size - needle.size
    outer@ for (start in 0..last) {
      for (offset in needle.indices) {
        if (haystack[start + offset] != needle[offset]) continue@outer
      }
      return true
    }
    return false
  }

  private fun collectEmittedTypeReferences(
      outputFiles: List<AbiValidationOutputFile>,
  ): Set<EmittedTypeReference> {
    val references = linkedSetOf<EmittedTypeReference>()
    outputFiles
        .asSequence()
        .filter { it.relativePath.endsWith(".class") }
        .forEach { outputFile ->
          val owner = classNameFromRelativePath(outputFile.relativePath)
          ClassReader(outputFile.bytes)
              .accept(
                  object : ClassVisitor(Opcodes.ASM9) {
                    override fun visit(
                        version: Int,
                        access: Int,
                        name: String?,
                        signature: String?,
                        superName: String?,
                        interfaces: Array<out String>?,
                    ) {
                      collectInternalName(
                          references,
                          owner,
                          "class identity",
                          name,
                          eligibleForStubOnlyDetection = false,
                      )
                      collectInternalName(references, owner, "supertype", superName)
                      interfaces.orEmpty().forEach { iface ->
                        collectInternalName(references, owner, "interface", iface)
                      }
                      collectSignature(references, owner, "class signature", signature)
                    }

                    override fun visitModule(
                        name: String?,
                        access: Int,
                        version: String?,
                    ): ModuleVisitor =
                        object : ModuleVisitor(Opcodes.ASM9) {
                          override fun visitMainClass(mainClass: String?) {
                            collectInternalName(
                                references,
                                owner,
                                "module main class",
                                mainClass,
                                eligibleForStubOnlyDetection = false,
                            )
                          }

                          override fun visitUse(service: String?) {
                            collectInternalName(
                                references,
                                owner,
                                "module service",
                                service,
                                eligibleForStubOnlyDetection = false,
                            )
                          }

                          override fun visitProvide(
                              service: String?,
                              providers: Array<out String>?,
                          ) {
                            collectInternalName(
                                references,
                                owner,
                                "module service",
                                service,
                                eligibleForStubOnlyDetection = false,
                            )
                            providers.orEmpty().forEach { provider ->
                              collectInternalName(
                                  references,
                                  owner,
                                  "module service provider",
                                  provider,
                                  eligibleForStubOnlyDetection = false,
                              )
                            }
                          }
                        }

                    override fun visitOuterClass(
                        outerClassOwner: String?,
                        name: String?,
                        descriptor: String?,
                    ) {
                      collectInternalName(references, owner, "outer class", outerClassOwner)
                      if (descriptor != null) {
                        collectDescriptor(references, owner, "outer method", descriptor)
                      }
                    }

                    override fun visitInnerClass(
                        name: String?,
                        outerName: String?,
                        innerName: String?,
                        access: Int,
                    ) {
                      collectInternalName(references, owner, "inner class", name)
                      collectInternalName(references, owner, "inner class outer", outerName)
                    }

                    override fun visitNestHost(nestHost: String?) {
                      collectInternalName(references, owner, "nest host", nestHost)
                    }

                    override fun visitNestMember(nestMember: String?) {
                      collectInternalName(references, owner, "nest member", nestMember)
                    }

                    override fun visitPermittedSubclass(permittedSubclass: String?) {
                      collectInternalName(
                          references,
                          owner,
                          "permitted subclass",
                          permittedSubclass,
                      )
                    }

                    override fun visitRecordComponent(
                        name: String,
                        descriptor: String,
                        signature: String?,
                    ): RecordComponentVisitor {
                      val site = "record component `$name`"
                      collectDescriptor(references, owner, site, descriptor)
                      collectTypeSignature(references, owner, "$site signature", signature)
                      return object : RecordComponentVisitor(Opcodes.ASM9) {
                        override fun visitAnnotation(
                            descriptor: String,
                            visible: Boolean,
                        ): AnnotationVisitor = annotationVisitor(
                            references,
                            owner,
                            "$site annotation",
                            descriptor,
                        )

                        override fun visitTypeAnnotation(
                            typeRef: Int,
                            typePath: TypePath?,
                            descriptor: String,
                            visible: Boolean,
                        ): AnnotationVisitor = annotationVisitor(
                            references,
                            owner,
                            "$site type annotation",
                            descriptor,
                        )
                      }
                    }

                    override fun visitAnnotation(
                        descriptor: String,
                        visible: Boolean,
                    ): AnnotationVisitor =
                        annotationVisitor(references, owner, "class annotation", descriptor)

                    override fun visitTypeAnnotation(
                        typeRef: Int,
                        typePath: TypePath?,
                        descriptor: String,
                        visible: Boolean,
                    ): AnnotationVisitor =
                        annotationVisitor(references, owner, "class type annotation", descriptor)

                    override fun visitField(
                        access: Int,
                        name: String,
                        descriptor: String,
                        signature: String?,
                        value: Any?,
                    ): FieldVisitor {
                      val site = "field `$name`"
                      collectDescriptor(references, owner, site, descriptor)
                      collectTypeSignature(references, owner, "$site signature", signature)
                      collectConstantValue(references, owner, "$site value", value)
                      return object : FieldVisitor(Opcodes.ASM9) {
                        override fun visitAnnotation(
                            descriptor: String,
                            visible: Boolean,
                        ): AnnotationVisitor = annotationVisitor(
                            references,
                            owner,
                            "$site annotation",
                            descriptor,
                        )

                        override fun visitTypeAnnotation(
                            typeRef: Int,
                            typePath: TypePath?,
                            descriptor: String,
                            visible: Boolean,
                        ): AnnotationVisitor = annotationVisitor(
                            references,
                            owner,
                            "$site type annotation",
                            descriptor,
                        )
                      }
                    }

                    override fun visitMethod(
                        access: Int,
                        name: String,
                        descriptor: String,
                        signature: String?,
                        exceptions: Array<out String>?,
                    ): MethodVisitor {
                      val site = "method `$name$descriptor`"
                      collectDescriptor(references, owner, site, descriptor)
                      collectSignature(references, owner, "$site signature", signature)
                      exceptions.orEmpty().forEach { exceptionInternalName ->
                        collectInternalName(
                            references,
                            owner,
                            "$site throws",
                            exceptionInternalName,
                        )
                      }
                      return object : MethodVisitor(Opcodes.ASM9) {
                        override fun visitAnnotationDefault(): AnnotationVisitor =
                            annotationValueVisitor(references, owner, "$site annotation default")

                        override fun visitAnnotation(
                            descriptor: String,
                            visible: Boolean,
                        ): AnnotationVisitor = annotationVisitor(
                            references,
                            owner,
                            "$site annotation",
                            descriptor,
                        )

                        override fun visitParameterAnnotation(
                            parameter: Int,
                            descriptor: String,
                            visible: Boolean,
                        ): AnnotationVisitor = annotationVisitor(
                            references,
                            owner,
                            "$site parameter annotation",
                            descriptor,
                        )

                        override fun visitTypeAnnotation(
                            typeRef: Int,
                            typePath: TypePath?,
                            descriptor: String,
                            visible: Boolean,
                        ): AnnotationVisitor = annotationVisitor(
                            references,
                            owner,
                            "$site type annotation",
                            descriptor,
                        )

                        override fun visitInsnAnnotation(
                            typeRef: Int,
                            typePath: TypePath?,
                            descriptor: String,
                            visible: Boolean,
                        ): AnnotationVisitor = annotationVisitor(
                            references,
                            owner,
                            "$site instruction type annotation",
                            descriptor,
                            eligibleForStubOnlyDetection = false,
                        )

                        override fun visitTryCatchAnnotation(
                            typeRef: Int,
                            typePath: TypePath?,
                            descriptor: String,
                            visible: Boolean,
                        ): AnnotationVisitor = annotationVisitor(
                            references,
                            owner,
                            "$site try/catch type annotation",
                            descriptor,
                            eligibleForStubOnlyDetection = false,
                        )

                        override fun visitLocalVariableAnnotation(
                            typeRef: Int,
                            typePath: TypePath?,
                            start: Array<out Label>?,
                            end: Array<out Label>?,
                            index: IntArray?,
                            descriptor: String,
                            visible: Boolean,
                        ): AnnotationVisitor = annotationVisitor(
                            references,
                            owner,
                            "$site local variable type annotation",
                            descriptor,
                            eligibleForStubOnlyDetection = false,
                        )

                        override fun visitTypeInsn(opcode: Int, type: String?) {
                          collectInternalName(
                              references,
                              owner,
                              "$site instruction",
                              type,
                              eligibleForStubOnlyDetection = false,
                          )
                        }

                        override fun visitFieldInsn(
                            opcode: Int,
                            instructionOwner: String?,
                            name: String?,
                            descriptor: String?,
                        ) {
                          collectInternalName(
                              references,
                              owner,
                              "$site field instruction owner",
                              instructionOwner,
                              eligibleForStubOnlyDetection = false,
                          )
                          if (descriptor != null) {
                            collectDescriptor(
                                references,
                                owner,
                                "$site field instruction",
                                descriptor,
                                eligibleForStubOnlyDetection = false,
                            )
                          }
                        }

                        override fun visitMethodInsn(
                            opcode: Int,
                            instructionOwner: String?,
                            name: String?,
                            descriptor: String?,
                            isInterface: Boolean,
                        ) {
                          collectInternalName(
                              references,
                              owner,
                              "$site method instruction owner",
                              instructionOwner,
                              eligibleForStubOnlyDetection = false,
                          )
                          if (descriptor != null) {
                            collectDescriptor(
                                references,
                                owner,
                                "$site method instruction",
                                descriptor,
                                eligibleForStubOnlyDetection = false,
                            )
                          }
                        }

                        override fun visitInvokeDynamicInsn(
                            name: String?,
                            descriptor: String?,
                            bootstrapMethodHandle: Handle?,
                            vararg bootstrapMethodArguments: Any?,
                        ) {
                          if (descriptor != null) {
                            collectDescriptor(
                                references,
                                owner,
                                "$site invokedynamic",
                                descriptor,
                                eligibleForStubOnlyDetection = false,
                            )
                          }
                          collectConstantValue(
                              references,
                              owner,
                              "$site invokedynamic bootstrap",
                              bootstrapMethodHandle,
                              eligibleForStubOnlyDetection = false,
                          )
                          bootstrapMethodArguments.forEach { argument ->
                            collectConstantValue(
                                references,
                                owner,
                                "$site invokedynamic bootstrap argument",
                                argument,
                                eligibleForStubOnlyDetection = false,
                            )
                          }
                        }

                        override fun visitLdcInsn(value: Any?) {
                          collectConstantValue(
                              references,
                              owner,
                              "$site constant",
                              value,
                              eligibleForStubOnlyDetection = false,
                          )
                        }

                        override fun visitMultiANewArrayInsn(
                            descriptor: String?,
                            numDimensions: Int,
                        ) {
                          if (descriptor != null) {
                            collectDescriptor(
                                references,
                                owner,
                                "$site multi-dimensional array",
                                descriptor,
                                eligibleForStubOnlyDetection = false,
                            )
                          }
                        }

                        override fun visitTryCatchBlock(
                            start: Label?,
                            end: Label?,
                            handler: Label?,
                            type: String?,
                        ) {
                          collectInternalName(
                              references,
                              owner,
                              "$site catch type",
                              type,
                              eligibleForStubOnlyDetection = false,
                          )
                        }

                        override fun visitLocalVariable(
                            name: String?,
                            descriptor: String?,
                            signature: String?,
                            start: Label?,
                            end: Label?,
                            index: Int,
                        ) {
                          if (descriptor != null) {
                            collectDescriptor(
                                references,
                                owner,
                                "$site local variable",
                                descriptor,
                                eligibleForStubOnlyDetection = false,
                            )
                          }
                          collectTypeSignature(
                              references,
                              owner,
                              "$site local variable signature",
                              signature,
                              eligibleForStubOnlyDetection = false,
                          )
                        }

                        override fun visitFrame(
                            type: Int,
                            numLocal: Int,
                            local: Array<out Any>?,
                            numStack: Int,
                            stack: Array<out Any>?,
                        ) {
                          local.orEmpty().filterIsInstance<String>().forEach { internalName ->
                            collectInternalName(
                                references,
                                owner,
                                "$site stack map frame",
                                internalName,
                                eligibleForStubOnlyDetection = false,
                            )
                          }
                          stack.orEmpty().filterIsInstance<String>().forEach { internalName ->
                            collectInternalName(
                                references,
                                owner,
                                "$site stack map frame",
                                internalName,
                                eligibleForStubOnlyDetection = false,
                            )
                          }
                        }
                      }
                    }
                  },
                  0,
              )
        }
    return references
  }

  private fun collectDescriptor(
      references: MutableSet<EmittedTypeReference>,
      owner: String,
      site: String,
      descriptor: String,
      eligibleForStubOnlyDetection: Boolean = true,
  ) {
    collectType(
        references,
        owner,
        site,
        Type.getType(descriptor),
        eligibleForStubOnlyDetection,
    )
  }

  private fun collectType(
      references: MutableSet<EmittedTypeReference>,
      owner: String,
      site: String,
      type: Type,
      eligibleForStubOnlyDetection: Boolean = true,
  ) {
    when (type.sort) {
      Type.ARRAY ->
          collectType(references, owner, site, type.elementType, eligibleForStubOnlyDetection)
      Type.OBJECT ->
          collectInternalName(
              references,
              owner,
              site,
              type.internalName,
              eligibleForStubOnlyDetection,
          )
      Type.METHOD -> {
        type.argumentTypes.forEach { argumentType ->
          collectType(references, owner, site, argumentType, eligibleForStubOnlyDetection)
        }
        collectType(references, owner, site, type.returnType, eligibleForStubOnlyDetection)
      }
    }
  }

  private fun collectInternalName(
      references: MutableSet<EmittedTypeReference>,
      owner: String,
      site: String,
      internalName: String?,
      eligibleForStubOnlyDetection: Boolean = true,
  ) {
    if (internalName == null) return
    if (internalName.startsWith("[")) {
      collectDescriptor(references, owner, site, internalName, eligibleForStubOnlyDetection)
    } else {
      references.add(
          EmittedTypeReference(owner, site, internalName, eligibleForStubOnlyDetection),
      )
    }
  }

  private fun collectConstantValue(
      references: MutableSet<EmittedTypeReference>,
      owner: String,
      site: String,
      value: Any?,
      eligibleForStubOnlyDetection: Boolean = true,
  ) {
    when (value) {
      is Type -> collectType(references, owner, site, value, eligibleForStubOnlyDetection)
      is Handle -> {
        collectInternalName(
            references,
            owner,
            "$site owner",
            value.owner,
            eligibleForStubOnlyDetection,
        )
        collectDescriptor(
            references,
            owner,
            site,
            value.desc,
            eligibleForStubOnlyDetection,
        )
      }
      is ConstantDynamic -> {
        collectDescriptor(
            references,
            owner,
            site,
            value.descriptor,
            eligibleForStubOnlyDetection,
        )
        collectConstantValue(
            references,
            owner,
            "$site bootstrap",
            value.bootstrapMethod,
            eligibleForStubOnlyDetection,
        )
        for (index in 0 until value.bootstrapMethodArgumentCount) {
          collectConstantValue(
              references,
              owner,
              "$site bootstrap argument",
              value.getBootstrapMethodArgument(index),
              eligibleForStubOnlyDetection,
          )
        }
      }
    }
  }

  private fun annotationVisitor(
      references: MutableSet<EmittedTypeReference>,
      owner: String,
      site: String,
      descriptor: String,
      eligibleForStubOnlyDetection: Boolean = true,
  ): AnnotationVisitor {
    collectDescriptor(references, owner, site, descriptor, eligibleForStubOnlyDetection)
    return annotationValueVisitor(references, owner, site, eligibleForStubOnlyDetection)
  }

  private fun annotationValueVisitor(
      references: MutableSet<EmittedTypeReference>,
      owner: String,
      site: String,
      eligibleForStubOnlyDetection: Boolean = true,
  ): AnnotationVisitor =
      object : AnnotationVisitor(Opcodes.ASM9) {
        override fun visit(name: String?, value: Any?) {
          collectConstantValue(references, owner, site, value, eligibleForStubOnlyDetection)
        }

        override fun visitEnum(name: String?, descriptor: String, value: String?) {
          collectDescriptor(references, owner, site, descriptor, eligibleForStubOnlyDetection)
        }

        override fun visitAnnotation(
            name: String?,
            descriptor: String,
        ): AnnotationVisitor = annotationVisitor(
            references,
            owner,
            site,
            descriptor,
            eligibleForStubOnlyDetection,
        )

        override fun visitArray(name: String?): AnnotationVisitor =
            annotationValueVisitor(references, owner, site, eligibleForStubOnlyDetection)
      }

  private fun collectSignature(
      references: MutableSet<EmittedTypeReference>,
      owner: String,
      site: String,
      signature: String?,
      eligibleForStubOnlyDetection: Boolean = true,
  ) {
    if (signature == null) return
    SignatureReader(signature)
        .accept(signatureVisitor(references, owner, site, eligibleForStubOnlyDetection))
  }

  private fun collectTypeSignature(
      references: MutableSet<EmittedTypeReference>,
      owner: String,
      site: String,
      signature: String?,
      eligibleForStubOnlyDetection: Boolean = true,
  ) {
    if (signature == null) return
    SignatureReader(signature)
        .acceptType(signatureVisitor(references, owner, site, eligibleForStubOnlyDetection))
  }

  private fun signatureVisitor(
      references: MutableSet<EmittedTypeReference>,
      owner: String,
      site: String,
      eligibleForStubOnlyDetection: Boolean = true,
  ): SignatureVisitor =
      object : SignatureVisitor(Opcodes.ASM9) {
        private var currentClassInternalName: String? = null

        override fun visitClassType(name: String?) {
          currentClassInternalName = name
          collectInternalName(references, owner, site, name, eligibleForStubOnlyDetection)
        }

        override fun visitInnerClassType(name: String?) {
          val enclosing = currentClassInternalName
          val nestedName =
              when {
                name == null -> null
                enclosing == null -> name
                else -> enclosing + "$" + name
              }
          currentClassInternalName = nestedName
          collectInternalName(
              references,
              owner,
              site,
              nestedName,
              eligibleForStubOnlyDetection,
          )
        }

        override fun visitClassBound(): SignatureVisitor =
            signatureVisitor(references, owner, site, eligibleForStubOnlyDetection)

        override fun visitInterfaceBound(): SignatureVisitor =
            signatureVisitor(references, owner, site, eligibleForStubOnlyDetection)

        override fun visitSuperclass(): SignatureVisitor =
            signatureVisitor(references, owner, site, eligibleForStubOnlyDetection)

        override fun visitInterface(): SignatureVisitor =
            signatureVisitor(references, owner, site, eligibleForStubOnlyDetection)

        override fun visitParameterType(): SignatureVisitor =
            signatureVisitor(references, owner, site, eligibleForStubOnlyDetection)

        override fun visitReturnType(): SignatureVisitor =
            signatureVisitor(references, owner, site, eligibleForStubOnlyDetection)

        override fun visitExceptionType(): SignatureVisitor =
            signatureVisitor(references, owner, site, eligibleForStubOnlyDetection)

        override fun visitArrayType(): SignatureVisitor =
            signatureVisitor(references, owner, site, eligibleForStubOnlyDetection)

        override fun visitTypeArgument(wildcard: Char): SignatureVisitor =
            signatureVisitor(references, owner, site, eligibleForStubOnlyDetection)
      }

  private fun classNameFromRelativePath(relativePath: String): String {
    return relativePath.removeSuffix(".class").replace('/', '.')
  }

  private fun collectClassesFromClasspath(
      classpathRoots: List<File>,
      messageCollector: MessageCollector,
      severity: CompilerMessageSeverity,
  ): Set<String>? {
    val classes = linkedSetOf<String>()
    for (root in classpathRoots) {
      try {
        when {
          root.isDirectory -> {
            root
                .walkTopDown()
                .filter { it.isFile && it.extension == "class" }
                .forEach { classFile ->
                  val relativePath = classFile.relativeTo(root).invariantSeparatorsPath
                  classes.add(classNameFromRelativePath(relativePath))
                }
          }
          root.isFile && root.extension == "jar" -> {
            JarFile(root).use { jarFile ->
              jarFile
                  .entries()
                  .asSequence()
                  .filter { entry -> !entry.isDirectory && entry.name.endsWith(".class") }
                  .forEach { entry -> classes.add(classNameFromRelativePath(entry.name)) }
            }
          }
        }
      } catch (failure: Exception) {
        messageCollector.report(
            severity,
            "Kosabi ABI validation: same-package phantom detection could not complete because " +
                "classpath root `$root` could not be read: $failure",
        )
        return null
      }
    }
    return classes
  }

  // KosabiStubgenStepsBuilder emits the stub classpath as `__%s_stubgen_stubs.jar`, or as the
  // corresponding `__%s_stubgen_stubs` directory before packaging. `stubs.jar` stays an equality
  // check: as a suffix it would also swallow `core-lambda-stubs.jar` and treat the Android lambda
  // stubs as fabricated classes.
  private fun File.isStubClasspathRoot(): Boolean =
      (isDirectory && name.endsWith("stubgen_stubs")) ||
          (isFile && (name.endsWith("stubgen_stubs.jar") || name == "stubs.jar"))
}

/**
 * Pipeline runner that orchestrates the ABI generation stages in the correct order.
 *
 * The pipeline stages are:
 * 1. FIR pre-IR cleanup (FirMetadataSanitizerStage.cleanupFirTree)
 * 2. FIR-to-IR conversion with IR sanitizer extension
 * 3. FIR metadata post-IR cleanup (FirMetadataSanitizerStage.cleanupFirMetadataSources)
 * 4. Code generation + in-memory bytecode transforms + write to disk
 * 5. Validation (ValidationStage.validate)
 *
 * Steps 2 and 4 are handled by the caller (K2JvmAbiFirAnalysisHandlerExtension) since they involve
 * compiler infrastructure (FIR-to-IR conversion, code generation).
 */
internal class AbiGenPipeline(
    /** Shared by every stage; the only record of repairs the emitted jar cannot reveal. */
    val repairLog: AbiGenRepairLog = AbiGenRepairLog(),
    val composeAbi: ComposeAbiEmulationStage = ComposeAbiEmulationStage(),
    val bytecodeSanitizer: BytecodeSanitizerStage = BytecodeSanitizerStage(),
) {
  val firMetadataSanitizer: FirMetadataSanitizerStage = FirMetadataSanitizerStage(repairLog)
  val inlineBodies: InlineBodyStage = InlineBodyStage()
  val irSanitizer: IrSanitizerStage = IrSanitizerStage(repairLog, inlineBodies)
  val validator: ValidationStage = ValidationStage(repairLog)

  val stages: List<AbiGenStage>
    get() = listOf(
        firMetadataSanitizer,
        composeAbi,
        inlineBodies,
        irSanitizer,
        bytecodeSanitizer,
        validator,
    )
}
