/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use dupe::Dupe;

use crate::digest::*;
use crate::error::*;

#[derive(Clone, Default)]
pub struct TTimestamp {
    pub seconds: i64,
    pub nanos: i32,
    pub _dot_dot_default: (),
}

impl TTimestamp {
    /// Returns the amount of time elapsed from another timestamp to this one,
    /// or zero duration if that timestamp is later than this one.
    pub fn saturating_duration_since(&self, earlier: &TTimestamp) -> std::time::Duration {
        let mut seconds = self.seconds - earlier.seconds;
        let mut nanos = self.nanos - earlier.nanos;
        if seconds < 0 && nanos > 0 {
            seconds += 1;
            nanos -= 1000000000;
        } else if seconds > 0 && nanos < 0 {
            seconds -= 1;
            nanos += 1000000000;
        }
        std::time::Duration::new(
            seconds.try_into().unwrap_or_default(),
            nanos.try_into().unwrap_or_default(),
        )
    }

    pub fn unix_epoch() -> Self {
        Self {
            seconds: 0,
            nanos: 0,
            ..Default::default()
        }
    }
}

#[derive(Clone, Default)]
pub struct TExecutedActionMetadata {
    pub worker: String,
    pub queued_timestamp: TTimestamp,
    pub worker_start_timestamp: TTimestamp,
    pub worker_completed_timestamp: TTimestamp,
    pub input_fetch_start_timestamp: TTimestamp,
    pub input_fetch_completed_timestamp: TTimestamp,
    pub execution_start_timestamp: TTimestamp,
    pub execution_completed_timestamp: TTimestamp,
    pub output_upload_start_timestamp: TTimestamp,
    pub output_upload_completed_timestamp: TTimestamp,
    pub execution_dir: String,
    pub input_analyzing_start_timestamp: TTimestamp,
    pub input_analyzing_completed_timestamp: TTimestamp,
    pub execution_attempts: i32,
    pub last_queued_timestamp: TTimestamp,
    pub instruction_counts: TPerfCount,
    pub auxiliary_metadata: Vec<TAny>,
    pub max_used_mem: i64,
    pub cpu_stat_user_usec: i64,
    pub cpu_stat_system_usec: i64,
    pub executed_on_elastic_capacity: bool,
    // Compatibility with the Thrift structs
    pub _dot_dot_default: (),
}

#[derive(Clone, Default)]
pub struct TPerfCount {
    pub kernel_events: TSubsysPerfCount,
    pub userspace_events: TSubsysPerfCount,
    // Compatibility with the Thrift structs
    pub _dot_dot_default: (),
}

#[derive(Clone, Default)]
pub struct TSubsysPerfCount {
    pub count: i64,
    pub time_enabled: i64,
    pub time_running: i64,
    // Compatibility with the Thrift structs
    pub _dot_dot_default: (),
}

#[derive(Clone, Default)]
pub struct TActionResult2 {
    pub output_files: Vec<TFile>,
    pub output_directories: Vec<TDirectory2>,
    pub exit_code: i32,
    pub stdout_raw: Option<Vec<u8>>,
    pub stdout_digest: Option<TDigest>,
    pub stderr_raw: Option<Vec<u8>>,
    pub stderr_digest: Option<TDigest>,
    pub execution_metadata: Box<TExecutedActionMetadata>,
    pub auxiliary_metadata: Vec<TAny>,
    pub output_symlinks: Vec<TSymlink>,
    // Compatibility with the Thrift structs
    pub _dot_dot_default: (),
}

#[derive(Clone, Default)]
pub struct TAny {
    pub type_url: String,
    pub value: Vec<u8>,
    // Compatibility with the Thrift structs
    pub _dot_dot_default: (),
}

#[derive(Clone, Default)]
pub struct ActionResultResponse {
    pub action_result: TActionResult2,
    pub ttl: i64,
}

#[derive(Clone, Default)]
pub struct WriteActionResultResponse {
    pub actual_action_result: TActionResult2,
    pub ttl_seconds: i64,
}

#[derive(Clone, Default)]
pub struct DownloadResponse {
    pub inlined_blobs: Option<Vec<InlinedDigestWithStatus>>,
    pub directories: Option<Vec<DigestWithStatus>>,
    pub local_cache_stats: TLocalCacheStats,
}

#[derive(Clone, Default)]
pub struct InlinedDigestWithStatus {
    pub digest: TDigest,
    pub status: TStatus,
    pub blob: Vec<u8>,
}

#[derive(Clone, Default)]
pub struct DigestWithStatus {
    pub digest: TDigest,
    pub status: TStatus,
    // Compatibility with the Thrift structs
    pub _dot_dot_default: (),
}

#[derive(Clone, Default)]
pub struct CacheFunnelStats {
    pub digests_served_from_memory: i64,
    pub digests_served_from_fs: i64,
    pub _dot_dot_default: (),
}

#[derive(Clone, Default)]
pub struct TLocalCacheStats {
    pub total_cache_lookup_attempts: i64,
    pub hits_files: i64,
    pub hits_bytes: i64,
    pub misses_files: i64,
    pub misses_bytes: i64,
    pub cache_lookup_latency_microseconds: i64,
    pub cache_funnel_stats: CacheFunnelStats,
    // Compatibility with the Thrift structs
    pub _dot_dot_default: (),
}

#[derive(Debug, Clone, Default)]
pub struct TStatus {
    pub code: TCode,
    pub message: String,
    // Compatibility with the Thrift structs
    pub _dot_dot_default: (),
}

