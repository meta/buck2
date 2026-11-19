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
use std::time::SystemTime;

use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::soft_error_or_panic;
use buck2_data::timing::Micros;
use buck2_error::BuckErrorContext;
use buck2_miniperf_proto::MiniperfCounter;
use remote_execution::ActionResultResponse;
use remote_execution::ExecuteResponse;
use remote_execution::TDirectory2;
use remote_execution::TExecutedActionMetadata;
use remote_execution::TFile;
use remote_execution::TSubsysPerfCount;
use remote_execution::TSymlink;
use remote_execution::TTimestamp;

use crate::digest_config::DigestConfig;
use crate::execute::kind::CommandExecutionKind;
use crate::execute::kind::RemoteCommandExecutionDetails;
use crate::execute::kind::RemoteExecutionOrigin;
use crate::re::manager::ManagedRemoteExecutionClient;
use crate::re::queue_stats::QueueStats;
use crate::re::streams::RemoteCommandStdStreams;

pub struct ActionCacheResult(pub ActionResultResponse, pub buck2_data::CacheType);

pub trait RemoteActionResult: Send + Sync {
    fn output_files(&self) -> &[TFile];
    fn output_directories(&self) -> &[TDirectory2];
    fn output_symlinks(&self) -> &[TSymlink];

    fn execution_kind(&self, details: RemoteCommandExecutionDetails) -> CommandExecutionKind;

    /// This is only called after we inspect the action result, and the exit code is not 0
    fn execution_kind_for_failed_actions(
        &self,
        details: RemoteCommandExecutionDetails,
        materialized_inputs_for_failed: Option<Vec<ProjectRelativePathBuf>>,
        materialized_outputs_for_failed_actions: Option<Vec<ProjectRelativePathBuf>>,
    ) -> CommandExecutionKind;

    fn timing(&self) -> ReMetadataTiming;

    fn std_streams(
        &self,
        client: &ManagedRemoteExecutionClient,
        digest_config: DigestConfig,
    ) -> RemoteCommandStdStreams;

    /// The TTL given by RE for the outputs for this action.
    fn ttl(&self) -> i64;
}

pub struct ExecuteResponseWithQueueStats {
    pub execute_response: ExecuteResponse,
    pub queue_stats: QueueStats,
}

impl ExecuteResponseWithQueueStats {
    /// How the remote execution service produced this result.
    pub fn origin(&self) -> RemoteExecutionOrigin {
        #[cfg(fbcode_build)]
        let (served_from_cache, deduplicated) = {
            let details = &self.execute_response.executed_action_details;
            (details.was_served_from_cache, details.was_deduplicated)
        };
        #[cfg(not(fbcode_build))]
        let (served_from_cache, deduplicated) = (self.execute_response.cached_result, false);
        if served_from_cache {
            RemoteExecutionOrigin::ServedFromCache
        } else if deduplicated {
            RemoteExecutionOrigin::Deduplicated
        } else {
            RemoteExecutionOrigin::Executed
        }
    }

    /// What the worker reported, if RE ran the action for this request.
    fn re_worker(&self) -> Option<Box<buck2_data::ReWorkerDetails>> {
        if self.origin() != RemoteExecutionOrigin::Executed {
            return None;
        }
        re_worker_details(&self.execute_response.action_result.execution_metadata).map(Box::new)
    }
}

impl RemoteActionResult for ExecuteResponseWithQueueStats {
    fn output_files(&self) -> &[TFile] {
        &self.execute_response.action_result.output_files
    }

    fn output_directories(&self) -> &[TDirectory2] {
        &self.execute_response.action_result.output_directories
    }

    fn output_symlinks(&self) -> &[TSymlink] {
        &self.execute_response.action_result.output_symlinks
    }

    fn execution_kind(&self, details: RemoteCommandExecutionDetails) -> CommandExecutionKind {
        self.execution_kind_for_failed_actions(details, None, None)
    }

