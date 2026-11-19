/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.installer.android;

import com.facebook.buck.android.AdbHelper;
import com.facebook.infer.annotation.Nullsafe;
import java.util.function.Supplier;
import org.jspecify.annotations.Nullable;

/**
 * Everything one install accumulates, so that something new to track is a field here rather than
 * another map keyed by install id.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
final class InstallState {
  private final AndroidArtifacts artifacts = new AndroidArtifacts();
  private final InstallMetrics metrics = new InstallMetrics();
  private final StreamedPushes streamedPushes = new StreamedPushes();

  // Written once, under this object's monitor; read without it, from the threads delivering
  // artifacts. Those reads happen on every arrival, so they must not queue behind a resolution
  // that is talking to adb.
  @Nullable private volatile AdbHelper adbHelper;

  // Read when the manifest arrives, because the manifest does not change during an install and
  // several stages of one ask for the package.
  @Nullable private volatile String packageName;

  AndroidArtifacts artifacts() {
    return artifacts;
  }

  InstallMetrics metrics() {
    return metrics;
  }

  /** The package this install is for, or null until the manifest has arrived. */
  @Nullable String packageName() {
    return packageName;
  }

  void setPackageName(String packageName) {
    this.packageName = packageName;
  }

  /** What has already been sent ahead of this install, so no payload is sent twice. */
  StreamedPushes streamedPushes() {
    return streamedPushes;
  }

  /** Bound to the devices this install targets, or null until its options have arrived. */
  @Nullable AdbHelper adbHelper() {
    return adbHelper;
  }

  /**
   * Resolves the devices if they are not resolved already, and answers with them.
   *
   * <p>Resolution happens once however many arrivals ask for it: the answer is what pins this
   * install to a set of devices, so a second resolution would move that instant.
   */
  synchronized AdbHelper resolveDevices(Supplier<AdbHelper> resolver) {
    if (adbHelper == null) {
      adbHelper = resolver.get();
    }
    return adbHelper;
  }
}
