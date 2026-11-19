/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.ksp

import com.facebook.buck.jvm.cd.command.kotlin.LanguageVersion
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDLoggingContext
import com.facebook.buck.jvm.kotlin.cd.analytics.ModeParam
import com.facebook.buck.jvm.kotlin.cd.analytics.StepParam

internal fun KotlinCDLoggingContext(
    languageVersion: LanguageVersion,
    durationMs: Long? = null,
): KotlinCDLoggingContext = KotlinCDLoggingContext(
    step = StepParam.KSP2,
    languageVersion = languageVersion,
    mode = ModeParam.NonIncremental,
)
    .apply {
      this.durationMs = durationMs
    }
