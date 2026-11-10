/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util;

import com.facebook.buck.core.util.log.Logger;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.collect.ImmutableList;
import java.io.IOException;
import java.net.URL;
import java.net.URLClassLoader;
import java.util.HashMap;
import java.util.Map;
import java.util.Optional;
import org.jspecify.annotations.Nullable;

/**
 * Maintain a cache mapping class paths to class loaders that load from these class paths. The class
 * loaders remain active until ClassLoaderCache itself is unloaded.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class ClassLoaderCache implements AutoCloseable {
  private static final Logger LOG = Logger.get(ClassLoaderCache.class);

  private final Map<ClassLoader, Map<ImmutableList<URL>, ClassLoader>> cache = new HashMap<>();

  private int referenceCount = 1;

  private synchronized Map<ImmutableList<URL>, ClassLoader> getCacheForParent(
      @Nullable ClassLoader parentClassLoader) {
    Map<ImmutableList<URL>, ClassLoader> cacheForParent = cache.get(parentClassLoader);

    if (cacheForParent == null) {
      cacheForParent = new HashMap<>();
      cache.put(parentClassLoader, cacheForParent);
    }

    return cacheForParent;
  }

  public synchronized ClassLoader getClassLoaderForClassPath(
      @Nullable ClassLoader parentClassLoader, ImmutableList<URL> classPath) {

    Map<ImmutableList<URL>, ClassLoader> cacheForParent = getCacheForParent(parentClassLoader);

    ClassLoader classLoader = cacheForParent.get(classPath);
    if (classLoader == null) {
      LOG.info(
          "Encountered cache miss for parent: %s and classpath:%s", parentClassLoader, classPath);
      URL[] urls = classPath.toArray(new URL[0]);
      classLoader = new CachedURLClassLoader(urls, parentClassLoader);
      cacheForParent.put(classPath, classLoader);
    }

    return classLoader;
  }

  @VisibleForTesting
  public synchronized void injectClassLoader(
      @Nullable ClassLoader parentClassLoader,
      ImmutableList<URL> classPath,
      ClassLoader injectedClassLoader) {
    Map<ImmutableList<URL>, ClassLoader> cacheForParent = getCacheForParent(parentClassLoader);

    cacheForParent.put(classPath, injectedClassLoader);
  }

  public synchronized ClassLoaderCache addRef() {
    referenceCount += 1;
    return this;
  }

  @Override
  public synchronized void close() throws IOException {
    if (referenceCount > 1) {
      referenceCount -= 1;
      return;
    }

    Optional<IOException> caughtEx = Optional.empty();

    for (Map<ImmutableList<URL>, ClassLoader> cacheForParent : cache.values()) {
      for (ClassLoader cl : cacheForParent.values()) {
        try {
          if (cl instanceof CachedURLClassLoader) {
            ((CachedURLClassLoader) cl).reallyClose();
          }
        } catch (IOException ex) {
          if (caughtEx.isPresent()) {
            caughtEx.get().addSuppressed(ex);
          } else {
            caughtEx = Optional.of(ex);
          }
        }
      }
    }

    if (caughtEx.isPresent()) {
      throw caughtEx.get();
    }
  }

  private static class CachedURLClassLoader extends URLClassLoader {
    public CachedURLClassLoader(URL[] urls, @Nullable ClassLoader parent) {
      super(urls, parent);
    }

    @Override
    public void close() {
      // Do nothing; only the cache can close this ClassLoader
    }

    private void reallyClose() throws IOException {
      super.close();
    }
  }
}
