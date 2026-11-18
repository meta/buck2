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

import com.facebook.buck.jvm.java.javax.SynchronizedToolProvider;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.MoreObjects;
import javax.tools.JavaCompiler;
import org.jspecify.annotations.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class JdkProvidedInMemoryJavac extends Jsr199Javac {

  public static final JdkProvidedInMemoryJavac INSTANCE = new JdkProvidedInMemoryJavac();

  private JdkProvidedInMemoryJavac() {}

  /** Creates in memory javac */
  public static ResolvedJsr199Javac createJsr199Javac() {
    return new ResolvedJsr199Javac() {

      @Override
      protected JavaCompiler createCompiler(JavacExecutionContext context) {
        JavaCompiler compiler = SynchronizedToolProvider.getSystemJavaCompiler();
        if (compiler == null) {
          throw new RuntimeException(
              "No system compiler found. Did you install the JRE instead of the JDK?");
        }
        return compiler;
      }
    };
  }

  @Override
  public boolean equals(@Nullable Object o) {
    if (this == o) {
      return true;
    }
    return o != null && getClass() == o.getClass();
  }

  @Override
  public int hashCode() {
    return super.hashCode();
  }

  @Override
  public String toString() {
    return MoreObjects.toStringHelper(this).toString();
  }
}
