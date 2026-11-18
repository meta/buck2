/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin

import com.facebook.buck.jvm.kotlin.ksp.Ksp2NoOpDetector
import com.google.devtools.ksp.processing.CodeGenerator
import com.google.devtools.ksp.processing.Resolver
import com.google.devtools.ksp.processing.SymbolProcessor
import com.google.devtools.ksp.processing.SymbolProcessorEnvironment
import com.google.devtools.ksp.processing.SymbolProcessorProvider
import com.google.devtools.ksp.symbol.KSAnnotated
import java.io.File
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Test
import org.mockito.kotlin.mock
import org.mockito.kotlin.whenever

class Ksp2NoOpDetectorTest {

  /**
   * Stands in for `CodeGeneratorImpl`: `generatedFile` is rebuilt on each access and holds only
   * files whose output streams are still open, so [closeRound] models KSP emptying it at each round
   * boundary.
   */
  private class FakeCodeGenerator : CodeGenerator by mock<CodeGenerator>() {
    private val open = mutableListOf<File>()

    override val generatedFile: Collection<File>
      get() = open.toList()

    fun generate(path: String) {
      open.add(File(path))
    }

    fun closeRound() = open.clear()
  }

  private val codeGenerator = FakeCodeGenerator()
  private var now = 0L
  private val detector = Ksp2NoOpDetector { now }

  private fun advance(ms: Long) {
    now += TimeUnit.MILLISECONDS.toNanos(ms)
  }

  private val deferred = listOf<KSAnnotated>(mock())

  private inner class FakeProcessor(
      private val paths: List<String> = emptyList(),
      private val finishPaths: List<String> = emptyList(),
  ) : SymbolProcessor {
    override fun process(resolver: Resolver): List<KSAnnotated> {
      paths.forEach(codeGenerator::generate)
      return deferred
    }

    override fun finish() = finishPaths.forEach(codeGenerator::generate)
  }

  /**
   * Named provider classes rather than lambdas: counts are keyed by provider class name, and all
   * lambdas from one call site share a class. Real providers are distinct classes, one per
   * processor, which is what these model.
   */
  private inner class ProductiveProvider(private val paths: List<String>) :
      SymbolProcessorProvider {
    override fun create(environment: SymbolProcessorEnvironment) = FakeProcessor(paths)
  }

  private inner class IdleProvider : SymbolProcessorProvider {
    override fun create(environment: SymbolProcessorEnvironment) = FakeProcessor()
  }

  private inner class FinishOnlyProvider : SymbolProcessorProvider {
    override fun create(environment: SymbolProcessorEnvironment) =
        FakeProcessor(finishPaths = listOf("late.kt"))
  }

  /** Writes the final-round placeholder, as a KSP-run protocol trigger does, besides its output. */
  private inner class PlaceholderProvider(private val paths: List<String>) :
      SymbolProcessorProvider {
    override fun create(environment: SymbolProcessorEnvironment) =
        FakeProcessor(paths + "/out/ksp/java/com/facebook/Dummy.java")
  }

  /** A trigger that writes only the placeholder. */
  private inner class IdleTriggerProvider : SymbolProcessorProvider {
    override fun create(environment: SymbolProcessorEnvironment) =
        FakeProcessor(listOf("/out/ksp/java/com/facebook/Dummy.java"))
  }

  /** Takes the given time, on the fake clock, in `create`, in each `process` and in `finish`. */
  private inner class TimedProvider(
      private val createMs: Long,
      private val processMs: Long,
      private val finishMs: Long,
  ) : SymbolProcessorProvider {
    override fun create(environment: SymbolProcessorEnvironment): SymbolProcessor {
      advance(createMs)
      return object : SymbolProcessor {
        override fun process(resolver: Resolver): List<KSAnnotated> {
          advance(processMs)
          return emptyList()
        }

        override fun finish() = advance(finishMs)
      }
    }
  }

  private fun wrap(vararg providers: SymbolProcessorProvider): List<SymbolProcessor> {
    val environment: SymbolProcessorEnvironment = mock()
    whenever(environment.codeGenerator).thenReturn(codeGenerator)
    return detector.wrap(providers.toList()).map { it.create(environment) }
  }

  private fun countOf(provider: Class<*>) = detector.countsByProcessor.getValue(provider.name)

