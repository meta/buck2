/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.test.selectors;

import com.facebook.infer.annotation.Nullsafe;

/**
 * A way of matching a test-method in a test-class, and saying whether or not to include any matches
 * in a test run.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface TestSelector {
  String getRawSelector();

  String getExplanation();

  boolean isInclusive();

  boolean isMatchAnyClass();

  boolean isMatchAnyMethod();

  /**
   * Whether this {@link TestSelector} matches the given {@link TestDescription}. A class or method
   * name being null in the {@link TestDescription} means that it will match anything.
   *
   * @param description the {@link TestDescription} to match
   * @return true if this selector matches the given {@link TestDescription}
   */
  boolean matches(TestDescription description);

  /**
   * @return true if the given className matches this selector
   */
  boolean matchesClassName(String className);

  /**
   * @return true if the given classpath may be required by any classes that matches this selector
   */
  boolean containsClassPath(String classPath);
}
