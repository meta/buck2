/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.plugin.adapter;

import com.facebook.buck.jvm.java.plugin.api.CompilationUnitTreeProxy;
import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.tree.CompilationUnitTree;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class CompilationUnitTreeProxyImpl implements CompilationUnitTreeProxy {
  private final CompilationUnitTree compilationUnitTree;

  public CompilationUnitTreeProxyImpl(CompilationUnitTree compilationUnitTree) {
    this.compilationUnitTree = compilationUnitTree;
  }

  public CompilationUnitTree getCompilationUnit() {
    return compilationUnitTree;
  }
}
