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
import java.net.URISyntaxException;
import java.net.URL;
import java.net.URLClassLoader;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.attribute.BasicFileAttributes;
import java.nio.file.attribute.FileTime;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.TimeUnit;
import org.jspecify.annotations.Nullable;

/**
 * Maintain a cache mapping class paths to class loaders that load from these class paths. The class
 * loaders remain active until ClassLoaderCache itself is unloaded.
 *
 * <p>A cached class loader is reused only while every file on its class path is unchanged: a worker
 * can outlive a build, and the next build can replace a jar at the same path.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class ClassLoaderCache implements AutoCloseable {
  private static final Logger LOG = Logger.get(ClassLoaderCache.class);

  private final Map<ClassLoader, Map<ImmutableList<URL>, Entry>> cache = new HashMap<>();

  /** Class loaders replaced because a file on their class path changed. Closed with the cache. */
  private final List<ClassLoader> replaced = new ArrayList<>();

  private int referenceCount = 1;

  private synchronized Map<ImmutableList<URL>, Entry> getCacheForParent(
      @Nullable ClassLoader parentClassLoader) {
    Map<ImmutableList<URL>, Entry> cacheForParent = cache.get(parentClassLoader);

    if (cacheForParent == null) {
      cacheForParent = new HashMap<>();
      cache.put(parentClassLoader, cacheForParent);
    }

    return cacheForParent;
  }

  public synchronized ClassLoader getClassLoaderForClassPath(
      @Nullable ClassLoader parentClassLoader, ImmutableList<URL> classPath) {

    Map<ImmutableList<URL>, Entry> cacheForParent = getCacheForParent(parentClassLoader);
    ImmutableList<FileStamp> stamps = FileStamp.of(classPath);

    Entry entry = cacheForParent.get(classPath);
    if (entry != null && entry.stamps.equals(stamps)) {
      return entry.classLoader;
    }
    if (entry == null) {
      LOG.info(
          "Encountered cache miss for parent: %s and classpath:%s", parentClassLoader, classPath);
    } else {
      LOG.info("A file changed on classpath:%s; creating a new class loader", classPath);
      // Not closed here: a compilation still running may be loading classes from it.
      replaced.add(entry.classLoader);
    }
    URL[] urls = classPath.toArray(new URL[0]);
    ClassLoader classLoader = new CachedURLClassLoader(urls, parentClassLoader);
    cacheForParent.put(classPath, new Entry(classLoader, stamps));
    return classLoader;
  }

  @VisibleForTesting
  public synchronized void injectClassLoader(
      @Nullable ClassLoader parentClassLoader,
      ImmutableList<URL> classPath,
      ClassLoader injectedClassLoader) {
    Map<ImmutableList<URL>, Entry> cacheForParent = getCacheForParent(parentClassLoader);

    cacheForParent.put(classPath, new Entry(injectedClassLoader, FileStamp.of(classPath)));
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

    List<ClassLoader> toClose = new ArrayList<>(replaced);
    for (Map<ImmutableList<URL>, Entry> cacheForParent : cache.values()) {
      for (Entry entry : cacheForParent.values()) {
        toClose.add(entry.classLoader);
      }
    }

    Optional<IOException> caughtEx = Optional.empty();

    for (ClassLoader cl : toClose) {
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

    if (caughtEx.isPresent()) {
      throw caughtEx.get();
    }
  }

  private static final class Entry {
    private final ClassLoader classLoader;
    private final ImmutableList<FileStamp> stamps;

    private Entry(ClassLoader classLoader, ImmutableList<FileStamp> stamps) {
      this.classLoader = classLoader;
      this.stamps = stamps;
    }
  }

  /**
   * What identifies the content of a regular file on a class path without reading it: its file key
   * (device and inode on Unix), size and modification time. Directories and non-file URLs get an
   * empty stamp, so changes inside them are not detected.
   */
  private static final class FileStamp {
    private static final FileStamp UNTRACKED = new FileStamp(null, -1, -1);

    @Nullable private final Object fileKey;
    private final long size;
    private final long modifiedNanos;

    private FileStamp(@Nullable Object fileKey, long size, long modifiedNanos) {
      this.fileKey = fileKey;
      this.size = size;
      this.modifiedNanos = modifiedNanos;
    }

    private static ImmutableList<FileStamp> of(ImmutableList<URL> classPath) {
      ImmutableList.Builder<FileStamp> stamps =
          ImmutableList.builderWithExpectedSize(classPath.size());
      for (URL url : classPath) {
        stamps.add(of(url));
      }
      return stamps.build();
    }

    private static FileStamp of(URL url) {
      if (!"file".equals(url.getProtocol())) {
        return UNTRACKED;
      }
      try {
        Path path = Paths.get(url.toURI());
        BasicFileAttributes attributes = Files.readAttributes(path, BasicFileAttributes.class);
        if (!attributes.isRegularFile()) {
          return UNTRACKED;
        }
        FileTime lastModifiedTime = attributes.lastModifiedTime();
        if (lastModifiedTime == null) {
          return UNTRACKED;
        }
        return new FileStamp(
            attributes.fileKey(), attributes.size(), lastModifiedTime.to(TimeUnit.NANOSECONDS));
      } catch (IOException | URISyntaxException | IllegalArgumentException e) {
        // A missing or unreadable file: the class loader fails as it would have without the cache.
        return UNTRACKED;
      }
    }

    @Override
    public boolean equals(@Nullable Object other) {
      if (!(other instanceof FileStamp)) {
        return false;
      }
      FileStamp that = (FileStamp) other;
      return size == that.size
          && modifiedNanos == that.modifiedNanos
          && Objects.equals(fileKey, that.fileKey);
    }

    @Override
    public int hashCode() {
      return Objects.hash(fileKey, size, modifiedNanos);
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
