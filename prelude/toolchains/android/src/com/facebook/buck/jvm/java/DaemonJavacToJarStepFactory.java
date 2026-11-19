/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.core.util.log.Logger;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.jvm.java.abi.AbiGenerationModeUtils;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Preconditions;
import com.google.common.collect.ImmutableList;
import org.jspecify.annotations.Nullable;

/**
 * Java implementation of compile to jar steps factory that doesn't depend on an internal build
 * graph datastructures, intended to be used from the Daemon worker.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class DaemonJavacToJarStepFactory extends BaseJavacToJarStepFactory {

  private static final Logger LOG = Logger.get(DaemonJavacToJarStepFactory.class);

  public DaemonJavacToJarStepFactory() {}

  @Override
  public void createCompileToJarStepImpl(
      RelPath buckOut,
      AbsPath buildCellRootPath,
      BuildTargetValue invokingRule,
      CompilerOutputPathsValue compilerOutputPathsValue,
      CompilerParameters compilerParameters,
      @Nullable JarParameters abiJarParameters,
      @Nullable JarParameters libraryJarParameters,
      ImmutableList.Builder<IsolatedStep> steps,
      ResolvedJavac resolvedJavac,
      @Nullable ActionMetadata actionMetadata,
      JavaExtraParams extraParams) {
    Preconditions.checkArgument(
        libraryJarParameters == null
            || libraryJarParameters
                .getEntriesToJar()
                .contains(compilerParameters.getOutputPaths().getClassesDir()));

    // In order to use direct spooling to the Jar:
    // (1) It must be enabled through a .buckconfig.
    // (2) The target must have 0 postprocessing steps.
    // (3) Tha compile API must be JSR 199.
    boolean isSpoolingToJarEnabled =
        AbiGenerationModeUtils.isNotClassAbi(compilerParameters.getAbiGenerationMode())
            || resolvedJavac instanceof Jsr199Javac.ResolvedJsr199Javac;

    if (isSpoolingToJarEnabled) {
      steps.add(
          new JavacStep(
              resolvedJavac,
              extraParams.getResolvedJavacOptions(),
              invokingRule,
              buckOut,
              compilerOutputPathsValue,
              compilerParameters,
              // javac writes libraryJarParameters here, so it must run even with nothing to compile
              /* skipIfNoCompilationUnits */ false,
              abiJarParameters,
              libraryJarParameters,
              false));
    } else {
      super.createCompileToJarStepImpl(
          buckOut,
          buildCellRootPath,
          invokingRule,
          compilerOutputPathsValue,
          compilerParameters,
          null,
          libraryJarParameters,
          steps,
          resolvedJavac,
          actionMetadata,
          extraParams);
    }
  }

  @Override
  public boolean supportsCompilationDaemon() {
    return true;
  }
}
