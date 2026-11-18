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
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSortedSet;
import java.io.File;
import java.net.MalformedURLException;
import java.net.URI;
import java.net.URISyntaxException;
import java.net.URL;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.stream.Collectors;

/** Resolved JavacPluginProperties used in {@link JavacPipelineState} */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ResolvedJavacPluginProperties {

  private final boolean canReuseClassLoader;
  private final boolean doesNotAffectAbi;
  private final boolean supportsAbiGenerationFromSource;
  private final boolean runsOnJavaOnly;
  private final ImmutableSortedSet<String> processorNames;

  private final ImmutableList<String> arguments;

  private final ImmutableList<RelPath> classpath;

  private final ImmutableMap<String, RelPath> pathParams;

  public ResolvedJavacPluginProperties(
      boolean canReuseClassLoader,
      boolean doesNotAffectAbi,
      boolean supportsAbiGenerationFromSource,
      boolean runsOnJavaOnly,
      ImmutableSortedSet<String> processorNames,
      ImmutableList<RelPath> classpath,
      ImmutableMap<String, RelPath> pathParams,
      ImmutableList<String> arguments) {
    this.canReuseClassLoader = canReuseClassLoader;
    this.doesNotAffectAbi = doesNotAffectAbi;
    this.supportsAbiGenerationFromSource = supportsAbiGenerationFromSource;
    this.runsOnJavaOnly = runsOnJavaOnly;
    this.processorNames = processorNames;
    this.classpath = classpath;
    this.pathParams = pathParams;
    this.arguments = arguments;
  }

  public boolean getCanReuseClassLoader() {
    return canReuseClassLoader;
  }

  public boolean getDoesNotAffectAbi() {
    return doesNotAffectAbi;
  }

  public boolean getSupportAbiGenerationFromSource() {
    return supportsAbiGenerationFromSource;
  }

  public boolean getRunsOnJavaOnly() {
    return runsOnJavaOnly;
  }

  public ImmutableSortedSet<String> getProcessorNames() {
    return processorNames;
  }

  /** Get the classpath for the plugin. */
  public ImmutableList<RelPath> getClasspath() {
    return classpath;
  }

  public ImmutableMap<String, RelPath> getPathParams() {
    return pathParams;
  }

  public ImmutableList<String> getArguments() {
    return arguments;
  }

  public static String getJoinedClasspath(
      ImmutableList<ResolvedJavacPluginProperties> resolvedProperties, AbsPath root) {
    return resolvedProperties.stream()
        .flatMap(p -> p.classpath.stream())
        .distinct()
        .map(root::resolve)
        .map(AbsPath::normalize)
        .map(ResolvedJavacPluginProperties::toUrl)
        .map(ResolvedJavacPluginProperties::toURI)
        .map(Paths::get)
        .map(Path::toString)
        .collect(Collectors.joining(File.pathSeparator));
  }

  static URI toURI(URL url) {
    try {
      return url.toURI();
    } catch (URISyntaxException e) {
      throw new RuntimeException(e);
    }
  }

  static URL toUrl(AbsPath absPath) {
    try {
      return absPath.toUri().toURL();
    } catch (MalformedURLException e) {
      // The paths we're being given should have all been resolved from the file system already.
      // We'd need to be unfortunate to get here. Bubble up a runtime exception.
      throw new RuntimeException(e);
    }
  }

  /** Resolves classpath to a list of URL */
  public ImmutableList<URL> toUrlClasspath(AbsPath relPathRoot) {
    ImmutableList.Builder<URL> builder = ImmutableList.builder();
    for (RelPath relPath : getClasspath()) {
      AbsPath path = relPathRoot.resolve(relPath).normalize();
      builder.add(toUrl(path));
    }
    return builder.build();
  }
}
