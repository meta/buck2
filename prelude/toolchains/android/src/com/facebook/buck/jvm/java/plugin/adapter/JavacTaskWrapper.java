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

import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.tree.CompilationUnitTree;
import com.sun.source.tree.Tree;
import com.sun.source.util.JavacTask;
import com.sun.source.util.TaskListener;
import java.io.IOException;
import java.util.Locale;
import javax.annotation.processing.Processor;
import javax.lang.model.element.Element;
import javax.lang.model.type.TypeMirror;
import javax.lang.model.util.Elements;
import javax.lang.model.util.Types;
import javax.tools.JavaFileObject;
import org.jspecify.annotations.Nullable;

/**
 * NOTE: A Java8 copy of this file exists in ../java8/JavacTaskWrapper.java. Please make sure to
 * update the other copy when modifying this file.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
abstract class JavacTaskWrapper extends JavacTask {
  protected final JavacTask inner;

  public JavacTaskWrapper(JavacTask inner) {
    this.inner = inner;
  }

  @Override
  public Iterable<? extends CompilationUnitTree> parse() throws IOException {
    return inner.parse();
  }

  @Override
  public Iterable<? extends Element> analyze() throws IOException {
    return inner.analyze();
  }

  @Override
  public Iterable<? extends JavaFileObject> generate() throws IOException {
    return inner.generate();
  }

  @Override
  public void setTaskListener(TaskListener taskListener) {
    inner.setTaskListener(taskListener);
  }

  @Override
  public void addTaskListener(TaskListener taskListener) {
    inner.addTaskListener(taskListener);
  }

  @Override
  public void removeTaskListener(TaskListener taskListener) {
    inner.removeTaskListener(taskListener);
  }

  @Override
  @Nullable
  public TypeMirror getTypeMirror(Iterable<? extends Tree> path) {
    return inner.getTypeMirror(path);
  }

  @Override
  public Elements getElements() {
    return inner.getElements();
  }

  @Override
  public Types getTypes() {
    return inner.getTypes();
  }

  @Override
  public void setProcessors(Iterable<? extends Processor> processors) {
    inner.setProcessors(processors);
  }

  @Override
  public void setLocale(Locale locale) {
    inner.setLocale(locale);
  }

  @Override
  public Boolean call() {
    return inner.call();
  }

  @Override
  public void addModules(Iterable<String> moduleNames) {
    inner.addModules(moduleNames);
  }
}
