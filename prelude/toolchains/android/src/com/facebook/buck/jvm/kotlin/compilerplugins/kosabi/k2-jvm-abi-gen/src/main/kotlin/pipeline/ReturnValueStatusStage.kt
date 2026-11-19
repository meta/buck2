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
@file:OptIn(com.facebook.DirectDeclarationsAccessCompat::class)

package com.facebook

import java.io.File
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.containingClassLookupTag
import org.jetbrains.kotlin.fir.declarations.FirCallableDeclaration
import org.jetbrains.kotlin.fir.declarations.FirDeclarationOrigin
import org.jetbrains.kotlin.fir.declarations.FirMemberDeclaration
import org.jetbrains.kotlin.fir.declarations.FirProperty
import org.jetbrains.kotlin.fir.declarations.FirRegularClass
import org.jetbrains.kotlin.fir.declarations.FirResolvedDeclarationStatus
import org.jetbrains.kotlin.fir.declarations.utils.sourceElement
import org.jetbrains.kotlin.fir.java.declarations.FirJavaClass
import org.jetbrains.kotlin.fir.resolve.ScopeSession
import org.jetbrains.kotlin.fir.resolve.providers.symbolProvider
import org.jetbrains.kotlin.fir.symbols.SymbolInternals
import org.jetbrains.kotlin.fir.symbols.impl.FirClassSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirTypeAliasSymbol
import org.jetbrains.kotlin.fir.types.ConeClassLikeType
import org.jetbrains.kotlin.fir.types.ConeErrorType
import org.jetbrains.kotlin.fir.types.ConeKotlinType
import org.jetbrains.kotlin.fir.types.FirResolvedTypeRef
import org.jetbrains.kotlin.fir.unwrapFakeOverrides
import org.jetbrains.kotlin.load.java.sources.JavaSourceElement
import org.jetbrains.kotlin.load.java.structure.impl.VirtualFileBoundJavaClass
import org.jetbrains.kotlin.load.kotlin.KotlinJvmBinarySourceElement
import org.jetbrains.kotlin.name.ClassId

private const val MUST_USE_RETURN_VALUE_STATUS = "MustUse"

/** Stands for a supertype that does not resolve at all, which has no class id to name. */
private const val UNRESOLVED_SUPERTYPE = "<unresolved supertype>"

/**
 * Fails the named steps of this stage, separated by commas, so tests can reach the fallbacks for a
 * stage that throws: `decide:<class id>.<member>` and `repair:<class id>.<member>` for an override,
 * `delegate:<class id>` and `delegateCopies:<class id>` for the members a class delegates.
 */
const val RETURN_VALUE_STATUS_INJECTED_FAILURES_PROPERTY =
    "kosabi.abi.returnValueStatus.injectFailure"

/**
 * Return-value status of overrides that may override a member the source-only ABI compile cannot
 * read.
 *
 * The compiler gives an override the return-value status of the first member it overrides. A
 * supertype outside the source-only ABI classpath reaches this compile as a member-less stub or not
 * at all, so an override of one of its members finds nothing to override here, or only other,
 * visible members, while the library inherits the hidden member's status. When the library status
 * is `MustUse` and this compile resolves a weaker one, a consumer compiled with the checker that
 * discards the result compiles against this ABI and fails against the class ABI, which release
 * builds use.
 *
 * This stage writes `MustUse`, the strictest status such an override can have in the library, for
 * every override that may override an unreadable member: an override that overrides nothing here,
 * and an override or data-class `componentN` whose class has a supertype this compile cannot read,
 * directly or through another supertype. Overrides whose own annotations decide their status keep
 * the compiler's status. An override of a repaired override in this module is repaired itself, as
 * its class reaches the same unreadable class. A member a class implements by delegation (`by`)
 * gets the status written for the member it wraps. When the stage cannot decide for an override, it
 * writes `MustUse` too. This ABI is then never laxer than the class ABI; it is stricter where the
 * hidden member is not `MustUse`. With validation on, every changed status is recorded in
 * [AbiGenRepairLog] so [ValidationStage] can name it with the unreadable class; adding the target
 * that provides that class to this target's `source_only_abi_deps` makes the status exact.
 */
