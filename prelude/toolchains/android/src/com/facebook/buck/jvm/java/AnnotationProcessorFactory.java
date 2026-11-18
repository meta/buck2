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
import com.facebook.buck.util.ClassLoaderCache;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.collect.ImmutableList;
import java.io.IOException;
import java.util.List;
import javax.annotation.processing.Processor;

@Nullsafe(Nullsafe.Mode.LOCAL)
class AnnotationProcessorFactory implements AutoCloseable {
  private final ClassLoader compilerClassLoader;
  private final ClassLoaderCache globalClassLoaderCache;
  private final ClassLoaderCache localClassLoaderCache = new ClassLoaderCache();
  private final String buildTargetName;

  AnnotationProcessorFactory(
      ClassLoader compilerClassLoader,
      ClassLoaderCache globalClassLoaderCache,
      String buildTargetName) {
    this.compilerClassLoader = compilerClassLoader;
    this.globalClassLoaderCache = globalClassLoaderCache;
    this.buildTargetName = buildTargetName;
  }

  @Override
  public void close() throws IOException {
    localClassLoaderCache.close();
  }

  public List<Processor> createProcessors(
      JavacExecutionContext context, JavacPluginParams processorParams) {
    ImmutableList.Builder<Processor> builder = ImmutableList.builder();
    for (ResolvedJavacPluginProperties plugin : processorParams.getPluginProperties()) {
      ClassLoader classLoader = getClassLoaderForProcessorGroup(plugin, context.getRuleCellRoot());
      plugin.getProcessorNames().stream()
          .map(name -> createProcessor(classLoader, name))
          .forEach((v) -> builder.add(v));
    }
    return builder.build();
  }

  private Processor createProcessor(ClassLoader classLoader, String name) {
    try {
      Class<? extends Processor> aClass = classLoader.loadClass(name).asSubclass(Processor.class);
      return aClass.newInstance();
    } catch (ReflectiveOperationException e) {
      // If this happens, then the build is really in trouble. Better warn the user.
      throw new RuntimeException(
          String.format("%s: javac unable to load annotation processor: %s", buildTargetName, name),
          e);
    }
  }

  @VisibleForTesting
  ClassLoader getClassLoaderForProcessorGroup(
      ResolvedJavacPluginProperties pluginParams, AbsPath relPathRoot) {
    ClassLoaderCache cache;
    // We can avoid lots of overhead in large builds by reusing the same classloader for java
    // plugins. However, some plugins use static variables in a way that assumes
    // there is only one instance running in the process at a time (or at all), and such plugin
    // would break running inside of Buck. So we default to creating a new ClassLoader
    // if any plugins meets those requirements.
    if (pluginParams.getCanReuseClassLoader()) {
      cache = globalClassLoaderCache;
    } else {
      cache = localClassLoaderCache;
    }
    return cache.getClassLoaderForClassPath(
        compilerClassLoader, pluginParams.toUrlClasspath(relPathRoot));
  }
}
