/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use dupe::Dupe;

/// Command-level config that can tweak how the executors work.
#[derive(Clone, Dupe, Default)]
pub struct ExecutorGlobalKnobs {
    pub enable_miniperf: bool,

    /// Whether to emit action keys to execution logs (thos are pretty verbose and omitted by
    /// default).
    pub log_action_keys: bool,

    /// Maximum duration in seconds that an execution can remain in the RE queue state before it is cancelled.
    pub re_cancel_on_estimated_queue_time_exceeds: Option<Duration>,

    /// Maximum duration in seconds that an execution can remain in the RE queue state before local execution is unblocked.
    /// Note this overrides, and should possibly replace, `remote_execution_queue_time_threshold_s` configured per executor.
    pub re_fallback_on_estimated_queue_time_exceeds: Option<Duration>,
}
