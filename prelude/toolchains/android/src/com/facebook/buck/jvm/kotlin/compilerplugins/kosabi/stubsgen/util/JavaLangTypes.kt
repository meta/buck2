/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util

import com.facebook.kotlin.compilerplugins.kosabi.common.JavaLangDefaultImports

object PlainJavaLangTypes : FTQCollection {
  /**
   * Returns the simple names of all types in this collection.
   *
   * @return A collection of simple type names defined in this class
   */
  override fun all(): Collection<String> = JavaLangDefaultImports.STATIC_NAMES
}
