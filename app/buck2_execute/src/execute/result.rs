/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::convert::Infallible;
use std::fmt::Display;
use std::ops::ControlFlow;
use std::ops::FromResidual;
use std::time::Duration;
use std::time::SystemTime;

use allocative::Allocative;
use buck2_action_metadata_proto::RemoteDepFile;
use buck2_build_signals::env::WaitingData;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_data::SchedulingMode;
use buck2_hash::BuckMutInsertionMap;
use buck2_util::time_span::TimeSpan;
use derivative::Derivative;
use dupe::Dupe;
use remote_execution::TActionResult2;

use crate::artifact_value::ArtifactValue;
use crate::execute::claim::Claim;
use crate::execute::dep_file_digest::DepFileDigest;
use crate::execute::kind::CommandExecutionKind;
use crate::execute::output::CommandStdStreams;
use crate::execute::request::CommandExecutionOutput;
use crate::execute::request::ResolvedCommandExecutionOutput;
use crate::materialize::materializer::ReadLease;
use crate::output_size::OutputSize;
use crate::re::remote_action_result::ReMetadataTiming;

#[derive(Debug)]
pub enum CommandExecutionErrorType {
    StorageResourceExhausted,
    Other,
}

#[derive(Debug)]
pub enum CommandCancellationReason {
    NotSpecified,
    ReQueueTimeout,
}

/// "Status" of an action execution indicating how it finished. E.g. "built_remotely", "local_fallback", "action_cache".
#[derive(Debug)]
pub enum CommandExecutionStatus {
    Success {
        execution_kind: CommandExecutionKind,
    },
    Failure {
        execution_kind: CommandExecutionKind,
        /// Whether a declared input was missing before this local failure.
        missing_materialized_inputs: bool,
    },
    WorkerFailure {
        execution_kind: CommandExecutionKind,
    },
    Error {
        stage: &'static str,
        error: buck2_error::Error,
        execution_kind: Option<CommandExecutionKind>,
        typ: CommandExecutionErrorType,
    },
    TimedOut {
        execution_kind: CommandExecutionKind,
        duration: Duration,
    },
    // TODO: We should rename this.
    Cancelled {
        execution_kind: CommandExecutionKind,
        reason: Option<CommandCancellationReason>,
    },
}

impl CommandExecutionStatus {
    pub fn execution_kind(&self) -> Option<&CommandExecutionKind> {
        match self {
            CommandExecutionStatus::Success { execution_kind, .. } => Some(execution_kind),
            CommandExecutionStatus::Failure { execution_kind, .. } => Some(execution_kind),
            CommandExecutionStatus::WorkerFailure { execution_kind } => Some(execution_kind),
            CommandExecutionStatus::Error { execution_kind, .. } => execution_kind.as_ref(),
            CommandExecutionStatus::TimedOut { execution_kind, .. } => Some(execution_kind),
            CommandExecutionStatus::Cancelled { execution_kind, .. } => Some(execution_kind),
        }
    }
}

impl Display for CommandExecutionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandExecutionStatus::Success { execution_kind, .. } => {
                write!(f, "success {execution_kind}",)
            }
            CommandExecutionStatus::WorkerFailure { execution_kind } => {
                write!(f, "worker failure {execution_kind}",)
            }
            CommandExecutionStatus::Failure { execution_kind, .. } => {
                write!(f, "failure {execution_kind}",)
            }
            CommandExecutionStatus::Error {
                stage,
                error,
                execution_kind: Some(execution_kind),
                ..
            } => {
                write!(f, "error {execution_kind}:{stage}\n{error:#}")
            }
            CommandExecutionStatus::Error {
                stage,
                error,
                execution_kind: None,
                ..
            } => {
                write!(f, "error:{stage}\n{error:#}")
            }
            CommandExecutionStatus::TimedOut { duration, .. } => {
                write!(f, "timed out after {:.3}s", duration.as_secs_f64())
            }
            CommandExecutionStatus::Cancelled {
                execution_kind,
                reason,
            } => {
                if let Some(reason) = reason {
                    write!(f, "Cancelled {execution_kind} due to {reason:?}")
                } else {
                    write!(f, "Cancelled {execution_kind}")
                }
            }
        }
    }
}

