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

import static org.hamcrest.MatcherAssert.assertThat;
import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNotSame;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;

import com.google.common.collect.FluentIterable;
import com.google.common.collect.ImmutableList;
import java.io.File;
import java.io.IOException;
import java.net.URL;
import java.net.URLClassLoader;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.nio.file.attribute.FileTime;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;
import org.hamcrest.Matchers;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class ClassLoaderCacheTest {
  @Rule public TemporaryFolder tempFolder = new TemporaryFolder();
  private File fooDir;
  private File barDir;
  private File bazFile;

  @Before
  public void setupTempFolder() throws IOException {
    fooDir = tempFolder.newFolder("foo");
    barDir = tempFolder.newFolder("bar");
    bazFile = new File(barDir, "baz");
    bazFile.createNewFile();
  }

  @Test
  public void cacheLoaderReturnsSameClassLoader() throws Exception {
    try (ClassLoaderCache clc = new ClassLoaderCache()) {
      ClassLoader dummyParent = ClassLoader.getSystemClassLoader();
      ImmutableList<URL> dummyClassPath =
          ImmutableList.of(fooDir.toURI().toURL(), barDir.toURI().toURL());
      ClassLoader cl1 = clc.getClassLoaderForClassPath(dummyParent, dummyClassPath);
      ClassLoader cl2 = clc.getClassLoaderForClassPath(dummyParent, dummyClassPath);

      // The class loader had better have been cached
      assertSame(cl1, cl2);

      // And the class loader should contain the URLs we supplied
      URL[] dummyUrls = FluentIterable.from(dummyClassPath).toArray(URL.class);

      assertArrayEquals(dummyUrls, ((URLClassLoader) cl1).getURLs());
    }
  }

  @Test
  public void cacheLoaderClosesClassLoaders() throws Exception {
    ClassLoader cl;
    try (ClassLoaderCache clc = new ClassLoaderCache()) {
      ClassLoader dummyParent = ClassLoader.getSystemClassLoader();
      ImmutableList<URL> dummyClassPath =
          ImmutableList.of(fooDir.toURI().toURL(), barDir.toURI().toURL());
      cl = clc.getClassLoaderForClassPath(dummyParent, dummyClassPath);

      assertThat(cl.getResource("baz"), Matchers.equalTo(bazFile.toURI().toURL()));
    }

    // When the class loader is closed, resources are no longer accessible
    assertNull(cl.getResource("baz"));
  }

  @Test
  public void callersCannotCloseCachedClassLoaders() throws Exception {
    URLClassLoader cl;
    try (ClassLoaderCache clc = new ClassLoaderCache()) {
      ClassLoader dummyParent = ClassLoader.getSystemClassLoader();
      ImmutableList<URL> dummyClassPath =
          ImmutableList.of(fooDir.toURI().toURL(), barDir.toURI().toURL());
      cl = (URLClassLoader) clc.getClassLoaderForClassPath(dummyParent, dummyClassPath);

      assertThat(cl.getResource("baz"), Matchers.equalTo(bazFile.toURI().toURL()));
      cl.close();

      // Because the class loader isn't closed, we can still get at the resource
      assertThat(cl.getResource("baz"), Matchers.equalTo(bazFile.toURI().toURL()));
    }
  }

  private static void writeJar(Path jar, String entryName) throws IOException {
    try (ZipOutputStream zip = new ZipOutputStream(Files.newOutputStream(jar))) {
      zip.putNextEntry(new ZipEntry(entryName));
      zip.closeEntry();
    }
  }

  /**
   * Replaces {@code jar} the way a build replaces an output: a new file renamed over the old one.
   */
  private void replaceJar(Path jar, String entryName) throws IOException {
    Path replacement = tempFolder.getRoot().toPath().resolve(jar.getFileName() + ".tmp");
    writeJar(replacement, entryName);
    Files.move(replacement, jar, StandardCopyOption.REPLACE_EXISTING);
  }

  @Test
  public void cacheLoaderReplacesClassLoaderWhenAJarIsReplaced() throws Exception {
    Path jar = tempFolder.getRoot().toPath().resolve("lib.jar");
    writeJar(jar, "first");
    try (ClassLoaderCache clc = new ClassLoaderCache()) {
      ClassLoader parent = ClassLoader.getSystemClassLoader();
      ImmutableList<URL> classPath = ImmutableList.of(jar.toUri().toURL());
      ClassLoader before = clc.getClassLoaderForClassPath(parent, classPath);
      assertSame(before, clc.getClassLoaderForClassPath(parent, classPath));

      replaceJar(jar, "second");

      ClassLoader after = clc.getClassLoaderForClassPath(parent, classPath);
      assertNotSame("a changed jar gets a new class loader", before, after);
      assertNotNull("the new class loader reads the new jar", after.getResource("second"));
      assertSame(after, clc.getClassLoaderForClassPath(parent, classPath));
    }
  }

  @Test
  public void cacheLoaderReplacesClassLoaderWhenAJarIsRewrittenInPlace() throws Exception {
    Path jar = tempFolder.getRoot().toPath().resolve("lib.jar");
    writeJar(jar, "first");
    try (ClassLoaderCache clc = new ClassLoaderCache()) {
      ClassLoader parent = ClassLoader.getSystemClassLoader();
      ImmutableList<URL> classPath = ImmutableList.of(jar.toUri().toURL());
      ClassLoader before = clc.getClassLoaderForClassPath(parent, classPath);
      FileTime modified = Files.getLastModifiedTime(jar);

      // Same file and size, rewritten with a later modification time.
      writeJar(jar, "other");
      Files.setLastModifiedTime(jar, FileTime.fromMillis(modified.toMillis() + 1000));

      assertNotSame(before, clc.getClassLoaderForClassPath(parent, classPath));
    }
  }

  @Test
  public void replacedClassLoaderStaysOpenUntilTheCacheCloses() throws Exception {
    Path jar = tempFolder.getRoot().toPath().resolve("lib.jar");
    writeJar(jar, "first");
    ClassLoader before;
    try (ClassLoaderCache clc = new ClassLoaderCache()) {
      ClassLoader parent = ClassLoader.getSystemClassLoader();
      ImmutableList<URL> classPath = ImmutableList.of(jar.toUri().toURL());
      before = clc.getClassLoaderForClassPath(parent, classPath);
      assertNotNull(before.getResource("first"));

      replaceJar(jar, "second");
      clc.getClassLoaderForClassPath(parent, classPath);

      // A compilation that started with the old class loader can keep using it.
      assertNotNull(before.getResource("first"));
    }

    assertNull("closing the cache closes replaced class loaders", before.getResource("first"));
  }
}
