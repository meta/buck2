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
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import org.jspecify.annotations.Nullable;

/**
 * Interface for adding the steps for compiling source to producing a JAR, for JVM languages.
 *
 * @param <T> Type of extra parameters needed to create these steps.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface CompileToJarStepFactory<T extends CompileToJarStepFactory.ExtraParams> {

  /**
   * Add the steps to {@code steps} to compile the sources (in {@code compilerParameters}) with java
   * compiler {@code resolvedJavac} for the build target {@code buildTargetValue} to a JAR, located
   * in {@code compilerOutputPathsValue}.
   *
   * <p>Language-specific parameters are passed through {@code extraParams}, which is instantiated
   * to a concrete type by implementations of this interface.
   */
  void createCompileToJarStep(
      RelPath buckOut,
      AbsPath buildCellRootPath,
      BuildTargetValue buildTargetValue,
      CompilerOutputPathsValue compilerOutputPathsValue,
      CompilerParameters compilerParameters,
      @Nullable JarParameters abiJarParameters,
      @Nullable JarParameters libraryJarParameters,
      ImmutableList.Builder<IsolatedStep> steps,
      ImmutableMap<RelPath, RelPath> resourcesMap,
      ResolvedJavac resolvedJavac,
      @Nullable ActionMetadata actionMetadata,
      T extraParams);

  /** Upcasts {@code extraParams} to the type of parameter expected by this factory. */
  T castExtraParams(ExtraParams extraParams);

  /** Extra params marker interface. */
  interface ExtraParams {}
}
