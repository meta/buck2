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

    /// How long a local test that hit its timeout gets to exit after `SIGTERM` before it and
    /// everything left in its process group get `SIGKILL`. `None` kills immediately.
    pub test_timeout_grace_period: Option<Duration>,

    /// On Linux, kill the processes a local test left behind outside its process group, such as
    /// daemons, when it is killed. Only applies to tests that do not run in an action cgroup. Such
    /// a test adopts its orphaned descendants, which then stay zombies until it exits instead of
    /// being reaped by init.
    pub kill_detached_test_processes: bool,
}
