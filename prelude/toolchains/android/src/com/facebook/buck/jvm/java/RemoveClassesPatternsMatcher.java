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
import com.google.common.base.MoreObjects;
import com.google.common.base.Objects;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSet;
import java.util.function.Predicate;
import java.util.regex.Pattern;
import java.util.zip.ZipEntry;
import org.jspecify.annotations.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class RemoveClassesPatternsMatcher implements Predicate<Object> {
  public static final RemoveClassesPatternsMatcher EMPTY =
      new RemoveClassesPatternsMatcher(ImmutableSet.of());

  private final ImmutableList<Pattern> patterns;
  private final boolean removeNonClassFiles;

  public RemoveClassesPatternsMatcher(ImmutableSet<Pattern> patterns) {
    this(patterns, false);
  }

  public RemoveClassesPatternsMatcher(ImmutableSet<Pattern> patterns, boolean removeNonClassFiles) {
    this.patterns = ImmutableList.copyOf(patterns);
    this.removeNonClassFiles = removeNonClassFiles;
  }

  private boolean shouldRemoveClass(ZipEntry entry) {
    if (patterns.isEmpty()) {
      return false;
    }

    if (!removeNonClassFiles && !entry.getName().endsWith(".class")) {
      return false;
    }

    return shouldRemoveClass(pathToClassName(entry.getName()));
  }

  private boolean shouldRemoveClass(String className) {
    if (patterns.isEmpty()) {
      return false;
    }

    for (Pattern pattern : patterns) {
      if (pattern.matcher(className).find()) {
        return true;
      }
    }

    return false;
  }

  private static String pathToClassName(String classFilePath) {
    return classFilePath.replace('/', '.').replace(".class", "");
  }

  @Override
  public boolean test(Object o) {
    if (o instanceof ZipEntry) {
      return shouldRemoveClass((ZipEntry) o);
    } else {
      return shouldRemoveClass((String) o);
    }
  }

  public ImmutableList<Pattern> getPatterns() {
    return patterns;
  }

  @Override
  public boolean equals(@Nullable Object o) {
    if (this == o) {
      return true;
    }
    if (o == null || getClass() != o.getClass()) {
      return false;
    }
    RemoveClassesPatternsMatcher that = (RemoveClassesPatternsMatcher) o;
    return Objects.equal(patterns, that.patterns);
  }

  @Override
  public int hashCode() {
    return Objects.hashCode(patterns);
  }

  @Override
  public String toString() {
    return MoreObjects.toStringHelper(this).add("patterns", patterns).toString();
  }
}
