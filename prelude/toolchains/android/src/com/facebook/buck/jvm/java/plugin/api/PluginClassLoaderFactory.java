/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.plugin.api;

import com.facebook.infer.annotation.Nullsafe;
import javax.tools.JavaCompiler;

/** Factory interface for {@link PluginClassLoader}s for different compiler instances */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface PluginClassLoaderFactory {
  PluginClassLoader getPluginClassLoader(JavaCompiler.CompilationTask task);
}
