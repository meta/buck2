/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:Suppress("PackageLocationMismatch")

package com.facebook

import org.jetbrains.kotlin.fir.FirModuleData
import org.jetbrains.kotlin.fir.FirSourceModuleData
import org.jetbrains.kotlin.name.Name
import org.jetbrains.kotlin.platform.TargetPlatform

fun createSourceModuleData(
    name: Name,
    dependencies: List<FirModuleData>,
    dependsOnDependencies: List<FirModuleData>,
    friendDependencies: List<FirModuleData>,
    platform: TargetPlatform,
): FirModuleData {
  return FirSourceModuleData(
      name,
      dependencies,
      dependsOnDependencies,
      friendDependencies,
      platform,
  )
}