internal class ReturnValueStatusStage(private val repairLog: AbiGenRepairLog) : AbiGenStage {
  override val name = "ReturnValueStatus"

  /**
   * Must run on the resolved FIR, before any stage edits supertypes. [record] keeps every changed
   * status for [ValidationStage], which reads them only when validation is on.
   */
  fun repair(analysisResults: FrontendOutputCompat, record: Boolean) {
    // The compiler has no return-value statuses before Kotlin 2.3.
    if (RETURN_VALUE_STATUS_NAMES.isEmpty()) return
    val injectedFailures =
        System.getProperty(RETURN_VALUE_STATUS_INJECTED_FAILURES_PROPERTY)
            ?.split(',')
            ?.mapTo(HashSet()) { it.trim() }
            .orEmpty()
    for (output in analysisResults.outputs) {
      val overrides = mutableListOf<Override>()
      val classes = mutableListOf<FirRegularClass>()
      for (file in output.fir) {
        for (declaration in file.declarations) {
          if (declaration is FirRegularClass) collect(declaration, overrides, classes)
        }
      }
      ModuleRepair(output.session, output.scopeSession, record, injectedFailures)
          .run(overrides, classes)
    }
  }

  /** [generated] marks a data-class `componentN`, which implements a member without `override`. */
  private class Override(
      val member: FirCallableDeclaration,
      val owner: FirRegularClass,
      val generated: Boolean,
  )

  /** A status this stage wrote, over [computed], the status the compiler resolved. */
  private class Change(
      val owner: FirRegularClass,
      val computed: String,
      val emitted: String,
      val unreadableClasses: List<String>,
  )

  private fun collect(
      owner: FirRegularClass,
      overrides: MutableList<Override>,
      classes: MutableList<FirRegularClass>,
  ) {
    classes += owner
    for (declaration in owner.declarations) {
      when (declaration) {
        is FirRegularClass -> collect(declaration, overrides, classes)
        is FirNamedFunctionCompat,
        is FirProperty -> {
          val member = declaration as FirCallableDeclaration
          when {
            member.status.isOverride -> overrides += Override(member, owner, generated = false)
            member.isGeneratedComponent() -> overrides += Override(member, owner, generated = true)
          }
        }
        else -> {}
      }
    }
  }

