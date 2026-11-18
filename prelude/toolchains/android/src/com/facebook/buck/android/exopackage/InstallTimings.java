/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.exopackage;

import com.facebook.infer.annotation.Nullsafe;

/**
 * Collects how long each stage of an install took.
 *
 * <p>Paired with when each artifact arrived, this is enough to work out the earliest the install
 * could have finished had it never waited for an artifact it did not yet need.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface InstallTimings {

  /** Preparing the device: creating the data root and listing what is already on it. */
  void recordDeviceSetup(long startMillis, long endMillis);

  /**
   * Pushing one payload.
   *
   * @param group the payload type, as passed to {@link AndroidDevice#installFiles} -- e.g. {@code
   *     secondary_dex}, {@code native_library}, {@code resources}, {@code metadata}.
   */
  void recordPush(String group, long startMillis, long endMillis);

  /** Installing the apk itself. */
  void recordApkInstall(long startMillis, long endMillis);

  /**
   * The whole device-side phase. Whatever this covers beyond the stages above -- collecting stale
   * files, reading back digests, writing the build id -- has to happen regardless of when artifacts
   * arrive, so it belongs on the critical path too.
   */
  void recordDeviceWork(long startMillis, long endMillis);

  /** Discards everything. For call sites that are not part of a measured install. */
  InstallTimings NONE =
      new InstallTimings() {
        @Override
        public void recordDeviceSetup(long startMillis, long endMillis) {}

        @Override
        public void recordPush(String group, long startMillis, long endMillis) {}

        @Override
        public void recordApkInstall(long startMillis, long endMillis) {}

        @Override
        public void recordDeviceWork(long startMillis, long endMillis) {}
      };
}
