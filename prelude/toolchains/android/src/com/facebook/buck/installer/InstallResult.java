/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.installer;

import com.facebook.infer.annotation.Nullsafe;
import java.util.List;
import java.util.Map;
import java.util.Optional;

/** Represents a result from an install attempt */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class InstallResult {

  private static final InstallResult SUCCESS = new InstallResult(List.of(), Optional.empty());

  private final List<Map<String, String>> deviceMetadata;
  private final Optional<InstallError> installError;

  public InstallResult(
      List<Map<String, String>> deviceMetadata, Optional<InstallError> installError) {
    this.deviceMetadata = deviceMetadata;
    this.installError = installError;
  }

  public static InstallResult success() {
    return SUCCESS;
  }

  /**
   * @deprecated use {@link this.error(InstallError)} instead
   */
  @Deprecated
  public static InstallResult error(String errorMessage) {
    return new InstallResult(
        List.of(), Optional.of(new InstallError(errorMessage, DefaultInstallErrorTag.INSTANCE)));
  }

  public static InstallResult error(InstallError installError) {
    return new InstallResult(List.of(), Optional.of(installError));
  }

  public boolean isError() {
    return installError.isPresent();
  }

  public InstallError getInstallError() {
    return installError.orElseThrow(
        () ->
            new IllegalStateException(
                "installError is not present. `isError()` has to be called before invoking"
                    + " `getInstallError()`"));
  }

  public List<Map<String, String>> getDeviceMetadata() {
    return deviceMetadata;
  }

  public String getErrorMessage() {
    return installError
        .orElseThrow(
            () ->
                new IllegalStateException(
                    "Error message is not present. `isError()` has to be called before invoking"
                        + " `getErrorMessage()`"))
        .getMessage();
  }

  @Override
  public String toString() {
    return "InstallResult{"
        + "deviceMetadata="
        + deviceMetadata
        + ", installError="
        + installError.orElse(null)
        + '}';
  }
}