  private inner class ModuleRepair(
      val session: FirSession,
      val scopeSession: ScopeSession,
      val record: Boolean,
      val injectedFailures: Set<String>,
  ) {
    /** Members whose status this stage changed, in the order it changed them. */
    private val changed = LinkedHashMap<FirCallableDeclaration, Change>()

    /** The first unreadable class a class is or reaches, or `null` if it reaches none. */
    private val unreadableClass = HashMap<ClassId, String?>()

    fun run(overrides: List<Override>, classes: List<FirRegularClass>) {
      for (override in overrides) {
        val (member, owner) = override.member to override.owner
        val decision =
            attempt(owner, member, "decide") {
              val overridden =
                  overriddenMembersCompat(session, scopeSession, member, owner).orEmpty()
              unreadableOrigins(override, overridden)
            }
        // Without a decision the override may inherit from anything: `MustUse` is never laxer.
        val origins = if (decision.isFailure) emptyList() else (decision.getOrNull() ?: continue)
        if (attempt(owner, member, "repair") { repair(member, owner, origins) }.isFailure) {
          attempt(owner, member, "force") {
            // An own ignorable-like annotation decides the status in both compiles.
            if (!isIgnorableByOwnAnnotationCompat(session, member)) {
              write(member, owner, MUST_USE_RETURN_VALUE_STATUS, origins)
            }
          }
        }
      }
      if (changed.isNotEmpty()) repairDelegated(classes)
      if (record) {
        for ((member, change) in changed) {
          if (change.owner.isNonApi() || member.isNonApi()) continue
          repairLog.recordUnknownReturnValueStatus(
              UnknownReturnValueStatus(
                  owner = change.owner.symbol.classId.asString(),
                  member = member.symbol.name.asString(),
                  computed = change.computed,
                  emitted = change.emitted,
                  unreadableClasses = change.unreadableClasses,
              ),
          )
        }
      }
    }

    /**
     * The unreadable classes [override] may inherit its status through, or `null` when it cannot
     * inherit from an unreadable member. It may when it overrides nothing here, or when a direct
     * supertype of its class is or reaches an unreadable type, whose members the library may find
     * first, and does not declare a member [override] overrides as a Kotlin declaration that is not
     * an override. Only such a declaration gets the same status in both compiles whatever is behind
     * it; a Java method can override a hidden member without `override`. A generated `componentN`
     * overrides nothing unless its class extends a supertype declaring it.
     */
    private fun unreadableOrigins(
        override: Override,
        overridden: List<FirCallableDeclaration>,
    ): List<String>? {
      val supertypes = override.owner.superTypeRefs.map { (it as? FirResolvedTypeRef)?.coneType }
      if (overridden.isEmpty() && !override.generated) {
        return supertypes.mapNotNull { unreadableIn(it, HashSet()) }.distinct()
      }
      val barriers =
          overridden.mapNotNullTo(HashSet()) { member ->
            val declaration = member.unwrapFakeOverrides()
            val classId = declaration.symbol.containingClassLookupTag()?.classId
            classId?.takeUnless { declaration.status.isOverride || isJavaClass(it) }
          }
      return supertypes
          .filter { (it as? ConeClassLikeType)?.lookupTag?.classId !in barriers }
          .mapNotNull { unreadableIn(it, HashSet()) }
          .distinct()
          .ifEmpty { null }
    }

    private fun isJavaClass(classId: ClassId): Boolean {
      val origin = session.symbolProvider.getClassLikeSymbolByClassId(classId)?.origin
      return origin is FirDeclarationOrigin.Java
    }

    /**
     * The first class [type]'s class is or reaches that is unreadable here: missing from this
     * compile, or a member-less stand-in that stubgen fabricated for a class off the classpath.
     */
    private fun unreadableIn(type: ConeKotlinType?, visiting: MutableSet<ClassId>): String? {
      if (type == null || type is ConeErrorType) return UNRESOLVED_SUPERTYPE
      val classId = (type as? ConeClassLikeType)?.lookupTag?.classId ?: return null
      if (unreadableClass.containsKey(classId)) return unreadableClass[classId]
      if (!visiting.add(classId)) return null
      val found =
          when (val symbol = session.symbolProvider.getClassLikeSymbolByClassId(classId)) {
            null -> classId.asString()
            is FirTypeAliasSymbol -> unreadableIn(symbol.resolvedExpandedTypeRef.coneType, visiting)
            is FirClassSymbol<*> ->
                if (symbol.isFabricatedStub()) {
                  classId.asString()
                } else {
                  symbol.resolvedSuperTypes.firstNotNullOfOrNull { unreadableIn(it, visiting) }
                }
            else -> null
          }
      unreadableClass[classId] = found
      return found
    }

    /**
     * Writes the status [member] gets when the member it overrides is `MustUse`, unless its own
     * annotations decide its status whatever it overrides: then the compiler's status is exact.
     */
    private fun repair(
        member: FirCallableDeclaration,
        owner: FirRegularClass,
        origins: List<String>,
    ) {
      val possible = RETURN_VALUE_STATUS_NAMES.associateWith { overriddenStatus ->
        returnValueStatusOverridingCompat(session, member, owner, overriddenStatus)
      }
      if (possible.values.toSet().size == 1) return
      write(member, owner, possible.getValue(MUST_USE_RETURN_VALUE_STATUS), origins)
    }

    private fun write(
        member: FirCallableDeclaration,
        owner: FirRegularClass,
        status: String,
        origins: List<String>,
    ) {
      val current = returnValueStatusCompat(member) ?: return
      if (current == status) return
      if (!replaceReturnValueStatusCompat(member, status)) {
        repairLog.recordFailedRepair(
            key(owner, member),
            "could not replace the return-value status $current with $status",
        )
        return
      }
      changed[member] = Change(owner, current, status, origins)
    }

    /**
     * A member a class implements by delegation copies the status of the member it wraps when the
     * compiler first looks its name up in the class, which happens before this stage whenever the
     * class or a subclass declares a member of that name. Gives each copy of a member this stage
     * changed the status written for that member; copies made from now on read it themselves. When
     * the wrapped members cannot be read, every copy under a changed name gets `MustUse` unless the
     * member it wraps is ignorable by its own annotation, which makes the copy ignorable in both
     * compiles.
     */
    private fun repairDelegated(classes: List<FirRegularClass>) {
      val names = changed.keys.mapTo(HashSet()) { it.symbol.name }
      for (klass in classes) {
        val delegated =
            attempt(klass, "delegate", "read the members it delegates and the members they wrap") {
              delegatedMembersCompat(session, scopeSession, klass, names)
            }
        if (delegated.isSuccess) {
          for ((copy, wrapped) in delegated.getOrThrow()) {
            val change = wrapped.firstNotNullOfOrNull { changed[it] } ?: continue
            write(copy, klass, change.emitted, change.unreadableClasses)
          }
          continue
        }
        val copies =
            attempt(klass, "delegateCopies", "read the members it delegates") {
              delegatedCopiesCompat(session, scopeSession, klass, names)
            }
        for (copy in copies.getOrNull().orEmpty()) {
          val ignorable = runCatching {
            delegatedWrappedMemberCompat(copy)?.let {
              isIgnorableByOwnAnnotationCompat(session, it)
            }
          }
          if (ignorable.getOrNull() == true) continue
          write(copy, klass, MUST_USE_RETURN_VALUE_STATUS, emptyList())
        }
      }
    }

    private fun <T> attempt(
        owner: FirRegularClass,
        member: FirCallableDeclaration,
        step: String,
        block: () -> T,
    ): Result<T> = attempt(key(owner, member), step, "$step its return-value status", block)

    private fun <T> attempt(
        klass: FirRegularClass,
        step: String,
        action: String,
        block: () -> T,
    ): Result<T> = attempt(klass.symbol.classId.asString(), step, action, block)

    private fun <T> attempt(key: String, step: String, action: String, block: () -> T): Result<T> =
        try {
          check("$step:$key" !in injectedFailures) { "injected failure" }
          Result.success(block())
        } catch (failure: RuntimeException) {
          repairLog.recordFailedRepair(
              key,
              "could not $action: ${failure.javaClass.simpleName}: ${failure.message}",
          )
          Result.failure(failure)
        }
  }

