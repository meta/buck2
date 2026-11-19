/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub enum LastCommandExecutionKind {
    Local,
    LocalWorker,
    Remote,
    /// Submitted for remote execution, but the service served its own action cache instead.
    RemoteServedFromCache,
    /// Executed remotely as an execution shared with another build: the service deduplicated it
    /// against an identical execution already in flight.
    RemoteDeduplicated,
    Cached,
    RemoteDepFileCached,
    NoCommand,
}

/// Returns what the execution kind of the last command was in the given action.
/// It tells the execution kind of the commands that actually produced a result
/// or an error, but not the commands that fell back and were retried.
pub fn get_last_command_execution_kind(
    action: &buck2_data::ActionExecutionEnd,
) -> LastCommandExecutionKind {
    let last_command_kind = action
        .commands
        .last()
        .and_then(|c| c.details.as_ref())
        .and_then(|c| c.command_kind.as_ref());

    if let Some(command_kind) = last_command_kind {
        use buck2_data::command_execution_kind::Command;
        match command_kind.command.as_ref() {
            Some(Command::LocalCommand(..)) | Some(Command::OmittedLocalCommand(..)) => {
                LastCommandExecutionKind::Local
            }
            Some(Command::WorkerCommand(_)) | Some(Command::WorkerInitCommand(_)) => {
                LastCommandExecutionKind::LocalWorker
            }
            Some(Command::RemoteCommand(remote)) => match remote.cache_hit_type() {
                buck2_data::RemoteCommandOrigin::ActionCache => LastCommandExecutionKind::Cached,
                buck2_data::RemoteCommandOrigin::RemoteDepFileCache => {
                    LastCommandExecutionKind::RemoteDepFileCached
                }
                buck2_data::RemoteCommandOrigin::Executed => LastCommandExecutionKind::Remote,
                buck2_data::RemoteCommandOrigin::ReServedFromCache => {
                    LastCommandExecutionKind::RemoteServedFromCache
                }
                buck2_data::RemoteCommandOrigin::ReDeduplicated => {
                    LastCommandExecutionKind::RemoteDeduplicated
                }
            },
            None => LastCommandExecutionKind::NoCommand,
        }
    } else {
        LastCommandExecutionKind::NoCommand
    }
}

pub struct ExecTime {
    pub exec_time_ms: u64,
    pub(crate) cached_exec_time_ms: u64,
}

pub fn get_last_command_execution_time(action: &buck2_data::ActionExecutionEnd) -> ExecTime {
    let exec_time_ms = action
        .commands
        .last()
        .and_then(|c| c.details.as_ref())
        .and_then(|c| c.metadata.as_ref())
        .and_then(|c| c.execution_time.as_ref())
        .map_or(0, |c| c.seconds as u64 * 1000 + c.nanos as u64 / 1000000);

    ExecTime {
        exec_time_ms,
        cached_exec_time_ms: match get_last_command_execution_kind(action) {
            LastCommandExecutionKind::Cached | LastCommandExecutionKind::RemoteDepFileCached => {
                exec_time_ms
            }
            _ => 0,
        },
    }
}