  /**
   * The reason each processor is wrapped rather than the shared generator read once: processors
   * share a generator, so one's output would otherwise be credited to another.
   */
  @Test
  fun `each processor is credited only with the files it generated`() {
    val (productive, idle) = wrap(ProductiveProvider(listOf("a.kt", "b.kt")), IdleProvider())

    productive.process(mock())
    idle.process(mock())

    assertEquals(2, countOf(ProductiveProvider::class.java))
    assertEquals(0, countOf(IdleProvider::class.java))
  }

  /** KSP closes output streams between rounds, emptying `generatedFile`. */
  @Test
  fun `counts accumulate across the round boundary that clears the generator`() {
    val processor = wrap(ProductiveProvider(listOf("a.kt", "b.kt"))).single()

    processor.process(mock())
    codeGenerator.closeRound()
    processor.process(mock())

    assertEquals(4, countOf(ProductiveProvider::class.java))
  }

  /**
   * KSP calls `finish()` after the last round, so a processor that generates only there has
   * generated output and must not read as a no-op.
   */
  @Test
  fun `files generated in finish are credited to their provider`() {
    val processor = wrap(FinishOnlyProvider()).single()

    processor.process(mock())
    processor.finish()

    assertEquals(mapOf(FinishOnlyProvider::class.java.name to 1), detector.countsByProcessor)
  }

  /** Dropping the deferred symbols would silently stop KSP reprocessing them. */
  @Test
  fun `deferred symbols are returned unchanged`() {
    val processor = wrap(ProductiveProvider(listOf("a.kt"))).single()

    assertSame(deferred, processor.process(mock()))
  }

  /** A provider whose processor never runs is still attached, so it reports zero, not absent. */
  @Test
  fun `provider whose processor never runs counts zero`() {
    wrap(ProductiveProvider(listOf("a.kt")))

    assertEquals(mapOf(ProductiveProvider::class.java.name to 0), detector.countsByProcessor)
  }

  /**
   * Protocol triggers write `com/facebook/Dummy.java` exactly when a round generated nothing, so
   * counting it would hide the no-op this detector reports.
   */
  @Test
  fun `the final-round placeholder is not counted`() {
    val (productive, idle) = wrap(PlaceholderProvider(listOf("a.kt")), IdleTriggerProvider())

    productive.process(mock())
    idle.process(mock())

    assertEquals(1, countOf(PlaceholderProvider::class.java))
    assertEquals(0, countOf(IdleTriggerProvider::class.java))
  }

  /** Only the exact placeholder path is skipped: a same-named file elsewhere is real output. */
  @Test
  fun `a Dummy file outside com facebook is counted`() {
    val processor =
        wrap(ProductiveProvider(listOf("/out/ksp/java/org/example/Dummy.java"))).single()

    processor.process(mock())

    assertEquals(1, countOf(ProductiveProvider::class.java))
  }

  /**
   * KSP runs processors one at a time, so each call's wall time is the processor's own, and the
   * step splits into the setup before the first `create`, the processors, and KSP's work around
   * them.
   */
  @Test
  fun `each processor is credited with its own time and the step splits into phases`() {
    detector.stepStarted()
    advance(100)
    val (timed, idle) =
        wrap(TimedProvider(createMs = 5, processMs = 30, finishMs = 2), IdleProvider())
    timed.process(mock())
    idle.process(mock())
    advance(10)
    timed.process(mock())
    timed.finish()
    detector.stepFinished()

    assertEquals(
        Ksp2NoOpDetector.ProcessorTime(createMs = 5, processMs = 60, finishMs = 2, rounds = 2),
        detector.timesByProcessor.getValue(TimedProvider::class.java.name),
    )
    assertEquals(
        Ksp2NoOpDetector.ProcessorTime(createMs = 0, processMs = 0, finishMs = 0, rounds = 1),
        detector.timesByProcessor.getValue(IdleProvider::class.java.name),
    )
    assertEquals(mapOf("setup" to 100L, "processors" to 67L, "other" to 10L), detector.phasesMs)
  }

  /** Phases need both step bounds; without them the split would be meaningless, so it is empty. */
  @Test
  fun `phases are empty when the step was not bracketed`() {
    wrap(TimedProvider(createMs = 5, processMs = 0, finishMs = 0))

    assertEquals(emptyMap<String, Long>(), detector.phasesMs)
  }
}
