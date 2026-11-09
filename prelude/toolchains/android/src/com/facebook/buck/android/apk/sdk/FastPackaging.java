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
import com.google.common.base.Preconditions;

/** Selects {@link FastApkJarBuilder} for an APK and sets how many threads deflate its files. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class FastPackaging {
  private final int mCompressionThreads;

  private FastPackaging(int compressionThreads) {
    Preconditions.checkArgument(
        compressionThreads >= 1, "compressionThreads must be positive: %s", compressionThreads);
    mCompressionThreads = compressionThreads;
  }

  public static FastPackaging withCompressionThreads(int compressionThreads) {
    return new FastPackaging(compressionThreads);
  }

  public int getCompressionThreads() {
    return mCompressionThreads;
  }
}
