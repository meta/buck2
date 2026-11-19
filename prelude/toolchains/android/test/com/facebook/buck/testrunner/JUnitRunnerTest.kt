/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.testrunner

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.Description
import org.junit.runner.Runner
import org.junit.runner.notification.RunNotifier
import org.junit.runners.ParentRunner

class JUnitRunnerTest {

  @Test
  fun returnsTrueWhenEveryNestedSuiteRunnerMatches() {
    val runner = CompositeRunner(TargetRunner(), CompositeRunner(TargetRunner(), TargetRunner()))

    assertTrue(
        JUnitRunner.isRunnerOfTypeOrSuiteOfType(
            runner,
            TargetRunner::class.java,
            CompositeRunner::class.java,
        ),
    )
  }

  @Test
  fun returnsFalseWhenAnyNestedSuiteRunnerDoesNotMatch() {
    val runner = CompositeRunner(TargetRunner(), CompositeRunner(TargetRunner(), OtherRunner()))

    assertFalse(
        JUnitRunner.isRunnerOfTypeOrSuiteOfType(
            runner,
            TargetRunner::class.java,
            CompositeRunner::class.java,
        ),
    )
  }

  @Test
  fun returnsFalseForEmptySuite() {
    assertFalse(
        JUnitRunner.isRunnerOfTypeOrSuiteOfType(
            CompositeRunner(),
            TargetRunner::class.java,
            CompositeRunner::class.java,
        ),
    )
  }

  private class CompositeRunner(vararg children: Runner) :
      ParentRunner<Runner>(CompositeRunner::class.java) {
    private val childRunners = children.toList()

    override fun getChildren(): List<Runner> = childRunners

    override fun describeChild(child: Runner): Description = child.description

    override fun runChild(child: Runner, notifier: RunNotifier) {
      child.run(notifier)
    }
  }

  private open class OtherRunner : Runner() {
    override fun getDescription(): Description = Description.EMPTY

    override fun run(notifier: RunNotifier) = Unit
  }

  private class TargetRunner : OtherRunner()
}