/// Unlike action where we only really have just 1 time, commands can have slightly richer timing
/// data.
#[derive(Debug, Copy, Clone, Dupe, Allocative)]
pub struct CommandExecutionMetadata {
    /// A TimeSpan covering the time that this build actually waited for this action to complete
    pub time_span: TimeSpan,

    /// How long this command actually took to execute. This can be different from the wall_time if
    /// this was e.g. an action cache hit, in which case this field would reflect how long the
    /// command took to actually execute but not how we had to wait for it.
    pub execution_time: Duration,

    /// When execution started.
    pub start_time: SystemTime,

    /// Additional stats.
    pub execution_stats: Option<buck2_data::CommandExecutionStats>,

    /// How long it took to materialize the action's inputs.
    pub input_materialization_duration: Duration,

    /// How long we spent hashing the action's inputs.
    pub hashing_duration: Duration,

    /// How many artifacts we hashed
    pub hashed_artifacts_count: u64,

    /// How long this command spent waiting to run
    pub queue_duration: Option<Duration>,

    pub suspend_duration: Option<Duration>,

    pub suspend_count: Option<u64>,
}

impl CommandExecutionMetadata {
    pub fn empty(time_span: TimeSpan) -> Self {
        Self {
            time_span,
            execution_time: Duration::default(),
            start_time: SystemTime::now(),
            execution_stats: None,
            input_materialization_duration: Duration::default(),
            hashing_duration: Duration::default(),
            hashed_artifacts_count: 0,
            queue_duration: None,
            suspend_count: None,
            suspend_duration: None,
        }
    }

    pub fn from_re_timing(re_timing: ReMetadataTiming, time_span: TimeSpan) -> Self {
        Self {
            time_span,
            execution_time: re_timing.execution_time,
            start_time: re_timing.start_time,
            execution_stats: re_timing.execution_stats,
            input_materialization_duration: re_timing.input_materialization_duration,
            queue_duration: re_timing.queue_duration,
            hashing_duration: Default::default(),
            hashed_artifacts_count: 0,
            suspend_duration: None,
            suspend_count: None,
        }
    }

    pub fn end_time(&self) -> SystemTime {
        self.start_time + self.time_span.duration()
    }

    pub fn to_proto(&self) -> buck2_data::CommandExecutionMetadata {
        let metadata = self.dupe();
        buck2_data::CommandExecutionMetadata {
            wall_time: metadata.time_span.duration().try_into().ok(),
            execution_time: metadata.execution_time.try_into().ok(),
            start_time: Some(metadata.start_time.into()),
            input_materialization_duration: metadata.input_materialization_duration.try_into().ok(),
            execution_stats: metadata.execution_stats,
            hashing_duration: metadata.hashing_duration.try_into().ok(),
            hashed_artifacts_count: metadata.hashed_artifacts_count,
            queue_duration: metadata.queue_duration.and_then(|d| d.try_into().ok()),
            suspend_duration: metadata.suspend_duration.and_then(|d| d.try_into().ok()),
            suspend_count: metadata.suspend_count,
            client_start_time: None,
        }
    }
}

