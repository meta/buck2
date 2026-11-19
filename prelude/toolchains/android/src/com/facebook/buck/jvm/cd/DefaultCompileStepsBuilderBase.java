/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd;

import com.facebook.buck.jvm.java.CompileToJarStepFactory;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;

/** Creates a list of {@link IsolatedStep} that is ready for in process execute. */
@Nullsafe(Nullsafe.Mode.LOCAL)
abstract class DefaultCompileStepsBuilderBase<T extends CompileToJarStepFactory.ExtraParams>
    implements CompileStepsBuilder {

  protected final ImmutableList.Builder<IsolatedStep> stepsBuilder = ImmutableList.builder();
  protected final CompileToJarStepFactory<T> configuredCompiler;

  DefaultCompileStepsBuilderBase(CompileToJarStepFactory<T> configuredCompiler) {
    this.configuredCompiler = configuredCompiler;
  }

  @Override
  public final ImmutableList<IsolatedStep> buildIsolatedSteps() {
    return stepsBuilder.build();
  }
}
