/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.kotlinc.incremental

enum class RebuildReason(val message: String) {
  NO_LAST_BUILD_CONFIGURATION("Last build configuration not found"),
  BUILD_CONFIGURATION_CHANGED("Build configuration has been changed"),
  NO_LAST_DEP_FILE("dep-file.txt not found"),
  NO_LAST_USED_JARS("used-jars.json not found"),
  NO_JVM_ABI_WORKING_DIR("jvm_abi_gen_working_dir not found"),
  KOTLIN_COMPILER_PLUGIN_CHANGED("Kotlin compiler plugin has been changed"),
  COMPILER_INPUT_FILES_CHANGED(
      "A Kotlin compiler plugin or a file that a compiler option names has been changed",
  ),
  KOTLIN_MODULE_MAPPINGS_CHANGED("Kotlin module mappings of a classpath jar have been changed"),
  INCREMENTAL_INPUTS_UNREADABLE("Inputs of the last successful round could not be compared"),
  APPLICABILITY_CLASSPATH_CHANGED("Kosabi applicability reduced classpath has been changed"),
  APPLICABILITY_ROUND_DID_NOT_FINISH("The last Kosabi applicability round did not finish"),
  APPLICABILITY_CHECKED_IN_FULL(
      "Kosabi applicability did not pass the incremental round; checking it in a full round",
  ),
  APPLICABILITY_LEDGER_NOT_FOUND("Kosabi applicability ledger not found"),
}
