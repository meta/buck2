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

import com.facebook.infer.annotation.Nullsafe;
import java.util.Objects;

/**
 * Listens to events coming from the Java compiler. This is analogous to {@link
 * com.sun.source.util.TaskListener}, but loads in Buck's {@link ClassLoader} instead of the
 * compiler's.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface BuckJavacTaskListener {
  static BuckJavacTaskListener wrapRealTaskListener(PluginClassLoader loader, Object taskListener) {
    Class<? extends BuckJavacTaskListener> taskListenerProxyClass =
        loader.loadClass(
            "com.facebook.buck.jvm.java.plugin.adapter.TaskListenerProxy",
            BuckJavacTaskListener.class);

    try {
      return Objects.requireNonNull(taskListenerProxyClass)
          .getConstructor(Object.class)
          .newInstance(taskListener);
    } catch (ReflectiveOperationException e) {
      throw new RuntimeException(e);
    }
  }

  void started(TaskEventMirror e);

  void finished(TaskEventMirror e);
}
