/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Joiner;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import java.util.Map;
import java.util.Objects;
import java.util.function.BiPredicate;
import org.jspecify.annotations.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class CompilerPluginUtils {

  public static final String X_PLUGIN_ARG = "-Xplugin=";
  public static final String KOTLIN_PLUGIN_OUT_PLACEHOLDER = "__codegen_dir__";
  static final String MODULE_NAME = "-module-name";
  static final String NO_STDLIB = "-no-stdlib";
  static final String VERBOSE = "-verbose";
  static final String JB_JVM_ABI_OUTPUT_DIR = "plugin:org.jetbrains.kotlin.jvm.abi:outputDir";
  private static final String PLUGIN = "-P";
  private static final String PLUGIN_KOTLIN_DI_KSP_ACTIVE =
      "plugin:com.facebook.kotlin.di:kspActive";

  public static ImmutableList<String> getKotlinCompilerPluginsArgs(
      ImmutableMap<AbsPath, ImmutableMap<String, String>> resolvedKotlinCompilerPlugins,
      String outputDir,
      BiPredicate<AbsPath, ImmutableMap<String, String>> filter) {
    ImmutableList.Builder<String> pluginArgs = ImmutableList.builder();
    for (Map.Entry<AbsPath, ImmutableMap<String, String>> entry :
        resolvedKotlinCompilerPlugins.entrySet()) {
      AbsPath pluginPath = entry.getKey();
      ImmutableMap<String, String> pluginOptions = entry.getValue();

      if (!filter.test(pluginPath, pluginOptions)) {
        continue;
      }

      pluginArgs.addAll(getKotlinCompilerPluginsArgs(pluginPath, pluginOptions, outputDir));
    }
    return pluginArgs.build();
  }

  // I'll add unit tests to this here
  public static ImmutableList<String> getKotlinCompilerPluginsArgs(
      AbsPath pluginPath,
      @Nullable ImmutableMap<String, String> kotlinCompilerPluginParams,
      String outputDir) {
    ImmutableList.Builder<String> pluginArgs = ImmutableList.builder();

    // Add plugin basic string, e.g. "-Xplugins=<pluginPath>"
    pluginArgs.add(X_PLUGIN_ARG + pluginPath);

    // If plugin options exist, add plugin option string,
    // e.g. "-P" and "<optionKey>=<optionValue>,<optionKey2>=<optionValue2>,..."
    if (kotlinCompilerPluginParams != null && !kotlinCompilerPluginParams.isEmpty()) {
      ImmutableList.Builder<String> pluginOptionStrings = ImmutableList.builder();

      for (String pluginOptionKey : kotlinCompilerPluginParams.keySet()) {
        String pluginOptionValue =
            Objects.requireNonNull(kotlinCompilerPluginParams.get(pluginOptionKey));

        if (pluginOptionValue.equals(KOTLIN_PLUGIN_OUT_PLACEHOLDER)) {
          pluginOptionValue = outputDir;
        }

        pluginOptionStrings.add(pluginOptionKey + "=" + pluginOptionValue);
      }
      String pluginOptionString = Joiner.on(",").join(pluginOptionStrings.build());
      pluginArgs.add(PLUGIN).add(pluginOptionString);
    }
    return pluginArgs.build();
  }

  public static boolean isDiK1PluginForKapt(
      AbsPath sourcePath, ImmutableMap<String, String> options) {
    return isDiK1Plugin(sourcePath) && !isDiKspActive(options);
  }

  public static boolean isDiK1PluginForKsp(
      AbsPath sourcePath, ImmutableMap<String, String> options) {
    return isDiK1Plugin(sourcePath) && isDiKspActive(options);
  }

  private static boolean isDiK1Plugin(AbsPath sourcePath) {
    return sourcePath.endsWith("di.jar");
  }

  private static boolean isDiKspActive(ImmutableMap<String, String> options) {
    return "True".equals(options.get(PLUGIN_KOTLIN_DI_KSP_ACTIVE));
  }

  public static boolean isKotlinAllOpenPlugin(
      AbsPath sourcePath, ImmutableMap<String, String> options) {
    return sourcePath.endsWith("kotlin-allopen.jar");
  }
}
