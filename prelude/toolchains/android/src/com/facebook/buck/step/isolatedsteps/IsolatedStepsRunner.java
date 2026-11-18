/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.step.isolatedsteps;

import com.facebook.buck.core.build.execution.context.IsolatedExecutionContext;
import com.facebook.buck.core.util.log.Logger;
import com.facebook.buck.step.StepExecutionResult;
import com.facebook.buck.step.StepExecutionResults;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import java.io.IOException;

/** Step runner that executes the steps the given {@link IsolatedStep}s. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class IsolatedStepsRunner {

  private static final Logger LOG = Logger.get(IsolatedStepsRunner.class);

  private IsolatedStepsRunner() {}

  /**
   * Executes the given {@link IsolatedStep} instances with the given {@link
   * IsolatedExecutionContext} and throws {@link StepFailedException} if it occurred during the
   * execution of any steps.
   */
  public static StepExecutionResult execute(
      ImmutableList<IsolatedStep> steps, IsolatedExecutionContext executionContext)
      throws StepFailedException {
    try {
      for (IsolatedStep step : steps) {
        runStep(executionContext, step);
        rethrowIgnoredInterruptedException(step);
      }
      return StepExecutionResults.SUCCESS;
    } catch (InterruptedException e) {
      // Just return an error code.
    }
    return StepExecutionResults.ERROR;
  }

  /**
   * Executes the given {@link IsolatedStep} instances with the given {@link
   * IsolatedExecutionContext} and handles {@link StepFailedException} (log it with an error level).
   *
   * <p>The difference from this method and {@link #execute} is that this method logs and do not
   * propagate {@link StepFailedException} in case it occurred.
   */
  public static StepExecutionResult executeWithDefaultExceptionHandling(
      ImmutableList<IsolatedStep> steps, IsolatedExecutionContext executionContext) {
    try {
      return execute(steps, executionContext);
    } catch (StepFailedException e) {
      if (!executionContext.getVerbosity().isSilent()) {
        LOG.warn(e, "Failed to execute isolated steps");
      }
      return StepExecutionResult.of(e);
    }
  }

  private static void runStep(IsolatedExecutionContext context, IsolatedStep step)
      throws InterruptedException, StepFailedException {
    StepExecutionResult executionResult = StepExecutionResults.ERROR;
    try {
      executionResult = step.executeIsolatedStep(context);
    } catch (IOException | RuntimeException e) {
      throw StepFailedException.createForFailingStepWithException(
          step, descriptionForStep(step, context), e);
    }
    if (!executionResult.isSuccess()) {
      throw StepFailedException.createForFailingIsolatedStepWithExitCode(
          step, descriptionForStep(step, context), executionResult);
    }
  }

  private static String descriptionForStep(IsolatedStep step, IsolatedExecutionContext context) {
    return context.getVerbosity().shouldPrintCommand()
        ? step.getIsolatedStepDescription(context)
        : step.getShortName();
  }

  private static void rethrowIgnoredInterruptedException(IsolatedStep step)
      throws InterruptedException {
    // Check for interruptions that may have been ignored by step.
    if (Thread.interrupted()) {
      Thread.currentThread().interrupt();
      throw new InterruptedException(
          "Thread was interrupted inside the executed step: " + step.getShortName());
    }
  }
}
