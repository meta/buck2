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

package com.facebook

import java.util.IdentityHashMap

/**
 * How the ABI generator reacts when it has to emit content it knows is not faithful to the real
 * declaration.
 *
 * `ERROR` is the only setting under which a green build implies a correct ABI.
 *
 * [WARN] is not a "keeps existing targets building" setting in this repository. fbsource compiles
 * Kotlin with `-Werror`, so a `CompilerMessageSeverity.WARNING` terminates the compile exactly as
 * [ERROR] would; the two differ only in the message text. Any rollout that treats warnings as the
 * safe intermediate step will break every target that trips a repair.
 */
enum class AbiRepairPolicy {
  OFF,
  WARN,
  ERROR;

  companion object {
    /**
     * Absent or unrecognised values fall back to [OFF].
     *
     * The plugin cannot see whether the enclosing compile passes `-Werror`, so it cannot pick a
     * non-fatal severity on its own. Reporting therefore has to be requested explicitly by whoever
     * knows the target is prepared for it.
     */
    fun parse(value: String?): AbiRepairPolicy =
        when (value?.lowercase()) {
          "warn" -> WARN
          "error",
          "fail" -> ERROR
          else -> OFF
        }
  }
}

/** Where the type of a fabricated constant came from. */
enum class ConstTypeSource {
  /** Taken from the declared type of the annotation parameter the constant is passed to. */
  ANNOTATION_PARAMETER,
  /**
   * No type information was available anywhere, so `String` was assumed. Any non-`String` constant
   * reaching this state produces an ABI whose type does not match the real declaration.
   */
  ASSUMED_STRING,
}

/**
 * A constant that had to be synthesised because its declaring class was not on the source-only ABI
 * classpath.
 *
 * The value is never recoverable in this situation - only the type sometimes is. See
 * [AbiGenRepairLog].
 */
data class FabricatedConstant(
    val classId: String,
    val name: String,
    val emittedType: String,
    val typeSource: ConstTypeSource,
)

/**
 * An initializer that was discarded because it could not be resolved.
 *
 * [isConst] and [consumerVisible] exist so [ValidationStage] can tell the defect it is looking for
 * - a `const val` a consumer will constant-fold - from a `private const val`, which no consumer can
 *   see and which therefore takes part in no consumer's constant folding. Both default to the
 *   conservative answer for the record kinds that do not set them.
 */
data class DiscardedInitializer(
    val owner: String,
    val detail: String,
    val isConst: Boolean = false,
    val consumerVisible: Boolean = true,
    val unresolvedSymbols: List<String> = emptyList(),
)

/**
 * A consumer-visible member whose signature did not resolve, so the emitted descriptor carries
 * `error/NonExistentClass` in the slot named by [where].
 *
 * A non-API member in this state is degraded to `Any?`; an API member cannot be, because the
 * descriptor IS the ABI. The only honest outcomes are to fail or to ship a stub no consumer can
 * link against.
 */
data class ErrorTypedApiMember(val owner: String, val member: String, val where: String)

/**
 * Record of every place the ABI generator knowingly emitted something other than what the source
 * says, plus every place a best-effort repair failed outright.
 *
 * Kosabi cannot see the value of a constant whose declaring target is absent from the source-only
 * ABI classpath, so it substitutes a placeholder. That substitution is not detectable in the
 * resulting jar - the bytecode is well formed, it just holds the wrong number. This log is the only
 * signal that it happened, so nothing here may be dropped on the floor.
 *
 * One instance is shared by all pipeline stages for a single compilation.
 */
class AbiGenRepairLog {
  val fabricatedConstants: MutableList<FabricatedConstant> = mutableListOf()
  val clearedPropertyInitializers: MutableList<DiscardedInitializer> = mutableListOf()
  val replacedFieldInitializers: MutableList<DiscardedInitializer> = mutableListOf()
  val strippedSupertypes: MutableList<DiscardedInitializer> = mutableListOf()

  /**
   * Repairs that did not even complete - a reflective mutation threw, so the FIR tree is in
   * whatever state the failure left it in. Previously these were swallowed by empty catch blocks.
   */
  val failedRepairs: MutableList<DiscardedInitializer> = mutableListOf()

  private val errorTypedApiPositionsByDeclaration: IdentityHashMap<Any, MutableSet<String>> =
      IdentityHashMap()

  val errorTypedApiMembers: MutableList<ErrorTypedApiMember> = mutableListOf()

  fun recordErrorTypedApiMember(
      declarationIdentity: Any,
      owner: String,
      member: String,
      where: String,
  ) {
    val positions =
        errorTypedApiPositionsByDeclaration.getOrPut(declarationIdentity) { mutableSetOf() }
    if (positions.add(where)) {
      errorTypedApiMembers.add(ErrorTypedApiMember(owner, member, where))
    }
  }

  fun recordFabricatedConstant(constant: FabricatedConstant) {
    fabricatedConstants.add(constant)
  }

  fun recordClearedPropertyInitializer(
      owner: String,
      detail: String,
      isConst: Boolean = false,
      consumerVisible: Boolean = true,
      unresolvedSymbols: List<String> = emptyList(),
  ) {
    clearedPropertyInitializers.add(
        DiscardedInitializer(owner, detail, isConst, consumerVisible, unresolvedSymbols),
    )
  }

  fun recordReplacedFieldInitializer(owner: String, detail: String) {
    replacedFieldInitializers.add(DiscardedInitializer(owner, detail))
  }

  fun recordStrippedSupertype(owner: String, detail: String) {
    strippedSupertypes.add(DiscardedInitializer(owner, detail))
  }

  fun recordFailedRepair(owner: String, detail: String) {
    failedRepairs.add(DiscardedInitializer(owner, detail))
  }

  /**
   * Count of repairs the generator actually performed. [errorTypedApiMembers] and [failedRepairs]
   * are deliberately excluded: neither is a repair. An error-typed API member is left untouched by
   * design (degrading it would move the lie from the descriptor into the metadata), and a failed
   * repair is one that threw. Both are unresolved defects that [ValidationStage] reports on their
   * own; do not fold them in here.
   */
  val totalRepairs: Int
    get() =
        fabricatedConstants.size +
            clearedPropertyInitializers.size +
            replacedFieldInitializers.size +
            strippedSupertypes.size

  /**
   * Constants whose emitted type is known not to match the real declaration, or is only a guess.
   * These are the ones that corrupt a consumer's constant folding rather than merely degrading it.
   */
  fun unsoundConstants(): List<FabricatedConstant> = fabricatedConstants.filter {
    it.typeSource == ConstTypeSource.ASSUMED_STRING
  }

  /** Single machine-greppable line so the repair rate can be aggregated across a build. */
  fun counterLine(): String =
      "KOSABI_ABI_REPAIR_COUNTERS" +
          " fabricated_constants=${fabricatedConstants.size}" +
          " fabricated_constants_type_unknown=${unsoundConstants().size}" +
          " cleared_property_initializers=${clearedPropertyInitializers.size}" +
          " replaced_field_initializers=${replacedFieldInitializers.size}" +
          " stripped_supertypes=${strippedSupertypes.size}" +
          " error_typed_api_members=${errorTypedApiMembers.size}" +
          " failed_repairs=${failedRepairs.size}"
}
