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

import com.facebook.buck.util.function.ThrowingSupplier;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Preconditions;
import com.google.common.base.Throwables;
import org.jspecify.annotations.Nullable;

/** A Generic memoizer capable of memoizing Exceptions */
@Nullsafe(Nullsafe.Mode.LOCAL)
class AbstractMemoizer<T, E extends Exception> {

  @Nullable private volatile T value = null;
  @Nullable private volatile E exception = null;

  /**
   * Get the value and memoize the result. Constructs the value if it hasn't been memoized before,
   * or if the previously memoized value has been collected.
   *
   * @param delegate delegate for constructing the value
   * @return the value
   */
  public T get(ThrowingSupplier<T, E> delegate, Class<E> exceptionClass) throws E {
    if (value == null && exception == null) {
      synchronized (this) {
        if (value == null) {
          try {
            T t = Preconditions.checkNotNull(delegate.get());
            value = t;
            return t;
          } catch (Exception e) {
            if (exceptionClass.isInstance(e)) {
              exception = exceptionClass.cast(e);
              Throwables.throwIfInstanceOf(e, exceptionClass);
            }
            Throwables.throwIfUnchecked(e);
            throw new IllegalStateException(e);
          }
        }
      }
    }
    if (value != null) {
      return value;
    }
    throw exception;
  }
}
