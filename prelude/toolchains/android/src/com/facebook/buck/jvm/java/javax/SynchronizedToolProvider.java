/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.javax;

import com.facebook.infer.annotation.Nullsafe;
import javax.tools.JavaCompiler;
import javax.tools.ToolProvider;
import org.jspecify.annotations.Nullable;

/**
 * ToolProvider has no synchronization internally, so if we don't synchronize from the outside we
 * could wind up loading the compiler classes multiple times from different class loaders.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class SynchronizedToolProvider {

  public static @Nullable JavaCompiler getSystemJavaCompiler() {
    JavaCompiler compiler;
    synchronized (ToolProvider.class) {
      compiler = ToolProvider.getSystemJavaCompiler();
    }
    return compiler;
  }
}
