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
import com.facebook.buck.jvm.java.plugin.api.CompilationUnitTreeProxy;
import com.facebook.buck.jvm.java.plugin.api.TaskEventMirror;
import com.facebook.buck.util.liteinfersupport.Nullable;
import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.tree.CompilationUnitTree;
import com.sun.source.util.TaskEvent;
import com.sun.source.util.TaskListener;

/**
 * Implements {@link TaskListener} by proxying calls to an inner {@link BuckJavacTaskListener}. This
 * is the bridge that allows us to implement {@link TaskListener}s in Buck itself.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class BuckJavacTaskListenerProxy implements TaskListener {
  private final BuckJavacTaskListener buckSideListener;

  public BuckJavacTaskListenerProxy(BuckJavacTaskListener buckSideListener) {
    if (buckSideListener instanceof TaskListenerProxy) {
      throw new IllegalArgumentException(
          "taskListener is a proxy, unwrap it rather than creating another proxy");
    }
    this.buckSideListener = buckSideListener;
  }

  @Override
  public void started(TaskEvent e) {
    this.buckSideListener.started(mirrorTaskEvent(e));
  }

  @Override
  public void finished(TaskEvent e) {
    this.buckSideListener.finished(mirrorTaskEvent(e));
  }

  private TaskEventMirror mirrorTaskEvent(TaskEvent e) {
    return new TaskEventMirror(
        e,
        mirrorKind(e.getKind()),
        e.getSourceFile(),
        proxyCompilationUnit(e.getCompilationUnit()),
        e.getTypeElement());
  }

  private TaskEventMirror.Kind mirrorKind(TaskEvent.Kind kind) {
    return TaskEventMirror.Kind.valueOf(kind.name());
  }

  @Nullable
  private CompilationUnitTreeProxy proxyCompilationUnit(@Nullable CompilationUnitTree tree) {
    if (tree == null) {
      return null;
    }

    return new CompilationUnitTreeProxyImpl(tree);
  }
}
