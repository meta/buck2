/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin;

import com.facebook.buck.core.util.log.Logger;
import com.facebook.buck.jvm.kotlin.buildtools.BuildToolsKotlinc;
import com.facebook.buck.jvm.kotlin.kotlinc.Kotlinc;
import com.facebook.infer.annotation.Nullsafe;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class KotlincFactory {

  private static final Logger LOG = Logger.get(KotlincFactory.class);

  public static Kotlinc create() {
    LOG.info("Kotlinc implementation used: " + BuildToolsKotlinc.class.getSimpleName());
    return new BuildToolsKotlinc();
  }
}