    fn execution_kind_for_failed_actions(
        &self,
        details: RemoteCommandExecutionDetails,
        materialized_inputs_for_failed: Option<Vec<ProjectRelativePathBuf>>,
        materialized_outputs_for_failed_actions: Option<Vec<ProjectRelativePathBuf>>,
    ) -> CommandExecutionKind {
        CommandExecutionKind::Remote {
            details,
            queue_time: self.queue_stats.cumulative_queue_duration,
            materialized_inputs_for_failed,
            materialized_outputs_for_failed_actions,
            origin: self.origin(),
            re_worker: self.re_worker(),
        }
    }

    fn timing(&self) -> ReMetadataTiming {
        timing_from_re_metadata(&self.execute_response.action_result.execution_metadata)
    }

    fn std_streams(
        &self,
        client: &ManagedRemoteExecutionClient,
        digest_config: DigestConfig,
    ) -> RemoteCommandStdStreams {
        RemoteCommandStdStreams::new(&self.execute_response.action_result, client, digest_config)
    }

    fn ttl(&self) -> i64 {
        self.execute_response.action_result_ttl
    }
}

impl RemoteActionResult for ActionCacheResult {
    fn output_files(&self) -> &[TFile] {
        &self.0.action_result.output_files
    }

    fn output_directories(&self) -> &[TDirectory2] {
        &self.0.action_result.output_directories
    }

    fn output_symlinks(&self) -> &[TSymlink] {
        &self.0.action_result.output_symlinks
    }

    fn execution_kind(&self, details: RemoteCommandExecutionDetails) -> CommandExecutionKind {
        match self.1 {
            buck2_data::CacheType::ActionCache => CommandExecutionKind::ActionCache { details },
            buck2_data::CacheType::RemoteDepFileCache => {
                CommandExecutionKind::RemoteDepFileCache { details }
            }
        }
    }

    fn execution_kind_for_failed_actions(
        &self,
        details: RemoteCommandExecutionDetails,
        _materialized_inputs_for_failed: Option<Vec<ProjectRelativePathBuf>>,
        _materialized_outputs_for_failed_actions: Option<Vec<ProjectRelativePathBuf>>,
    ) -> CommandExecutionKind {
        self.execution_kind(details)
    }

    fn timing(&self) -> ReMetadataTiming {
        let mut timing = timing_from_re_metadata(&self.0.action_result.execution_metadata);
        timing.input_materialization_duration = Duration::ZERO;
        timing.queue_duration = None;
        timing
    }

    fn std_streams(
        &self,
        client: &ManagedRemoteExecutionClient,
        digest_config: DigestConfig,
    ) -> RemoteCommandStdStreams {
        RemoteCommandStdStreams::new(&self.0.action_result, client, digest_config)
    }

    fn ttl(&self) -> i64 {
        self.0.ttl
    }
}

pub struct ReMetadataTiming {
    pub execution_time: Duration,
    pub start_time: SystemTime,
    pub execution_stats: Option<buck2_data::CommandExecutionStats>,
    pub input_materialization_duration: Duration,
    pub queue_duration: Option<Duration>,
}

fn timing_from_re_metadata(meta: &TExecutedActionMetadata) -> ReMetadataTiming {
    let execution_time = meta
        .execution_completed_timestamp
        .saturating_duration_since(&meta.execution_start_timestamp);

    let start_time = SystemTime::UNIX_EPOCH
        + meta
            .execution_start_timestamp
            .saturating_duration_since(&TTimestamp::unix_epoch());

    let execution_stats = match convert_perf_counts(meta) {
        Ok(v) => Some(v),
        Err(e) => {
            soft_error_or_panic!(
                "re_invalid_instruction_counts",
                e.context("Invalid instruction counts received from RE")
            );
            None
        }
    };

    let fetch_input_time = meta
        .input_fetch_completed_timestamp
        .saturating_duration_since(&meta.input_fetch_start_timestamp);

    let queue_duration = meta
        .worker_start_timestamp
        .saturating_duration_since(&meta.queued_timestamp);

    ReMetadataTiming {
        execution_time,
        start_time,
        execution_stats,
        input_materialization_duration: fetch_input_time,
        queue_duration: Some(queue_duration),
    }
}