/// CommandExecutionResult is the result of an executor executing a command.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct CommandExecutionResult {
    /// The outputs produced by this command
    pub outputs: BuckMutInsertionMap<CommandExecutionOutput, ArtifactValue>,
    /// How it executed.
    pub report: CommandExecutionReport,
    /// A previously rejected execution of this command.
    pub rejected_execution: Option<CommandExecutionReport>,
    /// Why the main action-result upload did or did not occur.
    pub cache_upload_result: buck2_data::UploadResult,
    /// Why dep file information for this action did or did not get uploaded to cache, by Buck2.
    pub dep_file_cache_upload_result: buck2_data::UploadResult,
    // Remote dep file key, if we did upload a dep file entry
    pub dep_file_key: Option<DepFileDigest>,
    /// Whether this command was eligible for hybrid execution.
    pub eligible_for_full_hybrid: bool,
    /// Execution metadata used for remote dep file lookups.
    /// This is picked up from the action result's auxiliary metadata and
    /// is used to verify the dep file cache lookup result
    pub dep_file_metadata: Option<RemoteDepFile>,
    /// If the action executed on RE, the original action result
    /// to be re-used when uploading the remote dep file.
    #[derivative(Debug = "ignore")]
    pub action_result: Option<TActionResult2>,
    /// Description of how local or remote execution were scheduled (currently only set by hybrid executor)
    pub scheduling_mode: Option<SchedulingMode>,

    /// Data about time spent waiting (not on critical path) during command execution.
    pub waiting_data: WaitingData,

    /// The outputs on this machine's disk, held for the readers that come after the run, the
    /// cache upload among them. `None` when the run did not report its outputs, or from an
    /// executor whose outputs are not on this machine's disk.
    pub held_outputs: Option<HeldOutputs>,
}

/// A run's outputs on disk, under a lease for their readers.
#[derive(Debug)]
pub struct HeldOutputs {
    pub lease: ReadLease,
    /// Where a content-based output among them is.
    pub content_based_at: ContentBasedOutputsAt,
}

/// Where a local run leaves a content-based output, which it writes at a placeholder path since
/// the content path is known only once the content is hashed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ContentBasedOutputsAt {
    /// At the placeholder path: the disk-state core's, whose consumers find content by digest.
    WrittenPath,
    /// At its content path, published there: the deferred core's, whose consumers copy from the
    /// declared path, and which keeps no reader out of the placeholder.
    ContentPath,
}

impl CommandExecutionResult {
    /// Total size of all outputs in bytes.
    pub fn calc_output_size_bytes(&self) -> u64 {
        self.outputs
            .values()
            .map(|v| v.calc_output_count_and_bytes(false).bytes)
            .sum()
    }

    pub fn was_success(&self) -> bool {
        matches!(self.report.status, CommandExecutionStatus::Success { .. })
    }

    pub fn was_served_by_remote_dep_file_cache(&self) -> bool {
        matches!(
            self.report.status,
            CommandExecutionStatus::Success {
                execution_kind: CommandExecutionKind::RemoteDepFileCache { .. },
            }
        )
    }

    pub fn was_remotely_executed(&self) -> bool {
        matches!(
            self.report.status,
            CommandExecutionStatus::Success {
                execution_kind: CommandExecutionKind::Remote { .. },
            }
        )
    }

    pub fn was_locally_executed(&self) -> bool {
        matches!(
            self.report.status,
            CommandExecutionStatus::Success {
                execution_kind: CommandExecutionKind::Local { .. },
            } | CommandExecutionStatus::Success {
                execution_kind: CommandExecutionKind::LocalWorker { .. },
            }
        )
    }

    pub fn was_action_cache_hit(&self) -> bool {
        matches!(
            self.report.status,
            CommandExecutionStatus::Success {
                execution_kind: CommandExecutionKind::ActionCache { .. },
            }
        )
    }

    /// Whether this is a local run that a hybrid executor chose at run time over a remote one:
    /// a fallback after the remote run failed, or a race the local run won. Where the command
    /// would run was not decided by its configuration, so the result is not one every build
    /// of the same command would have produced the same way.
    pub fn ran_locally_in_place_of_remote(&self) -> bool {
        self.was_locally_executed()
            && matches!(
                self.scheduling_mode,
                Some(
                    SchedulingMode::FullHybrid
                        | SchedulingMode::PreferRemote
                        | SchedulingMode::Fallback
                        | SchedulingMode::FallbackReQueueEstimate
                )
            )
    }

