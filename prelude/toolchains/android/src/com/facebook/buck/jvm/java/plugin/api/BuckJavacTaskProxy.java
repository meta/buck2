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

import com.facebook.buck.jvm.java.lang.model.ElementsExtended;
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.io.Writer;
import java.util.Objects;
import java.util.Set;
import java.util.function.Consumer;
import javax.annotation.processing.Messager;
import javax.lang.model.element.Element;
import javax.lang.model.util.Types;
import javax.tools.DiagnosticListener;
import javax.tools.JavaCompiler;
import javax.tools.JavaFileManager;
import javax.tools.JavaFileObject;

/**
 * {@link com.sun.source.util.JavacTask} is included with the compiler and is thus not directly
 * accessible from within Buck's class loader. This interface is used as a proxy within Buck's class
 * loader to allow access to commonly-used methods.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface BuckJavacTaskProxy extends JavaCompiler.CompilationTask {
  static BuckJavacTaskProxy getTask(
      PluginClassLoaderFactory loaderFactory,
      JavaCompiler compiler,
      Writer out,
      JavaFileManager fileManager,
      DiagnosticListener<? super JavaFileObject> diagnosticListener,
      Iterable<String> options,
      Iterable<String> classes,
      Iterable<? extends JavaFileObject> compilationUnits) {
    JavaCompiler.CompilationTask task =
        compiler.getTask(out, fileManager, diagnosticListener, options, classes, compilationUnits);

    PluginClassLoader loader = loaderFactory.getPluginClassLoader(task);
    Class<? extends BuckJavacTaskProxy> proxyImplClass =
        loader.loadClass(
            "com.facebook.buck.jvm.java.plugin.adapter.BuckJavacTaskProxyImpl",
            BuckJavacTaskProxy.class);

    try {
      return Objects.requireNonNull(proxyImplClass)
          .getConstructor(JavaCompiler.CompilationTask.class)
          .newInstance(task);
    } catch (ReflectiveOperationException e) {
      throw new RuntimeException(e);
    }
  }

  Iterable<CompilationUnitTreeProxy> parse() throws IOException;

  Iterable<? extends Element> enter() throws IOException;

  Iterable<? extends Element> analyze() throws IOException;

  Iterable<? extends JavaFileObject> generate() throws IOException;

  void setTaskListener(BuckJavacTaskListener taskListener);

  void addTaskListener(BuckJavacTaskListener taskListener);

  void removeTaskListener(BuckJavacTaskListener taskListener);

  void addPostEnterCallback(Consumer<Set<Element>> callback);

  ElementsExtended getElements();

  Types getTypes();

  Messager getMessager();
}
