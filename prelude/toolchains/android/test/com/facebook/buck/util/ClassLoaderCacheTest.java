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
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;

import com.google.common.collect.FluentIterable;
import com.google.common.collect.ImmutableList;
import java.io.File;
import java.io.IOException;
import java.net.URL;
import java.net.URLClassLoader;
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
}
