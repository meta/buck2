/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java;

import com.facebook.infer.annotation.Nullsafe;
import java.util.Collection;

@Nullsafe(Nullsafe.Mode.LOCAL)
public interface OptionsConsumer {
  void addOptionValue(String option, String value);

  void addFlag(String flagName);

  void addExtras(Collection<String> extras);
}
