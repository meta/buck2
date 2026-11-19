/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.device;

import com.facebook.infer.annotation.Nullsafe;
import java.util.Optional;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class TargetDeviceOptions {

  private boolean useEmulatorsOnlyMode;

  private boolean useRealDevicesOnlyMode;

  private Optional<String> serialNumber;

  public TargetDeviceOptions() {
    this(false, false, Optional.empty());
  }

  public TargetDeviceOptions(
      boolean useEmulatorsOnlyMode, boolean useRealDevicesOnlyMode, Optional<String> serialNumber) {
    this.useEmulatorsOnlyMode = useEmulatorsOnlyMode;
    this.useRealDevicesOnlyMode = useRealDevicesOnlyMode;
    this.serialNumber = serialNumber;
  }

  public boolean isEmulatorsOnlyModeEnabled() {
    return useEmulatorsOnlyMode;
  }

  public boolean isRealDevicesOnlyModeEnabled() {
    return useRealDevicesOnlyMode;
  }

  public Optional<String> getSerialNumber() {
    return serialNumber;
  }

  @Override
  public String toString() {
    return "TargetDeviceOptions{"
        + "useEmulatorsOnlyMode="
        + useEmulatorsOnlyMode
        + ", useRealDevicesOnlyMode="
        + useRealDevicesOnlyMode
        + ", serialNumber="
        + serialNumber
        + '}';
  }
}
