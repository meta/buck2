/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.ksp

import com.google.devtools.ksp.processing.CodeGenerator
import com.google.devtools.ksp.processing.Resolver
import com.google.devtools.ksp.processing.SymbolProcessor
import com.google.devtools.ksp.processing.SymbolProcessorProvider
import com.google.devtools.ksp.symbol.KSAnnotated
import java.io.File
import java.util.concurrent.TimeUnit

/**
 * Counts the files each KSP processor generates, so that a processor attached to a target but
 * generating nothing for it becomes visible.
 *
 * `CodeGenerator.generatedFile` is shared by every processor in an invocation and holds only files
 * whose output streams are still open, which KSP closes at each round boundary. Output is therefore
 * attributable only as a delta taken around each call and summed - exact because KSP runs
 * processors sequentially: `process()` per round, then `finish()` once after the last round.
 *
 * The final-round placeholder `com/facebook/Dummy.java` is not counted. The final-round triggers of
 * the shared KSP-run protocol (`//fbandroid/java/com/facebook/kotlin/ksp/runprotocol`) write it to
 * force a final processing round, precisely when a round generated nothing, so counting it would
 * hide the very no-op this class reports. `KspStepsBuilder` strips the same file from the staged
 * sources.
 *
 * Zero is evidence of a no-op only under non-incremental processing: a processor with nothing to do
 * in an incremental round legitimately generates nothing.
 *
 * The same wrapping times each processor's `create`, `process` and `finish` calls. KSP runs them
 * one at a time on one thread, so the per-processor times, KSP's own work between them, and the
 * setup before the first `create` add up to the step: [phasesMs] splits the step that way.
 */
class Ksp2NoOpDetector(private val nanoTime: () -> Long = System::nanoTime) {

  /** Wall time one provider's processor spent in each call, and how many rounds it processed. */
  data class ProcessorTime(
      val createMs: Long,
      val processMs: Long,
      val finishMs: Long,
      val rounds: Int,
  )

  private val generatedCounts = mutableMapOf<String, Int>()
  private val createNanos = mutableMapOf<String, Long>()
  private val processNanos = mutableMapOf<String, Long>()
  private val finishNanos = mutableMapOf<String, Long>()
  private val rounds = mutableMapOf<String, Int>()

  private var stepStartNanos: Long? = null
  private var stepEndNanos: Long? = null
  private var firstCreateNanos: Long? = null

  /** Files generated per provider class name. A provider whose processor never ran is 0. */
  val countsByProcessor: Map<String, Int>
    get() = generatedCounts.toMap()

  /** Time per provider class name, in the order the providers were attached. */
  val timesByProcessor: Map<String, ProcessorTime>
    get() =
        generatedCounts.keys.associateWith { name ->
          ProcessorTime(
              createMs = toMillis(createNanos[name]),
              processMs = toMillis(processNanos[name]),
              finishMs = toMillis(finishNanos[name]),
              rounds = rounds[name] ?: 0,
          )
        }

  /**
   * The step split into `setup` (step start to the first processor's `create`: loading the
   * processors, KSP's analysis session and source parsing), `processors` (every `create`, `process`
   * and `finish` call) and `other` (the rest: KSP's work between and after rounds). Empty unless
   * [stepStarted] and [stepFinished] bracketed the step and a processor was created.
   */
  val phasesMs: Map<String, Long>
    get() {
      val start = stepStartNanos ?: return emptyMap()
      val end = stepEndNanos ?: return emptyMap()
      val firstCreate = firstCreateNanos ?: return emptyMap()
      val setup = firstCreate - start
      val processors =
          createNanos.values.sum() + processNanos.values.sum() + finishNanos.values.sum()
      return linkedMapOf(
          "setup" to toMillis(setup),
          "processors" to toMillis(processors),
          "other" to toMillis((end - start - setup - processors).coerceAtLeast(0)),
      )
    }

  fun stepStarted() {
    stepStartNanos = nanoTime()
  }

  fun stepFinished() {
    stepEndNanos = nanoTime()
  }

  fun wrap(providers: List<SymbolProcessorProvider>): List<SymbolProcessorProvider> =
      providers.map { provider ->
        SymbolProcessorProvider { environment ->
          val name = provider::class.java.name
          val start = nanoTime()
          if (firstCreateNanos == null) {
            firstCreateNanos = start
          }
          val processor = provider.create(environment)
          createNanos.merge(name, nanoTime() - start, Long::plus)
          // Seed the entry so a provider whose processor never generates is distinguishable from
          // one that was never attached. That distinction is the point of this class.
          generatedCounts.putIfAbsent(name, 0)
          Counting(processor, name, environment.codeGenerator)
        }
      }

  private inner class Counting(
      private val delegate: SymbolProcessor,
      private val name: String,
      private val codeGenerator: CodeGenerator,
  ) : SymbolProcessor by delegate {

    override fun process(resolver: Resolver): List<KSAnnotated> {
      rounds.merge(name, 1, Int::plus)
      return counted(processNanos) { delegate.process(resolver) }
    }

    override fun finish() {
      counted(finishNanos) { delegate.finish() }
    }

    private fun <T> counted(nanos: MutableMap<String, Long>, call: () -> T): T {
      val before = generatedCount()
      val start = nanoTime()
      val result = call()
      nanos.merge(name, nanoTime() - start, Long::plus)
      generatedCounts[name] = generatedCounts.getValue(name) + (generatedCount() - before)
      return result
    }

    private fun generatedCount(): Int =
        codeGenerator.generatedFile.count { !isFinalRoundPlaceholder(it) }
  }

  private companion object {
    /** Relative to KSP's Java output directory; the path `KspStepsBuilder` removes. */
    const val FINAL_ROUND_PLACEHOLDER = "com/facebook/Dummy.java"

    fun toMillis(nanos: Long?): Long = TimeUnit.NANOSECONDS.toMillis(nanos ?: 0L)

    fun isFinalRoundPlaceholder(file: File): Boolean =
        file.invariantSeparatorsPath.endsWith("/$FINAL_ROUND_PLACEHOLDER")
  }
}
