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

/**
 * Thrown when a source ABI cannot be generated because the source code will not compile. Correct
 * handling is to simply stop attempting to generate the ABI and allow the compiler to fail.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class SourceCodeWillNotCompileException extends RuntimeException {}
