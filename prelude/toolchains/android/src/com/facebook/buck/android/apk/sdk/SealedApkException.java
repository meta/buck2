/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.apk.sdk;

import com.facebook.infer.annotation.Nullsafe;

/** An exception thrown when trying to add files to a sealed APK. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class SealedApkException extends Exception {
  private static final long serialVersionUID = 1L;

  public SealedApkException(String format, Object... args) {
    super(String.format(format, args));
  }

  public SealedApkException(Throwable cause, String format, Object... args) {
    super(String.format(format, args), cause);
  }

  public SealedApkException(Throwable cause) {
    super(cause);
  }
}
