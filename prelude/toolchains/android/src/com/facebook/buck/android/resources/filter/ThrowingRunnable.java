/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.resources.filter;

import com.facebook.infer.annotation.Nullsafe;

/**
 * A {@link Runnable} that can throw checked exceptions.
 *
 * @param <E> the type of exception that can be thrown
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
@FunctionalInterface
public interface ThrowingRunnable<E extends Exception> {
  void run() throws E;
}
