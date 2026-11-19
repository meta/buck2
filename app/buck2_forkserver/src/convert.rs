/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::convert::ProstDurationExt;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_execute_local::CommandEvent;
use buck2_execute_local::GatherOutputStatus;
use buck2_resource_control::OrphanProcessInfo;
use futures::stream::Stream;
use futures::stream::StreamExt;

pub(crate) fn encode_event_stream<S>(
    s: S,
) -> impl Stream<Item = Result<buck2_forkserver_proto::CommandEvent, tonic::Status>>
where
    S: Stream<Item = buck2_error::Result<CommandEvent>>,
{
    fn convert_event(e: CommandEvent) -> buck2_forkserver_proto::CommandEvent {
        use buck2_forkserver_proto::command_event::Data;

        let (data, orphans) = match e {
            CommandEvent::Stdout(bytes) => (
                Data::Stdout(buck2_forkserver_proto::StreamEvent {
                    data: bytes.to_vec(),
                }),
                Vec::new(),
            ),
            CommandEvent::Stderr(bytes) => (
                Data::Stderr(buck2_forkserver_proto::StreamEvent {
                    data: bytes.to_vec(),
                }),
                Vec::new(),
            ),
            CommandEvent::Exit(
                GatherOutputStatus::Finished {
                    exit_code,
                    execution_stats,
                },
                orphans,
            ) => (
                Data::Exit(buck2_forkserver_proto::ExitEvent {
                    exit_code,
                    execution_stats: execution_stats.map(|s| {
                        buck2_forkserver_proto::CollectedExecutionStats {
                            cpu_instructions_user: s.cpu_instructions_user,
                            cpu_instructions_kernel: s.cpu_instructions_kernel,
                            userspace_events: s.userspace_events,
                            kernel_events: s.kernel_events,
                        }
                    }),
                }),
                orphans,
            ),
            CommandEvent::Exit(GatherOutputStatus::TimedOut(duration), orphans) => (
                Data::Timeout(buck2_forkserver_proto::TimeoutEvent {
                    duration: duration.try_into().ok(),
                }),
                orphans,
            ),
            CommandEvent::Exit(GatherOutputStatus::Cancelled, orphans) => (
                Data::Cancel(buck2_forkserver_proto::CancelEvent {}),
                orphans,
            ),
            CommandEvent::Exit(GatherOutputStatus::SpawnFailed(reason), orphans) => (
                Data::SpawnFailed(buck2_forkserver_proto::SpawnFailedEvent { reason }),
                orphans,
            ),
        };

        buck2_forkserver_proto::CommandEvent {
            data: Some(data),
            orphan_processes: orphans
                .into_iter()
                .map(|o| buck2_forkserver_proto::OrphanProcess {
                    pid: o.pid,
                    comm: o.comm,
                })
                .collect(),
        }
    }

    fn convert_err(e: buck2_error::Error) -> tonic::Status {
        tonic::Status::unknown(format!("{e:#}"))
    }

    s.map(|r| r.map(convert_event).map_err(convert_err))
}

pub(crate) fn decode_event_stream<S>(s: S) -> impl Stream<Item = buck2_error::Result<CommandEvent>>
where
    S: Stream<Item = Result<buck2_forkserver_proto::CommandEvent, tonic::Status>>,
{
    fn convert_event(e: buck2_forkserver_proto::CommandEvent) -> buck2_error::Result<CommandEvent> {
        use buck2_forkserver_proto::command_event::Data;

        let orphans: Vec<OrphanProcessInfo> = e
            .orphan_processes
            .into_iter()
            .map(|o| OrphanProcessInfo {
                pid: o.pid,
                comm: o.comm,
            })
            .collect();

        let event = match e.data.internal_error("Missing `data`")? {
            Data::Stdout(buck2_forkserver_proto::StreamEvent { data }) => {
                CommandEvent::Stdout(data.into())
            }
            Data::Stderr(buck2_forkserver_proto::StreamEvent { data }) => {
                CommandEvent::Stderr(data.into())
            }
            Data::Exit(buck2_forkserver_proto::ExitEvent {
                exit_code,
                execution_stats,
            }) => CommandEvent::Exit(
                GatherOutputStatus::Finished {
                    exit_code,
                    execution_stats: execution_stats.map(|s| {
                        buck2_execute_local::CollectedExecutionStats {
                            cpu_instructions_user: s.cpu_instructions_user,
                            cpu_instructions_kernel: s.cpu_instructions_kernel,
                            userspace_events: s.userspace_events,
                            kernel_events: s.kernel_events,
                        }
                    }),
                },
                orphans,
            ),
            Data::Timeout(buck2_forkserver_proto::TimeoutEvent { duration }) => CommandEvent::Exit(
                GatherOutputStatus::TimedOut(
                    duration
                        .internal_error("Missing `duration`")?
                        .try_into_duration()
                        .buck_error_context("Invalid `duration`")?,
                ),
                orphans,
            ),
            Data::Cancel(buck2_forkserver_proto::CancelEvent {}) => {
                CommandEvent::Exit(GatherOutputStatus::Cancelled, orphans)
            }
            Data::SpawnFailed(buck2_forkserver_proto::SpawnFailedEvent { reason }) => {
                CommandEvent::Exit(GatherOutputStatus::SpawnFailed(reason), orphans)
            }
        };

        Ok(event)
    }

    fn convert_err(e: tonic::Status) -> buck2_error::Error {
        buck2_error::buck2_error!(
            buck2_error::ErrorTag::Tier0,
            "forkserver error: {}",
            e.message()
        )
    }

    s.map(|r| r.map_err(convert_err).and_then(convert_event))
}