    /// Resolves the output path as used by execution, a stable path it can be
    /// accessed at without holding a lock, and the artifact value.
    ///
    /// The outputs, each with the path remote execution names it by, the placeholder path for a
    /// content-based one, the path its content is at on this machine, and its value. The local
    /// path is where the run holding the outputs (`held_outputs`) left them, safe to read for as
    /// long as this result is held; without such a run it is the content path, where a
    /// materialization puts a content-based output.
    pub fn resolve_outputs<'a>(
        &'a self,
        fs: &'a ArtifactFs,
    ) -> impl Iterator<
        Item = buck2_error::Result<(
            ResolvedCommandExecutionOutput,
            ProjectRelativePathBuf,
            &'a ArtifactValue,
        )>,
    > + 'a {
        let at_written_path = self
            .held_outputs
            .as_ref()
            .is_some_and(|held| held.content_based_at == ContentBasedOutputsAt::WrittenPath);
        self.outputs.iter().map(move |(output, value)| {
            let has_content_based_path = output.has_content_based_path();
            let resolved = output.as_ref().resolve(
                fs,
                if has_content_based_path {
                    Some(ContentBasedPathHash::OutputArtifact)
                } else {
                    None
                }
                .as_ref(),
            )?;
            let local_path = if has_content_based_path && !at_written_path {
                output
                    .as_ref()
                    .resolve(fs, Some(&value.content_based_path_hash()))?
                    .into_path()
            } else {
                resolved.path().to_buf()
            };
            Ok((resolved, local_path, value))
        })
    }
}

/// Describes how a command executed.
#[derive(Debug)]
pub struct CommandExecutionReport {
    pub claim: Option<Box<dyn Claim>>,
    pub status: CommandExecutionStatus,
    pub timing: CommandExecutionMetadata,
    pub std_streams: CommandStdStreams,
    /// No exit_code means the command did not finish executing. Signals get mapped into this as
    /// 128 + SIGNUM, which is the convention shells follow.
    pub exit_code: Option<i32>,
    /// Any additional message that a command's executor wants to be user visible in case of a
    /// failure. Provided by non-Meta RE server.
    pub additional_message: Option<String>,
    pub inline_environment_metadata: buck2_data::InlineCommandExecutionEnvironmentMetadata,
}

impl CommandExecutionReport {
    pub async fn to_command_execution_proto(
        &self,
        omit_stdout: bool,
        omit_stderr: bool,
        omit_command_details: bool,
    ) -> buck2_data::CommandExecution {
        let details = self
            .to_command_execution_details_proto(omit_stdout, omit_stderr, omit_command_details)
            .await;

        let status = match &self.status {
            CommandExecutionStatus::Success { .. } => {
                buck2_data::command_execution::Success {}.into()
            }
            CommandExecutionStatus::Cancelled { .. } => {
                buck2_data::command_execution::Cancelled {}.into()
            }
            CommandExecutionStatus::Failure {
                missing_materialized_inputs,
                ..
            } => buck2_data::command_execution::Failure {
                missing_materialized_inputs: *missing_materialized_inputs,
            }
            .into(),
            CommandExecutionStatus::WorkerFailure { .. } => {
                buck2_data::command_execution::WorkerFailure {}.into()
            }
            CommandExecutionStatus::TimedOut { duration, .. } => {
                buck2_data::command_execution::Timeout {
                    duration: (*duration).try_into().ok(),
                }
                .into()
            }
            CommandExecutionStatus::Error { stage, error, .. } => {
                buck2_data::command_execution::Error {
                    stage: (*stage).to_owned(),
                    error: format!("{error:#}"),
                }
                .into()
            }
        };

        buck2_data::CommandExecution {
            details: Some(details),
            status: Some(status),
            inline_environment_metadata: Some(self.inline_environment_metadata),
        }
    }

