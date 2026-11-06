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

import com.facebook.buck.android.AndroidInstallPrinter;
import com.facebook.infer.annotation.Nullsafe;
import java.util.logging.Level;
import java.util.logging.Logger; // NOPMD

/** Handles logging for android installs */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class IsolatedAndroidInstallerPrinter implements AndroidInstallPrinter {

  private final Logger logger;

  public IsolatedAndroidInstallerPrinter(Logger logger) {
    this.logger = logger;
  }

  @Override
  public void printMessage(String message) {
    logger.log(Level.INFO, message);
  }

  @Override
  public void printSuccess(String successMessage) {
    logger.log(Level.INFO, successMessage);
  }

  @Override
  public void printWarning(String message) {
    logger.log(Level.WARNING, message);
  }

  @Override
  public void printError(String failureMessage) {
    logger.log(Level.WARNING, failureMessage);
  }
}
