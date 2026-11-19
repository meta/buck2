/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::SystemTime;

use dupe::Dupe;

use crate::report::Report;

#[derive(Eq, PartialEq, Hash, Debug, Clone)]
pub enum HealthCheckType {
    MemoryPressure,
    LowDiskSpace,
    SlowDownloadSpeed,
    VpnEnabled,
    StableRevision,
    SlowBuild,
}

/// Trait to generalize a buck2 health check.
/// Refer <https://fburl.com/buck_health_checks> for details on adding a new health check.
#[async_trait::async_trait]
pub trait HealthCheck: Send + Sync {
    /// Returns an optional report when invoked at every `snapshot` event.
    /// Return value is interpreted as follows:
    /// `None`: Health check cannot run. e.g. not applicable for this command/target
    /// `tag: None and health_issue: None`: Health check ran but nothing to report (all healthy)
    /// `tag: Some/None and health_issue: Some/None`: The issue could either be reported to user on console, logged to scuba or both.
    fn run_check(
        &mut self,
        snapshot: HealthCheckSnapshotData,
    ) -> buck2_error::Result<Option<Report>>;

    /// Trigger when the health check context updates.
    /// The `run_check` method is executed repeatedly at every snapshot and should be optimized.
    /// This trigger can be used to precompute/cache relevant data.
    async fn handle_context_update(&mut self, context: &HealthCheckContext);
}

/// Trait to generalize a health check service e.g. in-process, out-of-process over gRPC.
#[async_trait::async_trait]
pub trait HealthCheckService: Sync + Send {
    /// Update the context for the health check service.
    async fn update_context(&mut self, event: HealthCheckContextEvent) -> buck2_error::Result<()>;

    /// Run all registered health checks.
    async fn run_checks(
        &mut self,
        snapshot: HealthCheckSnapshotData,
    ) -> buck2_error::Result<Vec<Report>>;
}

/// A subset of the client data that is relevant for health checks.
/// This is intentionally kept as a small set to avoid serialization costs.
#[derive(Default)]
pub struct HealthCheckContext {
    /// Data from the command start.
    /// Example use: Run a check only on a subset of commands.
    pub command_data: Option<buck2_data::command_start::Data>,

    pub trace_id: Option<String>,

    pub command_start_time: Option<SystemTime>,

    /// Target patterns.
    /// Example use: Project/target specific checks, target specific configs e.g. warm revision.
    pub parsed_target_patterns: Option<buck2_data::ParsedTargetPatterns>,

    /// Revision hash of the mergebase.
    /// Example use: Warm revision check.
    pub branched_from_revision: Option<String>,

    /// Denotes if this command is seeing cache miss without any file changes.
    pub has_excess_cache_misses: bool,

    /// Configurations for health check experiments.
    pub experiment_configurations: Option<buck2_data::SystemInfo>,

    /// Test override for slow build threshold (seconds). When set, bypasses
    /// the normal p99 Scuba lookup and uses this value instead.
    pub test_slow_build_threshold_secs: Option<u64>,
}

/// A subset of the Snapshot data specifically for health check use.
/// This struct contains timing metrics extracted from buck2_data::Snapshot.
#[derive(Dupe, Clone)]
pub struct HealthCheckSnapshotData {
    /// Timestamp when the snapshot was created
    pub timestamp: SystemTime,
}

/// An event from the daemon event subscriber to the health check client.
#[allow(clippy::large_enum_variant)]
pub enum HealthCheckEvent {
    HealthCheckContextEvent(HealthCheckContextEvent),
    // This snapshot passes a subset of the buck2_data::Snapshot data to health checks.
    // Contains timing metrics and other relevant data for health check analysis.
    Snapshot(HealthCheckSnapshotData),
}

/// An event to trigger update of context in the health check server.
/// This may result in side effects like precomputing data, etc. in health checks.
pub enum HealthCheckContextEvent {
    CommandStart(buck2_data::CommandStartWithTraceId),
    ParsedTargetPatterns(buck2_data::ParsedTargetPatterns),
    BranchedFromRevision(String),
    /// Sent only once and communicates if buck2 is experiencing excess cache misses.
    HasExcessCacheMisses(),
    /// Configuration about the health checks.
    ExperimentConfigurations(buck2_data::SystemInfo),
    /// Test override for slow build threshold (in seconds).
    /// Set via BUCK2_TEST_SLOW_BUILD_CHECK env var on the client.
    TestSlowBuildThreshold(u64),
}
