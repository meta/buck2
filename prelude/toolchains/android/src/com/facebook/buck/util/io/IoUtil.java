/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.io;

import com.facebook.infer.annotation.Nullsafe;
import java.io.Closeable;
import java.io.IOException;

/** IO utilities. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class IoUtil {
  /** Like {@link java.util.function.Function} but allows throwing {@link IOException}. */
  public interface IoFunction<A, B> {
    /** Apply the function. */
    B apply(A a) throws IOException;
  }

  /** Apply a function to just opened output stream; close the stream if function fails. */
  public static <I extends Closeable, R> R mapJustOpened(I justOpened, IoFunction<I, R> f)
      throws IOException {
    try {
      return f.apply(justOpened);
    } catch (Throwable e) {
      try {
        justOpened.close();
      } catch (Throwable ignore) {
        // ignore
      }
      // original exception is more important, and generally close should not throw
      throw e;
    }
  }
}
