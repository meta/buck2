/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.cd.analytics

import com.facebook.buck.jvm.cd.command.kotlin.LanguageVersion

/** A logging context designed to accumulate data relevant to KotlinCD operations. */
class KotlinCDLoggingContext(
    val step: StepParam,
    val languageVersion: LanguageVersion,
    val mode: ModeParam?,
) {

  val classpathChangesParam = (mode as? ModeParam.Incremental)?.classpathChangesParam

  var numKotlinTokens: Long = 0L
  var numJavaTokens: Long = 0L

  /** Wall-clock duration of the step, in milliseconds; null when the step does not time itself. */
  var durationMs: Long? = null

  private val _extras: MutableMap<String, MutableList<String>> = mutableMapOf()
  val extras: Map<String, List<String>>
    get() = _extras

  fun addExtras(key: String, value: String) {
    _extras.computeIfAbsent(key) { mutableListOf() }.add(value)
  }
}
