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

import com.facebook.buck.jvm.java.plugin.api.BuckJavacTaskListener;
import com.facebook.buck.jvm.java.plugin.api.TaskEventMirror;
import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.util.TaskEvent;
import com.sun.source.util.TaskListener;

/**
 * Implements {@link BuckJavacTaskListener} by proxying calls to an inner {@link TaskListener}. This
 * is the bridge that allows Buck to call {@link TaskListener}s that are loaded in the compiler's
 * {@link ClassLoader}.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class TaskListenerProxy implements BuckJavacTaskListener {
  private final TaskListener taskListener;

  public TaskListenerProxy(Object taskListener) {
    if (taskListener instanceof BuckJavacTaskListenerProxy) {
      throw new IllegalArgumentException(
          "taskListener is a proxy, unwrap it rather than creating another proxy");
    }
    this.taskListener = (TaskListener) taskListener;
  }

  public TaskListener getInner() {
    return taskListener;
  }

  @Override
  public void started(TaskEventMirror e) {
    taskListener.started(unmirror(e));
  }

  @Override
  public void finished(TaskEventMirror e) {
    taskListener.finished(unmirror(e));
  }

  private TaskEvent unmirror(TaskEventMirror mirror) {
    return (TaskEvent) mirror.getOriginal();
  }
}
