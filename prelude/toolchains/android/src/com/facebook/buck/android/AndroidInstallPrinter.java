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

/** Interface for logging android installs in the AdbHelper */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface AndroidInstallPrinter {
  void printMessage(String message);

  void printSuccess(String message);

  void printWarning(String message);

  void printError(String message);
}
