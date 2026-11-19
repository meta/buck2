/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util;

import com.facebook.buck.core.util.log.Logger;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Throwables;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class Threads {

  private static final Logger LOG = Logger.get(Threads.class);

  /** Utility class: do not instantiate. */
  private Threads() {}

  public static Thread namedThread(String name, Runnable runnable) {
    Thread newThread = new Thread(runnable);
    newThread.setName(name);
    return newThread;
  }

  public static void interruptCurrentThread() {
    LOG.warn(
        "Current thread interrupted at this location: "
            + Throwables.getStackTraceAsString(new Throwable()));
    Thread.currentThread().interrupt();
  }

  public static void interruptThread(Thread thread) {
    LOG.warn(
        "Thread interrupted at this location: "
            + Throwables.getStackTraceAsString(new Throwable()));
    thread.interrupt();
  }
}
