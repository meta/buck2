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
import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.tree.CompilationUnitTree;
import com.sun.source.tree.Tree;
import com.sun.source.util.JavacTask;
import com.sun.source.util.TaskListener;
import com.sun.source.util.Trees;
import java.io.IOException;
import java.lang.reflect.InvocationTargetException;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.function.Consumer;
import javax.annotation.processing.Processor;
import javax.lang.model.element.Element;
import javax.lang.model.type.TypeMirror;
import javax.lang.model.util.Elements;
import javax.lang.model.util.Types;
import javax.tools.JavaFileObject;
import org.jspecify.annotations.Nullable;

/**
 * Extends {@link JavacTask} with functionality that is useful for Buck:
 *
 * <ul>
 *   <li>Exposes the enter method from JavacTaskImpl
 *   <li>Runs {@link BuckJavacPlugin}s and post-enter callbacks
 *   <li>Wraps {@link Elements} with some extended functionality
 * </ul>
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class BuckJavacTask extends JavacTask {
  protected final JavacTask inner;

  private final Map<BuckJavacPlugin, String[]> pluginsAndArgs = new LinkedHashMap<>();
  private final List<Consumer<Set<Element>>> postEnterCallbacks = new ArrayList<>();

  private boolean prepared = false;

  @Nullable private ElementsExtended elements;

  public BuckJavacTask(JavacTask inner) {
    if (inner instanceof BuckJavacTask) {
      throw new IllegalArgumentException();
    }
    this.inner = inner;
  }

  @Override
  public Iterable<? extends CompilationUnitTree> parse() throws IOException {
    prepare();
    return inner.parse();
  }

  public Iterable<? extends Element> enter() throws IOException {
    prepare();
    try {
      @SuppressWarnings("unchecked")
      Iterable<? extends Element> result =
          (Iterable<? extends Element>) inner.getClass().getMethod("enter").invoke(inner);
      return Objects.requireNonNull(result);
    } catch (IllegalAccessException | NoSuchMethodException e) {
      throw new RuntimeException(e);
    } catch (InvocationTargetException e) {
      if (e.getCause() instanceof IOException) {
        throw (IOException) e.getCause();
      } else if (e.getCause() instanceof RuntimeException) {
        throw (RuntimeException) e.getCause();
      } else if (e.getCause() instanceof Error) {
        throw (Error) e.getCause();
      }

      throw new RuntimeException(e);
    }
  }

  @Override
  public Iterable<? extends Element> analyze() throws IOException {
    prepare();
    return inner.analyze();
  }

  @Override
  public Iterable<? extends JavaFileObject> generate() throws IOException {
    prepare();
    return inner.generate();
  }

  @Override
  public Boolean call() {
    prepare();
    return inner.call();
  }

  @Override
  public ElementsExtended getElements() {
    if (elements == null) {
      elements = new ElementsExtendedImpl(inner.getElements(), getTypes(), getTrees());
    }
    return elements;
  }

  @Override
  public Types getTypes() {
    return inner.getTypes();
  }

  @Override
  @Nullable
  public TypeMirror getTypeMirror(Iterable<? extends Tree> path) {
    return inner.getTypeMirror(path);
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
  public void setProcessors(Iterable<? extends Processor> processors) {
    inner.setProcessors(processors);
  }

  @Override
  public void setLocale(Locale locale) {
    inner.setLocale(locale);
  }

  @Override
  public void addModules(Iterable<String> moduleNames) {
    inner.addModules(moduleNames);
  }

  public void addPlugin(BuckJavacPlugin plugin, String... args) {
    pluginsAndArgs.put(plugin, args);
  }

  public void addPostEnterCallback(Consumer<Set<Element>> callback) {
    postEnterCallbacks.add(callback);
  }

  public Trees getTrees() {
    return Trees.instance(inner);
  }

  /**
   * Initializes plugins and registers the post-enter listener before the task starts running.
   * Plugin listeners therefore receive every event, after the listeners registered before the task
   * started, and the post-enter callbacks run after all of them. Subclasses that run the inner task
   * directly must call this first.
   */
  protected void prepare() {
    if (prepared) {
      return;
    }
    prepared = true;

    for (Map.Entry<BuckJavacPlugin, String[]> pluginAndArgs : pluginsAndArgs.entrySet()) {
      pluginAndArgs.getKey().init(this, pluginAndArgs.getValue());
    }
    pluginsAndArgs.clear();

    inner.addTaskListener(new PostEnterTaskListener(this, this::onPostEnter));
  }

  protected void onPostEnter(Set<Element> topLevelElements) {
    postEnterCallbacks.forEach(callback -> callback.accept(topLevelElements));
  }
}
