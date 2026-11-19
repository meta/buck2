/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi.source.api;

import com.facebook.buck.jvm.java.plugin.api.BuckJavacTaskProxy;
import com.facebook.buck.jvm.java.plugin.api.PluginClassLoader;
import com.facebook.buck.jvm.java.plugin.api.PluginClassLoaderFactory;
import com.facebook.infer.annotation.Nullsafe;
import java.io.Writer;
import java.util.Objects;
import javax.tools.DiagnosticListener;
import javax.tools.JavaCompiler;
import javax.tools.JavaFileManager;
import javax.tools.JavaFileObject;

@Nullsafe(Nullsafe.Mode.LOCAL)
public interface FrontendOnlyJavacTaskProxy extends BuckJavacTaskProxy {
  static FrontendOnlyJavacTaskProxy getTask(
      PluginClassLoaderFactory loaderFactory,
      JavaCompiler compiler,
      Writer out,
      JavaFileManager fileManager,
      DiagnosticListener<? super JavaFileObject> diagnosticListener,
      Iterable<String> options,
      Iterable<String> classes,
      Iterable<? extends JavaFileObject> compilationUnits) {
    ErrorSuppressingDiagnosticListener errorSuppressingDiagnosticListener =
        new ErrorSuppressingDiagnosticListener(diagnosticListener);
    JavaCompiler.CompilationTask task =
        compiler.getTask(
            out,
            fileManager,
            errorSuppressingDiagnosticListener,
            options,
            classes,
            compilationUnits);
    errorSuppressingDiagnosticListener.setTask(task);

    PluginClassLoader loader = loaderFactory.getPluginClassLoader(task);
    Class<? extends FrontendOnlyJavacTaskProxy> proxyImplClass =
        loader.loadClass(
            "com.facebook.buck.jvm.java.abi.source.FrontendOnlyJavacTaskProxyImpl",
            FrontendOnlyJavacTaskProxy.class);

    try {
      return Objects.requireNonNull(proxyImplClass)
          .getConstructor(JavaCompiler.CompilationTask.class)
          .newInstance(task);
    } catch (ReflectiveOperationException e) {
      throw new RuntimeException(e);
    }
  }
}