    async fn to_command_execution_details_proto(
        &self,
        omit_stdout: bool,
        omit_stderr: bool,
        omit_command_details: bool,
    ) -> buck2_data::CommandExecutionDetails {
        // If the top-level command failed then we don't want to omit any details. If it succeeded and
        // so did this command (it could succeed while not having a success here if we have rejected
        // executions), then we'll strip non-relevant stuff.
        let omit_stdout =
            omit_stdout && matches!(self.status, CommandExecutionStatus::Success { .. });

        let signed_exit_code = self.exit_code;

        let std_pair = self.std_streams.to_lossy().await;
        let mut stdout = std_pair.stdout;
        let mut stderr = std_pair.stderr;

        if omit_stdout {
            stdout = "".to_owned();
        }

        if omit_stderr {
            stderr = "".to_owned();
        }

        let command_kind = self
            .status
            .execution_kind()
            .map(|k| k.to_proto(omit_command_details));

        buck2_data::CommandExecutionDetails {
            cmd_stdout: stdout,
            cmd_stderr: stderr,
            command_kind,
            signed_exit_code,
            metadata: Some(self.timing.to_proto()),
            additional_message: self.additional_message.clone(),
        }
    }
}

/// Implement FromResidual so that it's easier to refactor functions returning a CommandExecutionResult
/// (it allows to easily factor out early returns into another function and then propagate them with `?`).
impl FromResidual<ControlFlow<Self, Infallible>> for CommandExecutionResult {
    fn from_residual(residual: ControlFlow<Self, Infallible>) -> Self {
        match residual {
            ControlFlow::Break(v) => v,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use buck2_common::cas_digest::CasDigest;
    use buck2_hash::BuckFixedSortedMap;
    use buck2_util::time_span::TimeSpan;

    use super::*;

    fn make_simple_report() -> CommandExecutionReport {
        // The field values correspond to what `make_simple_proto()` builds.
        let status = CommandExecutionStatus::Success {
            execution_kind: CommandExecutionKind::Local {
                digest: CasDigest::new_blake3([0].repeat(32).as_slice().try_into().unwrap(), 123),
                command: vec!["fake_buck2".to_owned()],
                env: BuckFixedSortedMap::from_iter([("FAKE_ENV_VAR".to_owned(), "1".to_owned())]),
            },
        };
        let timing = CommandExecutionMetadata {
            time_span: TimeSpan::from_start_and_duration(Instant::now(), Duration::from_secs(2)),
            execution_time: Duration::from_secs(3),
            start_time: SystemTime::UNIX_EPOCH,
            execution_stats: Some(buck2_data::CommandExecutionStats {
                cpu_instructions_user: Some(4),
                cpu_instructions_kernel: Some(5),
                userspace_events: Some(buck2_data::CpuCounter {
                    count: 4,
                    time_enabled: 100,
                    time_running: 100,
                }),
                kernel_events: Some(buck2_data::CpuCounter {
                    count: 10,
                    time_enabled: 50,
                    time_running: 100,
                }),
                memory_peak: None,
            }),
            input_materialization_duration: Duration::from_secs(6),
            hashing_duration: Duration::from_secs(7),
            hashed_artifacts_count: 8,
            queue_duration: Some(Duration::from_secs(9)),
            suspend_duration: None,
            suspend_count: None,
        };
        let std_streams = CommandStdStreams::Local {
            stdout: [65, 66, 67].to_vec(), // ABC
            stderr: [68, 69, 70].to_vec(), // DEF
        };

        CommandExecutionReport {
            claim: None,
            status,
            timing,
            std_streams,
            exit_code: Some(456),
            additional_message: None,
            inline_environment_metadata: buck2_data::InlineCommandExecutionEnvironmentMetadata {
                sandcastle_instance_id: Some(123),
            },
        }
    }

    fn make_simple_proto() -> buck2_data::CommandExecution {
        // The field values correspond to what `make_simple_report()` builds.
        let command_execution_kind = buck2_data::CommandExecutionKind {
            command: Some(buck2_data::command_execution_kind::Command::LocalCommand(
                buck2_data::LocalCommand {
                    argv: vec!["fake_buck2".to_owned()],
                    env: vec![buck2_data::EnvironmentEntry {
                        key: "FAKE_ENV_VAR".to_owned(),
                        value: "1".to_owned(),
                    }],
                    action_digest: format!("{}:{}", "0".repeat(64), "123"),
                    ..Default::default()
                },
            )),
        };
        let command_execution_stats = buck2_data::CommandExecutionStats {
            cpu_instructions_user: Some(4),
            cpu_instructions_kernel: Some(5),
            userspace_events: Some(buck2_data::CpuCounter {
                count: 4,
                time_enabled: 100,
                time_running: 100,
            }),
            kernel_events: Some(buck2_data::CpuCounter {
                count: 10,
                time_enabled: 50,
                time_running: 100,
            }),
            memory_peak: None,
        };
        let command_execution_metadata = buck2_data::CommandExecutionMetadata {
            wall_time: Some(prost_types::Duration {
                seconds: 2,
                nanos: 0,
            }),
            execution_time: Some(prost_types::Duration {
                seconds: 3,
                nanos: 0,
            }),
            start_time: Some(prost_types::Timestamp {
                seconds: 0, // UNIX_EPOCH
                nanos: 0,
            }),
            input_materialization_duration: Some(prost_types::Duration {
                seconds: 6,
                nanos: 0,
            }),
            execution_stats: Some(command_execution_stats),
            hashing_duration: Some(prost_types::Duration {
                seconds: 7,
                nanos: 0,
            }),
            hashed_artifacts_count: 8,
            queue_duration: Some(prost_types::Duration {
                seconds: 9,
                nanos: 0,
            }),
            suspend_duration: None,
            suspend_count: None,
            client_start_time: None,
        };
        let command_execution_details = buck2_data::CommandExecutionDetails {
            signed_exit_code: Some(456),
            cmd_stdout: "ABC".to_owned(),
            cmd_stderr: "DEF".to_owned(),
            command_kind: Some(command_execution_kind),
            metadata: Some(command_execution_metadata),
            additional_message: None,
        };

        buck2_data::CommandExecution {
            details: Some(command_execution_details),
            status: Some(buck2_data::command_execution::Status::Success(
                buck2_data::command_execution::Success {},
            )),
            inline_environment_metadata: Some(
                buck2_data::InlineCommandExecutionEnvironmentMetadata {
                    sandcastle_instance_id: Some(123),
                },
            ),
        }
    }

    #[tokio::test]
    async fn test_to_command_execution_proto() {
        let report = make_simple_report();
        let proto = report.to_command_execution_proto(false, false, false).await;
        let expected_proto = make_simple_proto();

        assert_eq!(proto, expected_proto);
    }

    #[tokio::test]
    async fn test_to_command_execution_proto_omit_stdout() {
        let report = make_simple_report();
        let proto = report.to_command_execution_proto(true, false, false).await;
        let mut expected_proto = make_simple_proto();

        expected_proto.details.as_mut().unwrap().cmd_stdout = "".to_owned();

        assert_eq!(proto, expected_proto);
    }

    #[tokio::test]
    async fn test_to_command_execution_proto_omit_stderr() {
        let report = make_simple_report();
        let proto = report.to_command_execution_proto(false, true, false).await;
        let mut expected_proto = make_simple_proto();

        expected_proto.details.as_mut().unwrap().cmd_stderr = "".to_owned();

        assert_eq!(proto, expected_proto);
    }

    #[tokio::test]
    async fn test_to_command_execution_proto_omit_command_details() {
        let report = make_simple_report();
        let proto = report.to_command_execution_proto(false, false, true).await;
        let mut expected_proto = make_simple_proto();

        expected_proto
            .details
            .as_mut()
            .unwrap()
            .command_kind
            .as_mut()
            .unwrap()
            .command = Some(
            buck2_data::command_execution_kind::Command::OmittedLocalCommand(
                buck2_data::OmittedLocalCommand {
                    action_digest: format!("{}:{}", "0".repeat(64), "123"),
                },
            ),
        );

        assert_eq!(proto, expected_proto);
    }
}
