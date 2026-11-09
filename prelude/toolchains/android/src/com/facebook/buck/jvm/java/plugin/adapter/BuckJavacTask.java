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
import com.sun.source.util.JavacTask;
import com.sun.source.util.TaskEvent;
import com.sun.source.util.TaskListener;
import com.sun.source.util.Trees;
import java.io.IOException;
import java.lang.reflect.InvocationTargetException;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.function.Consumer;
import javax.lang.model.element.Element;
import javax.lang.model.util.Elements;
import org.jspecify.annotations.Nullable;

/**
 * Extends {@link JavacTask} with functionality that is useful for Buck:
 *
 * <ul>
 *   <li>Exposes the enter method from JavacTaskImpl
 *   <li>Pre-javac-8 support for addTaskListener/removeTaskListener
 *   <li>Wraps {@link Elements} with some extended functionality
 * </ul>
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class BuckJavacTask extends JavacTaskWrapper {
  private final Map<BuckJavacPlugin, String[]> pluginsAndArgs = new LinkedHashMap<>();
  private final MultiplexingTaskListener taskListeners = new MultiplexingTaskListener();
  private final PostEnterTaskListener postEnterTaskListener;
  private final List<Consumer<Set<Element>>> postEnterCallbacks = new ArrayList<>();

  private boolean pluginsInstalled = false;

  @Nullable private TaskListener singleTaskListener;
  @Nullable private ElementsExtended elements;

  public BuckJavacTask(JavacTask inner) {
    super(inner);
    if (inner instanceof BuckJavacTask) {
      throw new IllegalArgumentException();
    }

    inner.setTaskListener(
        new TaskListener() {
          @Override
          public void started(TaskEvent e) {
            BuckJavacTask.this.started(e);
          }

          @Override
          public void finished(TaskEvent e) {
            BuckJavacTask.this.finished(e);
          }
        });

    postEnterTaskListener = new PostEnterTaskListener(this, this::onPostEnter);
  }

  public Iterable<? extends Element> enter() throws IOException {
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

  /**
   * Sets a {@link TaskListener}. Like {@link JavacTask}'s implementation of this method, the
   * listener does not replace listeners added with {@link #addTaskListener(TaskListener)}. Instead,
   * it replaces only the listener provided in the previous call to this method, if any.
   *
   * <p>Presumably this behavior was to enable {@link com.sun.source.util.Plugin}s to work properly
   * with build systems that were written when only a single {@link TaskListener} was supported at a
   * time.
   */
  @Override
  public void setTaskListener(@Nullable TaskListener taskListener) {
    if (singleTaskListener != null) {
      removeTaskListener(singleTaskListener);
    }
    singleTaskListener = taskListener;
    if (singleTaskListener != null) {
      addTaskListener(singleTaskListener);
    }
  }

  @Override
  public void addTaskListener(TaskListener taskListener) {
    taskListeners.addListener(taskListener);
  }

  @Override
  public void removeTaskListener(TaskListener taskListener) {
    taskListeners.removeListener(taskListener);
  }

  @Override
  public ElementsExtended getElements() {
    if (elements == null) {
      elements = new ElementsExtendedImpl(super.getElements(), getTypes(), getTrees());
    }
    return elements;
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

  protected void started(TaskEvent e) {
    // Initialize plugins just before sending the first event to registered listeners. We do it
    // this way (rather than initializing plugins just before starting to run the task) because
    // most plugins will call methods like getElements, which in javac 7 are not safe to call
    // before the task is actually running.
    installPlugins();

    taskListeners.started(e);
    postEnterTaskListener.started(e);
  }

  protected void finished(TaskEvent e) {
    taskListeners.finished(e);
    postEnterTaskListener.finished(e);
  }

  protected void onPostEnter(Set<Element> topLevelElements) {
    postEnterCallbacks.forEach(callback -> callback.accept(topLevelElements));
  }

  private void installPlugins() {
    if (pluginsInstalled) {
      return;
    }

    for (Map.Entry<BuckJavacPlugin, String[]> pluginAndArgs : pluginsAndArgs.entrySet()) {
      pluginAndArgs.getKey().init(BuckJavacTask.this, pluginAndArgs.getValue());
    }

    pluginsAndArgs.clear();
    pluginsInstalled = true;
  }
}
