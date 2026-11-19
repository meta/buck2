/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd.command.kotlin

enum class KotlinSupportedLanguageVersion(val value: String) {
  v1_6("1.6"),
  V2_0("2.0"),
  V2_1("2.1"),
}

@RequiresOptIn(level = RequiresOptIn.Level.ERROR) annotation class LanguageVersionForLogs

class LanguageVersion(private val internalValue: String) {

  init {
    validateLanguageVersion()
  }

  private fun validateLanguageVersion() {
    val versionComponents = internalValue.split(".")
    require(versionComponents.size in (2..3)) { "Invalid language version: $internalValue" }
    require(versionComponents.all { it.toIntOrNull() != null }) {
      "Invalid language version: $internalValue"
    }
  }

  @LanguageVersionForLogs val valueForLogs: String? = internalValue

  val value: String
    get() {
      check(supportsLanguageVersion) { "Language version is not supported" }
      return internalValue
    }

  val compilerArgs: String
    get() = LANGUAGE_VERSION_ARG + value

  fun isGreaterOrEqual(version: KotlinSupportedLanguageVersion): Boolean {
    // Compare numerically per version component so that e.g. "2.10" >= "2.2" holds.
    // A plain string comparison is lexicographic and breaks once a minor reaches 2 digits.
    val current = internalValue.split(".")
    val target = version.value.split(".")
    for (i in 0 until maxOf(current.size, target.size)) {
      val c = current.getOrNull(i)?.toIntOrNull() ?: 0
      val t = target.getOrNull(i)?.toIntOrNull() ?: 0
      if (c != t) return c > t
    }
    return true
  }

  // Kotlinc 1.6+ can properly recognize `-language-version` flag
  val supportsLanguageVersion: Boolean = isGreaterOrEqual(LANGUAGE_VERSION_PARAM_SUPPORTED_FROM)

  val supportsK2: Boolean = isGreaterOrEqual(KotlinSupportedLanguageVersion.V2_0)

  companion object {
    private val LANGUAGE_VERSION_PARAM_SUPPORTED_FROM = KotlinSupportedLanguageVersion.v1_6
    private const val LANGUAGE_VERSION_ARG: String = "-language-version="
    @JvmStatic val K2: LanguageVersion = LanguageVersion(KotlinSupportedLanguageVersion.V2_0.value)
  }
}
