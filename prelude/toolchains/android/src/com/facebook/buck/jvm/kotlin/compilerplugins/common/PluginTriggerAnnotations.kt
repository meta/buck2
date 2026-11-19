/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.compilerplugins.common

import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.config.CompilerConfigurationKey

/**
 * The allopen/noarg/sam-with-receiver trigger annotations named by this compilation's plugin
 * options (explicit `annotation=` values plus preset expansions). Both the abi-gen keep decision
 * and the applicability keepability rule subtract this set from the lint-only annotation allowlist,
 * so the two gates agree; anything unreadable degrades to no triggers.
 */
fun pluginTriggerAnnotations(configuration: CompilerConfiguration): Set<String> {
  val keysInstance =
      try {
        Class.forName("org.jetbrains.kotlin.allopen.AllOpenConfigurationKeys")
            .getField("INSTANCE")
            .get(null)
      } catch (_: ClassNotFoundException) {
        return emptySet()
      }
  fun keyValues(vararg memberNames: String): List<String> {
    val method =
        memberNames.firstNotNullOfOrNull { name ->
          runCatching { keysInstance.javaClass.getMethod("get$name") }.getOrNull()
        } ?: return emptyList()
    @Suppress("UNCHECKED_CAST")
    val key = method.invoke(keysInstance) as CompilerConfigurationKey<List<String>>
    return configuration.get(key).orEmpty()
  }
  val explicit = keyValues("ALLOPEN_ANNOTATION", "ANNOTATION")
  val presets = keyValues("ALLOPEN_PRESET", "PRESET")
  if (presets.isEmpty()) return explicit.toSet()
  val supportedPresets = allopenSupportedPresets()
  return (explicit + presets.flatMap { supportedPresets[it].orEmpty() }).toSet()
}

private fun allopenSupportedPresets(): Map<String, List<String>> =
    try {
      val names =
          Class.forName("org.jetbrains.kotlin.allopen.AllOpenPluginNames")
              .getField("INSTANCE")
              .get(null)
      @Suppress("UNCHECKED_CAST")
      names.javaClass.getMethod("getSUPPORTED_PRESETS").invoke(names) as Map<String, List<String>>
    } catch (_: Exception) {
      emptyMap()
    }
