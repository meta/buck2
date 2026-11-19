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

import com.facebook.buck.jvm.java.lang.model.ElementsExtended;
import com.facebook.buck.jvm.java.plugin.api.BuckJavacTaskListener;
import com.facebook.buck.jvm.java.plugin.api.BuckJavacTaskProxy;
import com.facebook.buck.jvm.java.plugin.api.CompilationUnitTreeProxy;
import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.util.JavacTask;
import com.sun.source.util.TaskListener;
import java.io.IOException;
import java.util.Locale;
import java.util.Set;
import java.util.function.Consumer;
import java.util.stream.Collectors;
import java.util.stream.StreamSupport;
import javax.annotation.processing.Messager;
import javax.annotation.processing.Processor;
import javax.lang.model.element.Element;
import javax.lang.model.util.Types;
import javax.tools.JavaCompiler;
import javax.tools.JavaFileObject;

/**
 * NOTE: A Java8 copy of this file exists in ../java8/BuckJavacTaskProxyImpl.java. Please make sure
 * to update the other copy when modifying this file.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class BuckJavacTaskProxyImpl implements BuckJavacTaskProxy {
  private final BuckJavacTask javacTask;

  public BuckJavacTaskProxyImpl(JavaCompiler.CompilationTask javacTask) {
    this.javacTask = new BuckJavacTask((JavacTask) javacTask);
  }

  public BuckJavacTaskProxyImpl(BuckJavacTask javacTask) {
    this.javacTask = javacTask;
  }

  public BuckJavacTask getInner() {
    return javacTask;
  }

  @Override
  public Iterable<CompilationUnitTreeProxy> parse() throws IOException {
    return StreamSupport.stream(javacTask.parse().spliterator(), false)
        .map(CompilationUnitTreeProxyImpl::new)
        .collect(Collectors.toList());
  }

  @Override
  public Iterable<? extends Element> enter() throws IOException {
    return javacTask.enter();
  }

  @Override
  public Iterable<? extends Element> analyze() throws IOException {
    return javacTask.analyze();
  }

  @Override
  public Iterable<? extends JavaFileObject> generate() throws IOException {
    return javacTask.generate();
  }

  @Override
  public void setTaskListener(BuckJavacTaskListener buckTaskListener) {
    javacTask.setTaskListener(getTaskListener(buckTaskListener));
  }

  @Override
  public void addTaskListener(BuckJavacTaskListener buckTaskListener) {
    javacTask.addTaskListener(getTaskListener(buckTaskListener));
  }

  @Override
  public void removeTaskListener(BuckJavacTaskListener buckTaskListener) {
    javacTask.removeTaskListener(getTaskListener(buckTaskListener));
  }

  private TaskListener getTaskListener(BuckJavacTaskListener taskListener) {
    if (taskListener instanceof TaskListenerProxy) {
      return ((TaskListenerProxy) taskListener).getInner();
    }

    return new BuckJavacTaskListenerProxy(taskListener);
  }

  @Override
  public void addPostEnterCallback(Consumer<Set<Element>> callback) {
    javacTask.addPostEnterCallback(callback);
  }

  @Override
  public ElementsExtended getElements() {
    return javacTask.getElements();
  }

  @Override
  public Types getTypes() {
    return javacTask.getTypes();
  }

  @Override
  public Messager getMessager() {
    return new TreesMessager(javacTask.getTrees());
  }

  @Override
  public void setProcessors(Iterable<? extends Processor> processors) {
    javacTask.setProcessors(processors);
  }

  @Override
  public void setLocale(Locale locale) {
    javacTask.setLocale(locale);
  }

  @Override
  public Boolean call() {
    return javacTask.call();
  }

  @Override
  public void addModules(Iterable<String> moduleNames) {
    javacTask.addModules(moduleNames);
  }
}
