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
import java.util.Objects;
import org.jspecify.annotations.Nullable;

/**
 * A {@link TestDescription} will match if this selector's class-part is identical to the
 * TestDescriptions class name (same for the method name).
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class SimpleTestSelector implements TestSelector {
  private final @Nullable String className;
  private final @Nullable String methodName;

  public SimpleTestSelector(@Nullable String className, @Nullable String methodName) {
    this.className = className;
    this.methodName = methodName;
  }

  @Override
  public String getRawSelector() {
    // This method is effectively used by Buck to plumb the args passed to the '-f' option into
    // the --test-selectors option that is passed to the junit test runner.
    // The Simple selector is intended to be only used by whoever directly calls the junit runner.
    throw new UnsupportedOperationException("SimpleTestSelector does not have a raw selector.");
  }

  @Override
  public String getExplanation() {
    return String.format(
        "class:%s method:%s",
        isMatchAnyClass() ? "<any>" : className, isMatchAnyMethod() ? "<any>" : methodName);
  }

  @Override
  public boolean isInclusive() {
    return true;
  }

  @Override
  public boolean isMatchAnyClass() {
    return className == null;
  }

  @Override
  public boolean isMatchAnyMethod() {
    return methodName == null;
  }

  @Override
  public boolean matches(TestDescription description) {
    return matchesClassName(description.getClassName())
        && matchesMethodName(description.getMethodName());
  }

  @Override
  public boolean matchesClassName(String thatClassName) {
    if (className == null) {
      return true;
    }
    return Objects.equals(this.className, thatClassName);
  }

  @Override
  public boolean containsClassPath(String classPath) {
    // classpath of com.example.A should match selectors matching com.example.A and
    // com.example.A.Inner
    if (className == null) {
      return true;
    }
    return className.startsWith(classPath);
  }

  private boolean matchesMethodName(String thatMethodName) {
    if (methodName == null) {
      return true;
    }
    return Objects.equals(this.methodName, thatMethodName);
  }
}