/// What the worker reported in `meta`, or `None` if it reported nothing.
fn re_worker_details(meta: &TExecutedActionMetadata) -> Option<buck2_data::ReWorkerDetails> {
    // RE reports zero both for a real zero and for "not reported".
    let positive = |value: i64| u64::try_from(value).ok().filter(|v| *v > 0);
    let positive_us =
        |value: i64| positive(value).map(|us| Micros::from(Duration::from_micros(us)).into());
    let duration_us = |start: &TTimestamp, end: &TTimestamp| {
        end.duration_since_if_set(start)
            .map(|d| Micros::from(d).into())
    };
    let details = buck2_data::ReWorkerDetails {
        worker_wall_time_us: duration_us(
            &meta.worker_start_timestamp,
            &meta.worker_completed_timestamp,
        ),
        output_upload_us: duration_us(
            &meta.output_upload_start_timestamp,
            &meta.output_upload_completed_timestamp,
        ),
        elastic_capacity: meta.executed_on_elastic_capacity,
        cpu_time_user_us: positive_us(meta.cpu_stat_user_usec),
        cpu_time_system_us: positive_us(meta.cpu_stat_system_usec),
        attempts: u32::try_from(meta.execution_attempts)
            .ok()
            .filter(|count| *count > 1)
            .map(|count| buck2_data::ReAttempts {
                count,
                // Stamped by the RE engine and by the worker, so clock skew can make it negative.
                final_queue_us: meta
                    .worker_start_timestamp
                    .duration_since_if_set(&meta.last_queued_timestamp)
                    .map(|d| Micros::from(d).into()),
            }),
        input_download_bytes: positive(meta.storage_stats.inputs_downloads.total_bytes),
        output_upload_bytes: positive(meta.storage_stats.outputs_uploads.total_bytes),
    };
    (details != buck2_data::ReWorkerDetails::default()).then_some(details)
}

/// Durations between RE's timestamps.
trait DurationSinceIfSet {
    /// The time from `earlier` to this timestamp, or `None` if either is unset (RE leaves the
    /// stamps of phases it did not record at zero) or `earlier` is later, so that neither case
    /// reads as a plausible duration.
    fn duration_since_if_set(&self, earlier: &TTimestamp) -> Option<Duration>;
}

impl DurationSinceIfSet for TTimestamp {
    fn duration_since_if_set(&self, earlier: &TTimestamp) -> Option<Duration> {
        let unset = |t: &TTimestamp| t.seconds == 0 && t.nanos == 0;
        if unset(self)
            || unset(earlier)
            || (self.seconds, self.nanos) < (earlier.seconds, earlier.nanos)
        {
            return None;
        }
        Some(self.saturating_duration_since(earlier))
    }
}

fn convert_perf_counts(
    meta: &TExecutedActionMetadata,
) -> buck2_error::Result<buck2_data::CommandExecutionStats> {
    Ok({
        let userspace_counter = convert_perf_count(&meta.instruction_counts.userspace_events)?;
        let kernel_counter = convert_perf_count(&meta.instruction_counts.kernel_events)?;
        buck2_data::CommandExecutionStats {
            cpu_instructions_user: userspace_counter.map(|p| p.adjusted_count()),
            cpu_instructions_kernel: kernel_counter.map(|p| p.adjusted_count()),
            userspace_events: userspace_counter.map(Into::into),
            kernel_events: kernel_counter.map(Into::into),
            memory_peak: Some(meta.max_used_mem as u64),
        }
    })
}

fn convert_perf_count(
    perf_count: &TSubsysPerfCount,
) -> buck2_error::Result<Option<MiniperfCounter>> {
    if perf_count.time_running == 0 {
        return Ok(None);
    }

    Ok(Some(MiniperfCounter {
        count: perf_count
            .count
            .try_into()
            .buck_error_context("Invalid count")?,
        time_enabled: perf_count
            .time_enabled
            .try_into()
            .buck_error_context("Invalid time_enabled")?,
        time_running: perf_count
            .time_running
            .try_into()
            .buck_error_context("Invalid time_running")?,
    }))
}

#[cfg(test)]
mod tests {
    use remote_execution::TActionResult2;

    use super::*;

    fn ts(seconds: i64) -> TTimestamp {
        TTimestamp {
            seconds,
            nanos: 0,
            ..Default::default()
        }
    }