#[derive(Clone, Default)]
pub struct FindMissingBlobsResponse {
    pub missing_digests: Vec<TDigest>,
}

#[derive(Clone, Default)]
pub struct DigestWithTtl {
    pub digest: TDigest,
    pub ttl: i64,
}

#[derive(Clone, Default)]
pub struct GetDigestsTtlResponse {
    pub digests_with_ttl: Vec<DigestWithTtl>,
}

#[derive(Clone, Default)]
pub struct ExecuteResponse {
    pub action_result: TActionResult2,
    pub status: TStatus,
    pub cached_result: bool,
    pub action_digest: TDigest,
    pub action_result_digest: TDigest,
    pub action_result_ttl: i64,
}

#[derive(Clone, Default)]
pub struct ExecutedActionStorageStats {
    pub downloads: TStorageStats,
    pub uploads: TStorageStats,
    pub inputs_downloads: TStorageStats,
    pub inputs_uploads: TStorageStats,
    pub outputs_downloads: TStorageStats,
    pub outputs_uploads: TStorageStats,
    pub local_cache_stats: TLocalCacheStats,
}

#[derive(Clone, Default)]
pub struct ExecutedActionMemoryStats {
    pub max_used_mem: i64,
    pub reserved_mem: i64,
}

#[derive(Clone, Default)]
pub struct TaskInfo {
    pub estimated_queue_time_ms: i64,
    pub new_estimated_queue_time_ms: i64,
    pub state: TaskState,
}

#[derive(Clone)]
#[allow(non_camel_case_types)]
pub enum TaskState {
    enqueued(EnqueuedTaskState),
    waiting_on_reservation(WaitingOnReservationTaskState),
    no_worker_available(NoAgentAvailableTaskState),
    cancelled(CancelledTaskState),
    over_quota(OverQuotaTaskState),
    acquiring_dependencies(AcquiringDependenciesTaskState),
    waiting_for_gang_allocation(WaitingForGangAllocationTaskState),
    UnknownField(i32),
}

impl Default for TaskState {
    fn default() -> Self {
        Self::UnknownField(1)
    }
}

#[derive(Clone, Default)]
pub struct EnqueuedTaskState {}

#[derive(Clone, Default)]
pub struct NoAgentAvailableTaskState {}

#[derive(Clone, Default)]
pub struct CancelledTaskState {}

#[derive(Clone, Default)]
pub struct OverQuotaTaskState {
    pub exceeded_quotas: Vec<ExceededQuota>,
}

#[derive(Clone, Default)]
pub struct ExceededQuota {
    pub name: String,
    pub limit: i64,
    pub used: i64,
}

#[derive(Clone, Default)]
pub struct WaitingOnReservationTaskState {}

#[derive(Clone, Default)]
pub struct AcquiringDependenciesTaskState {}

#[derive(Clone, Default)]
pub struct WaitingForGangAllocationTaskState {}

#[derive(Clone, Default)]
pub struct OperationMetadata {
    pub action_digest: TDigest,
    pub stdout_stream_name: String,
    pub stderr_stream_name: String,
    pub task_info: Option<TaskInfo>,
}

#[derive(PartialEq, Eq, Debug, Clone, Dupe, Copy, Default)]
pub struct Stage(pub i32);

impl Stage {
    pub const UNKNOWN: Self = Stage(0i32);
    pub const CACHE_CHECK: Self = Stage(1i32);
    pub const QUEUED: Self = Stage(2i32);
    pub const EXECUTING: Self = Stage(3i32);
    pub const COMPLETED: Self = Stage(4i32);
    pub const MATERIALIZING_INPUT: Self = Stage(100i32);
    pub const UPLOADING_OUTPUT: Self = Stage(101i32);
    pub const KEEP_ALIVE: Self = Stage(102i32);
    pub const BEFORE_ACTION: Self = Stage(103i32);
    pub const AFTER_ACTION: Self = Stage(104i32);
    pub const WORKER_RECEIVED: Self = Stage(105i32);
}

#[derive(Clone, Default)]
pub struct ExecuteWithProgressResponse {
    pub stage: Stage,
    pub execute_response: Option<ExecuteResponse>,
    pub metadata: OperationMetadata,
}

#[derive(Clone, Debug, Dupe, Default)]
pub struct UploadResponse {}

#[derive(Clone, Default)]
pub struct TDirectory2 {
    pub path: String,
    pub tree_digest: TDigest,
    pub root_directory_digest: TDigest,
    // Compatibility with the Thrift structs
    pub _dot_dot_default: (),
}

#[derive(Clone, Default)]
pub struct TFile {
    pub digest: DigestWithStatus,
    pub name: String,
    pub existed: bool,
    pub executable: bool,
    pub ttl: i64,
    // Compatibility with the Thrift structs
    pub _dot_dot_default: (),
}

#[derive(Clone, Default)]
pub struct TSymlink {
    pub name: String,
    pub target: String,
    // Compatibility with the Thrift structs
    pub _dot_dot_default: (),
}

#[derive(Clone, Default)]
pub struct NetworkStatisticsResponse {
    pub uploaded: i64,
    pub downloaded: i64,
    pub download_storage_stats: TStorageStats,
    pub upload_storage_stats: TStorageStats,
    // Compatibility with the Thrift structs
    pub _dot_dot_default: (),
}

#[derive(Clone, Default)]
pub struct TStorageStats {
    // Compatibility with the Thrift structs
    pub _dot_dot_default: (),
}
