/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin;

/**
 * JNI entry for in-worker rokoko stubgen (prototype). The worker loads the cdylib once per process,
 * so the jar-index cache persists across actions. Returns null on success, else the failure
 * message.
 */
public final class RokokoStubgenNative {
  private RokokoStubgenNative() {}

  public static native String generate(
      String[] sources, String[] classpath, String classesOut, String stubsOut, String logPath);
}
