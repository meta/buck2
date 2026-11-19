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
import com.google.common.base.Preconditions;
import com.google.common.base.Strings;
import java.util.Objects;
import org.jspecify.annotations.Nullable;

/** Install id. Typically represents fully qualified name of the build target. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class InstallId {

  private final String value;

  private InstallId(final String value) {
    this.value = value;
  }

  public String getValue() {
    return value;
  }

  @Override
  public int hashCode() {
    return Objects.hash(value);
  }

  @Override
  public boolean equals(@Nullable Object obj) {
    if (this == obj) {
      return true;
    }
    if (obj == null || getClass() != obj.getClass()) {
      return false;
    }
    return value.equals(((InstallId) obj).value);
  }

  public static InstallId of(String value) {
    Preconditions.checkState(!Strings.isNullOrEmpty(value), "install id has to be not empty");
    return new InstallId(value);
  }
}
