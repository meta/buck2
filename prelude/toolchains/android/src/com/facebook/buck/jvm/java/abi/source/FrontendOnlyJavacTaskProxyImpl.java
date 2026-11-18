/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi.source;

import com.facebook.buck.jvm.java.abi.source.api.FrontendOnlyJavacTaskProxy;
import com.facebook.buck.jvm.java.plugin.adapter.BuckJavacTaskProxyImpl;
import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.util.JavacTask;
import javax.tools.JavaCompiler;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class FrontendOnlyJavacTaskProxyImpl extends BuckJavacTaskProxyImpl
    implements FrontendOnlyJavacTaskProxy {
  private final FrontendOnlyJavacTask javacTask;

  public FrontendOnlyJavacTaskProxyImpl(JavaCompiler.CompilationTask task) {
    this(new FrontendOnlyJavacTask((JavacTask) task));
  }

  public FrontendOnlyJavacTaskProxyImpl(FrontendOnlyJavacTask javacTask) {
    super(javacTask);
    this.javacTask = javacTask;
  }

  @Override
  public FrontendOnlyJavacTask getInner() {
    return javacTask;
  }
}
