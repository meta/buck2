/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi.source.api;

import com.facebook.infer.annotation.Nullsafe;

/** Thrown by our ABI generating plugin to stop the compiler. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class StopCompilation extends RuntimeException {}
