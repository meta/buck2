/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.cd.analytics.logger.model

data class KotlinCDLogEntry(
    val time: Long,
    val eventTime: Double,
    val target: String,
    val subtarget: String,
    val buildUuid: String,
    val executionPlatform: String,
    val numJavaFiles: Long,
    val numKotlinFiles: Long,
    val incremental: Boolean,
    val mode: String?,
    val classpathChanges: String?,
    val step: String,
    val languageVersion: String?,
    val extras: String?,
    val addedAndModifiedFiles: Set<String>?,
    val removedFiles: Set<String>?,
    val numKotlinTokens: Long? = null,
    val numJavaTokens: Long? = null,
    /**
     * Wall-clock duration of the step this entry describes, in milliseconds.
     *
     * Null when the step does not time itself. Without this there is no step-level timing anywhere
     * in KotlinCD - `IsolatedStepsRunner.runStep` does not time steps, and
     * `buck2_action_command_perf` only sees the whole `kotlincd_jar` action, which bundles ksp2,
     * kotlinc, kosabi and copy/zip together.
     */
    val durationMs: Long? = null,
)
