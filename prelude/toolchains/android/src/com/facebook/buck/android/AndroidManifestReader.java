/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android;

import com.facebook.infer.annotation.Nullsafe;
import java.util.List;

/** Allows querying an Android manifest file for various types of information. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface AndroidManifestReader {

  /**
   * @return list of names (as they appear in the manifest) of activities that should appear in the
   *     Android app drawer.
   */
  List<String> getLauncherActivities();

  /**
   * @return the value of the package attribute to the manifest element.
   */
  String getPackage();

  /**
   * @return the value of the versionCode attribute to the manifest element.
   */
  String getVersionCode();

  /**
   * @return the name of the instrumentation test runner.
   */
  String getInstrumentationTestRunner();

  /**
   * @return the value of the target package attribute to the manifest element.
   */
  String getTargetPackage();
}
