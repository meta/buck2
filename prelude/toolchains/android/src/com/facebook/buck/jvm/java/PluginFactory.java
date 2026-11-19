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

// Counter part of AnnotationProcessorFactory
@Nullsafe(Nullsafe.Mode.LOCAL)
public class PluginFactory implements AutoCloseable {

  private final ClassLoader compilerClassLoader;
  private final ClassLoaderCache globalClassLoaderCache;
  private final ClassLoaderCache localClassLoaderCache = new ClassLoaderCache();

  PluginFactory(ClassLoader compilerClassLoader, ClassLoaderCache globalClassLoaderCache) {
    this.compilerClassLoader = compilerClassLoader;
    this.globalClassLoaderCache = globalClassLoaderCache;
  }

  @Override
  public void close() throws Exception {
    localClassLoaderCache.close();
  }

  ClassLoader getClassLoaderForProcessorGroups(
      JavacPluginParams pluginParams, AbsPath relPathRoot) {
    ClassLoaderCache cache;
    // We can avoid lots of overhead in large builds by reusing the same classloader for java
    // plugins. However, some plugins use static variables in a way that assumes
    // there is only one instance running in the process at a time (or at all), and such plugin
    // would break running inside of Buck. So we default to creating a new ClassLoader
    // if any plugins meets those requirements.
    if (pluginParams.getPluginProperties().stream()
        .allMatch(ResolvedJavacPluginProperties::getCanReuseClassLoader)) {
      cache = globalClassLoaderCache;
    } else {
      cache = localClassLoaderCache;
    }
    return cache.getClassLoaderForClassPath(
        compilerClassLoader, pluginParams.toUrlClasspath(relPathRoot));
  }
}