  private fun key(owner: FirRegularClass, member: FirCallableDeclaration): String =
      "${owner.symbol.classId.asString()}.${member.symbol.name.asString()}"

  // No consumer sees a member of a private or local class, so its status is not ABI.
  private fun FirMemberDeclaration.isNonApi(): Boolean =
      (status as? FirResolvedDeclarationStatus)?.effectiveVisibility?.privateApi ?: false
}

/** A data class's generated `componentN`. */
private fun FirCallableDeclaration.isGeneratedComponent(): Boolean =
    origin == FirDeclarationOrigin.Synthetic.DataClassMember &&
        symbol.name.asString().startsWith("component")

/** Whether stubgen fabricated this class: its class file comes from the stub classpath. */
@OptIn(SymbolInternals::class)
private fun FirClassSymbol<*>.isFabricatedStub(): Boolean {
  val location =
      when (val fir = fir) {
        is FirJavaClass ->
            if (fir.origin is FirDeclarationOrigin.Java.Library) {
              runCatching { fir.toBinaryJavaClassCompat().virtualFile?.path }.getOrNull()
            } else {
              null
            }
        is FirRegularClass ->
            when (val source = fir.sourceElement) {
              is KotlinJvmBinarySourceElement -> source.binaryClass.location
              is JavaSourceElement ->
                  (source.javaElement as? VirtualFileBoundJavaClass)?.virtualFile?.path
              else -> null
            }
        else -> null
      } ?: return false
  val path = location.replace('\\', '/')
  val jar = path.substringBefore("!/", missingDelimiterValue = "")
  if (jar.isNotEmpty()) return File(jar).isStubClasspathRoot()
  return generateSequence(File(path).parentFile) { it.parentFile }.any { it.isStubClasspathRoot() }
}
