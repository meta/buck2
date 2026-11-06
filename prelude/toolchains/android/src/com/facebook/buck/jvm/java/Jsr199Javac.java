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

import com.facebook.buck.cd.model.java.AbiGenerationMode;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.jvm.java.abi.source.api.SourceOnlyAbiRuleInfoFactory;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Joiner;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSortedSet;
import javax.annotation.Nullable;
import javax.tools.JavaCompiler;

/** Command used to compile java libraries with a variety of ways to handle dependencies. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public abstract class Jsr199Javac {

  /** Base class for a resolved javac in-process tool. */
  public abstract static class ResolvedJsr199Javac implements ResolvedJavac {

    protected abstract JavaCompiler createCompiler(JavacExecutionContext context);

    @Override
    public String getDescription(ImmutableList<String> options, RelPath pathToSrcsList) {
      StringBuilder builder = new StringBuilder("javac ");
      Joiner.on(" ").appendTo(builder, options);
      builder.append(" ");
      builder.append("@").append(pathToSrcsList);

      return builder.toString();
    }

    @Override
    public String getShortName() {
      return "javac";
    }

    @Override
    public ResolvedJavac.Invocation newBuildInvocation(
        JavacExecutionContext context,
        BuildTargetValue invokingRule,
        CompilerOutputPathsValue compilerOutputPathsValue,
        ImmutableList<String> options,
        JavacPluginParams annotationProcessorParams,
        JavacPluginParams pluginParams,
        ImmutableSortedSet<RelPath> javaSourceFilePaths,
        RelPath pathToSrcsList,
        RelPath workingDirectory,
        boolean trackClassUsage,
        @Nullable JarParameters abiJarParameters,
        @Nullable JarParameters libraryJarParameters,
        AbiGenerationMode abiGenerationMode,
        AbiGenerationMode abiCompatibilityMode,
        @Nullable SourceOnlyAbiRuleInfoFactory ruleInfoFactory) {
      return new Jsr199JavacInvocation(
          () -> createCompiler(context),
          context,
          invokingRule,
          compilerOutputPathsValue,
          options,
          annotationProcessorParams,
          pluginParams,
          javaSourceFilePaths,
          pathToSrcsList,
          trackClassUsage,
          abiJarParameters,
          libraryJarParameters,
          abiGenerationMode,
          abiCompatibilityMode,
          ruleInfoFactory);
    }
  }
}
