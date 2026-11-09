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

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Preconditions;
import com.google.common.collect.ImmutableMap;
import java.net.URI;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.regex.Pattern;

/** Take URI input for class usage data, and output the total class usage as map */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ClassUsageURIParser {

  private static final String FILE_SCHEME = "file";
  private static final String JAR_SCHEME = "jar";
  private static final String JIMFS_SCHEME = "jimfs"; // Used in tests

  // Examples: First anonymous class is Foo$1.class. First local class named Bar is Foo$1Bar.class.
  private static final Pattern LOCAL_OR_ANONYMOUS_CLASS = Pattern.compile("^.*\\$\\d.*.class$");

  private final Map<Path, Set<Path>> resultBuilder = new HashMap<>();

  /**
   * Returns a map from JAR path on disk to .class file paths within the jar for any classes that
   * were used, and the count for how often those classes were read.
   */
  public ImmutableMap<Path, Set<Path>> getClassUsageMap() {
    return ImmutableMap.copyOf(resultBuilder);
  }

  /** Parse URI input and record it in class usage cache */
  public void parseAndRecordURI(URI classFileJarUri) {
    if (classFileJarUri.getScheme() == null || !classFileJarUri.getScheme().equals(JAR_SCHEME)) {
      // Not in a jar; must not have been built with java_library
      return;
    }

    // The jar: scheme is somewhat underspecified. See the JarURLConnection docs
    // for the closest thing it has to documentation.
    String jarUriSchemeSpecificPart = classFileJarUri.getRawSchemeSpecificPart();
    String[] split = jarUriSchemeSpecificPart.split("!/");
    Preconditions.checkState(split.length == 2);

    if (isLocalOrAnonymousClass(split[1])) {
      // The compiler reads local and anonymous classes because of the naive way in which it
      // completes the enclosing class, but changes to them can't affect compilation of dependent
      // classes so we don't need to consider them "used".
      return;
    }

    URI jarFileUri = URI.create(split[0]);
    String jarFileUriScheme = Objects.requireNonNull(jarFileUri.getScheme());
    Preconditions.checkState(
        jarFileUriScheme.equals(FILE_SCHEME)
            || jarFileUriScheme.equals(JIMFS_SCHEME)); // jimfs is used in tests
    Path jarFilePath = Paths.get(jarFileUri);

    // Using URI.create here for de-escaping
    Path classPath = Paths.get(URI.create(split[1]).toString());

    Preconditions.checkState(jarFilePath.isAbsolute());
    Preconditions.checkState(!classPath.isAbsolute());

    Set<Path> classpaths = resultBuilder.computeIfAbsent(jarFilePath, _path -> new HashSet<>());
    classpaths.add(classPath);
  }

  private boolean isLocalOrAnonymousClass(String className) {
    return LOCAL_OR_ANONYMOUS_CLASS.matcher(className).matches();
  }
}