    fn executed_metadata() -> TExecutedActionMetadata {
        TExecutedActionMetadata {
            execution_start_timestamp: ts(110),
            execution_completed_timestamp: ts(140),
            worker_start_timestamp: ts(100),
            worker_completed_timestamp: ts(160),
            output_upload_start_timestamp: ts(150),
            output_upload_completed_timestamp: ts(158),
            cpu_stat_user_usec: 400,
            cpu_stat_system_usec: 100,
            executed_on_elastic_capacity: true,
            execution_attempts: 2,
            last_queued_timestamp: ts(95),
            ..Default::default()
        }
    }

    #[test]
    fn test_worker_details_come_from_re_metadata() {
        let details = re_worker_details(&executed_metadata()).unwrap();
        assert_eq!(
            details.worker_wall_time_us,
            Some(60_000_000),
            "worker receipt to completion"
        );
        assert_eq!(
            details.output_upload_us,
            Some(8_000_000),
            "upload start to completion"
        );
        assert!(details.elastic_capacity);
        assert_eq!(
            (details.cpu_time_user_us, details.cpu_time_system_us),
            (Some(400), Some(100))
        );
    }

    #[test]
    fn test_unrecorded_or_reversed_stamps_are_absent() {
        for (start, end, expected) in [
            (0, 158, None),
            (150, 0, None),
            (158, 150, None),
            (150, 150, Some(Duration::ZERO)),
        ] {
            assert_eq!(
                ts(end).duration_since_if_set(&ts(start)),
                expected,
                "stamps {start}..{end}: an unset stamp would otherwise read as decades, a reversed pair as zero"
            );
        }
        assert_eq!(
            re_worker_details(&TExecutedActionMetadata::default()),
            None,
            "RE reports zero CPU time and unset stamps when it has nothing to report"
        );
    }

    #[test]
    fn test_reused_results_have_no_worker_details() {
        let mut response = ExecuteResponseWithQueueStats {
            execute_response: ExecuteResponse {
                action_result: TActionResult2 {
                    execution_metadata: Box::new(executed_metadata()),
                    ..Default::default()
                },
                ..Default::default()
            },
            queue_stats: QueueStats::default(),
        };
        assert!(response.re_worker().is_some(), "a real execution has them");

        #[cfg(fbcode_build)]
        {
            response
                .execute_response
                .executed_action_details
                .was_deduplicated = true;
        }
        #[cfg(not(fbcode_build))]
        {
            response.execute_response.cached_result = true;
        }
        assert_eq!(
            response.re_worker(),
            None,
            "the metadata describes the original execution"
        );
    }

    #[test]
    fn test_attempts_final_queue_and_storage() {
        let mut meta = executed_metadata();
        let mut storage = (*meta.storage_stats).clone();
        storage.inputs_downloads.total_bytes = 1_234_567;
        storage.outputs_uploads.total_bytes = 2000;
        meta.storage_stats = storage.into();
        let details = re_worker_details(&meta).unwrap();
        assert_eq!(
            details.attempts,
            Some(buck2_data::ReAttempts {
                count: 2,
                final_queue_us: Some(5_000_000),
            }),
            "the final attempt queued from last queued to worker receipt"
        );
        assert_eq!(
            (details.input_download_bytes, details.output_upload_bytes),
            (Some(1_234_567), Some(2000)),
            "byte counts are exact, unlike the two-significant-figure times"
        );

        let single = TExecutedActionMetadata {
            execution_attempts: 1,
            ..executed_metadata()
        };
        let single = re_worker_details(&single).unwrap();
        assert_eq!(
            single.attempts, None,
            "one attempt is the common case, and its queue time is the one recorded already"
        );

        let skewed = TExecutedActionMetadata {
            last_queued_timestamp: ts(101),
            ..executed_metadata()
        };
        assert_eq!(
            re_worker_details(&skewed).unwrap().attempts,
            Some(buck2_data::ReAttempts {
                count: 2,
                final_queue_us: None,
            }),
            "the two ends come from different machines, so skew leaves the queue time unset"
        );
    }
}
