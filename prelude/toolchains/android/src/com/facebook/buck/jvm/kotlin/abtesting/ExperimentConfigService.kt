/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.abtesting

import java.util.ServiceLoader

interface ExperimentConfigService {

  fun loadConfig(universeName: String): ExperimentConfig

  companion object {
    @JvmStatic
    fun loadImplementation(): ExperimentConfigService {
      val implementations = ServiceLoader.load(ExperimentConfigService::class.java)
      implementations.firstOrNull()
          ?: error(
              "The classpath contains no implementation for ${ExperimentConfigService::class.qualifiedName}",
          )
      return implementations.singleOrNull()
          ?: error(
              "The classpath contains more than one implementation for ${ExperimentConfigService::class.qualifiedName}",
          )
    }
  }
}
