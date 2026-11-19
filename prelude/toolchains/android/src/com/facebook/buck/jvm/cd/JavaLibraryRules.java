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

import com.facebook.buck.cd.model.java.AbiGenerationMode;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.java.CompilerOutputPaths;
import com.facebook.buck.jvm.java.CompilerParameters;
import com.facebook.buck.jvm.java.DefaultSourceOnlyAbiRuleInfoFactory;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSortedSet;

/** Common utilities. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class JavaLibraryRules {

  /** Utility class: do not instantiate. */
  private JavaLibraryRules() {}

  /** Creates {@link CompilerParameters} */
  public static CompilerParameters getCompilerParameters(
      ImmutableList<RelPath> compileTimeClasspathPaths,
      ImmutableList<RelPath> compileTimeClasspathSnapshotPaths,
      ImmutableSortedSet<RelPath> javaSrcs,
      String fullyQualifiedBuildTargetName,
      boolean trackClassUsage,
      AbiGenerationMode abiGenerationMode,
      AbiGenerationMode abiCompatibilityMode,
      boolean isRequiredForSourceOnlyAbi,
      CompilerOutputPaths compilerOutputPaths) {
    return new CompilerParameters(
        javaSrcs,
        compileTimeClasspathPaths,
        compileTimeClasspathSnapshotPaths,
        compilerOutputPaths,
        abiGenerationMode,
        abiCompatibilityMode,
        trackClassUsage,
        new DefaultSourceOnlyAbiRuleInfoFactory(
            fullyQualifiedBuildTargetName, isRequiredForSourceOnlyAbi));
  }
}
