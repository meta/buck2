/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Independently normalize action starts/ends after full decoding.
//!
//! No pairing, ancestry, interning, capture validation or output ordering lives
//! here. Serial readers and decoder workers produce the same projections.
//! Only the ordered reducer decides whether a start/end pair is retained.
//!
//! Retained heap accounting includes actual vector/string capacities. During
//! projection a raw protobuf and its normalized result can coexist: this is
//! additional per-worker transient memory, not covered by the retained chunk
//! charge. Accepted oversized records remain accepted. Moving owned vectors
//! avoids copies but can retain more spare capacity than the old clone.

use std::mem::needs_drop;
use std::mem::size_of;

use allocative::Allocative;
use buck2_event_log::stream_value::StreamValue;

use super::scan::ActionRec;
use super::scan::CmdRec;
use super::scan::LastCmd;
use super::scan::dur_us;
use super::scan::key_hash;
use super::scan::remote_cache_origin;
use super::scan::status_name;
use super::scan::ts_us;
use crate::execution;

/// Box the uncommon large variants so every other input record does not pay
/// for their inline size. No raw action payload is retained as a sidecar.
#[derive(Allocative)]
pub(super) enum Record {
    Raw(StreamValue),
    Start(Box<Start>),
    End(Box<End>),
}

#[derive(Allocative)]
pub(super) struct Start {
    pub sid: u64,
    pub parent: u64,
    pub ts: i64,
    pub full_target: String,
    /// Conversion to u32 remains deferred until a matching end, like the
    /// original projector. An orphan/overwritten huge name must not introduce
    /// a speculative refusal before the ordered reducer pairs its end.
    pub short_at: Option<u32>,
    pub cfg: String,
    pub category: String,
    pub key_hash: Option<u128>,
    pub kind: i32,
}

#[derive(Allocative)]
pub(super) struct End {
    pub sid: u64,
    pub key_hash: Option<u128>,
    pub dur: i64,
    pub failed: bool,
    pub error_kind: &'static str,
    pub execution_kind: i32,
    pub cache_upload_result: i32,
    pub dep_file_cache_upload_result: i32,
    pub wall_time_us: Option<i64>,
    pub prepare_us: Option<u64>,
    pub finish_us: Option<u64>,
    pub output_size: u64,
    pub input_files_bytes: Option<u64>,
    pub invalidated: bool,
    /// Required even for an orphan end: the reducer selects the first local
    /// hostname in stream order, independently of successful pairing.
    pub hostname: Option<String>,
    pub cmds: Vec<CmdRec>,
    /// Owned only until the ordered reducer spills it. Never copied into the
    /// trace JSON or retained with the completed action graph.
    pub reports: Vec<PendingAttempt>,
    pub last: LastCmd,
    pub digest: Option<String>,
    pub dep_span_ids: Vec<u64>,
    pub dep_action_keys: Vec<u128>,
    pub dep_tset_ids: Vec<u64>,
}

#[derive(Allocative)]
pub(super) struct PendingAttempt {
    pub report: execution::Attempt,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

impl End {
    pub fn has_deps(&self) -> bool {
        !self.dep_span_ids.is_empty()
            || !self.dep_action_keys.is_empty()
            || !self.dep_tset_ids.is_empty()
    }

    pub fn into_action(mut self, start: Start, cfg: u32, category: u32) -> ActionRec {
        // Keep the compatibility argv without a second copy of a potentially
        // large command during worker handoff. Details have already been
        // serialized by the reducer (when enabled), so their argv can move.
        if let Some(command) = self.reports.pop().and_then(|p| p.report.command)
            && !command.argv.is_empty()
        {
            self.last.argv = Some(command.argv);
        }
        ActionRec {
            sid: self.sid,
            ts: start.ts,
            dur: self.dur,
            full_target: start.full_target,
            short_at: start
                .short_at
                .expect("commit_normalized checks the package path before interning"),
            cfg,
            category,
            key_hash: start.key_hash,
            kind: start.kind,
            failed: self.failed,
            error_kind: self.error_kind,
            execution_kind: self.execution_kind,
            cache_upload_result: self.cache_upload_result,
            dep_file_cache_upload_result: self.dep_file_cache_upload_result,
            wall_time_us: self.wall_time_us,
            prepare_us: self.prepare_us,
            finish_us: self.finish_us,
            output_size: self.output_size,
            input_files_bytes: self.input_files_bytes,
            invalidated: self.invalidated,
            cmds: self.cmds,
            last: self.last,
            digest: self.digest,
            execution: None,
            dep_span_ids: self.dep_span_ids,
            dep_action_keys: self.dep_action_keys,
            dep_tset_ids: self.dep_tset_ids,
        }
    }
}

pub(super) fn project(value: StreamValue) -> Record {
    use buck2_data::buck_event::Data;
    use buck2_data::span_end_event::Data as EndData;
    use buck2_data::span_start_event::Data as StartData;

    let StreamValue::Event(mut event) = value else {
        return Record::Raw(value);
    };
    let Some(timestamp) = &event.timestamp else {
        // Capture identity records remain meaningful without timestamps;
        // the reducer validates them before considering positioned events.
        return Record::Raw(StreamValue::Event(event));
    };
    // Inspect borrowed tags first. Non-action events keep their payloads and
    // need no take/rebuild of the large protobuf data enum.
    let action = match &event.data {
        Some(Data::SpanStart(s)) => matches!(&s.data, Some(StartData::ActionExecution(_))),
        Some(Data::SpanEnd(e)) => matches!(&e.data, Some(EndData::ActionExecution(_))),
        _ => false,
    };
    if !action {
        return Record::Raw(StreamValue::Event(event));
    }
    let ts = ts_us(timestamp);
    match event.data.take() {
        Some(Data::SpanStart(buck2_data::SpanStartEvent {
            data: Some(StartData::ActionExecution(start)),
        })) => Record::Start(Box::new(project_start(
            event.span_id,
            event.parent_id,
            ts,
            start,
        ))),
        Some(Data::SpanEnd(end)) => {
            let dur = end
                .duration
                .as_ref()
                .map(dur_us)
                .filter(|d| *d >= 0)
                .unwrap_or(0);
            if let Some(EndData::ActionExecution(action)) = end.data {
                Record::End(Box::new(project_end(event.span_id, dur, action)))
            } else {
                unreachable!("the borrowed end tag was ActionExecution")
            }
        }
        _ => unreachable!("the borrowed tag was an action start/end"),
    }
}

pub(super) fn project_start(
    sid: u64,
    parent: u64,
    ts: i64,
    start: buck2_data::ActionExecutionStart,
) -> Start {
    use buck2_data::action_key::Owner;

    // Hash the entire original key BEFORE moving any owned name fields out.
    let key_hash = start.key.as_ref().map(key_hash);
    let (what, category) = match start.name {
        Some(name) => (
            format!("{} {}", name.category, name.identifier)
                .trim()
                .to_owned(),
            name.category,
        ),
        None => (String::new(), String::new()),
    };
    let (pkg, name, cfg) = match start.key.and_then(|key| key.owner) {
        Some(Owner::TargetLabel(target)) => {
            let (pkg, name) = target
                .label
                .map(|label| (label.package, label.name))
                .unwrap_or_else(|| ("?".to_owned(), "?".to_owned()));
            let cfg = target
                .configuration
                .map(|configuration| configuration.full_name)
                .unwrap_or_default();
            (pkg, name, cfg)
        }
        Some(Owner::BxlKey(_)) => ("?".to_owned(), "BxlKey".to_owned(), String::new()),
        Some(Owner::TestTargetLabel(_)) => {
            ("?".to_owned(), "TestTargetLabel".to_owned(), String::new())
        }
        Some(Owner::AnonTarget(_)) => ("?".to_owned(), "AnonTarget".to_owned(), String::new()),
        Some(Owner::LocalResourceSetup(_)) => (
            "?".to_owned(),
            "LocalResourceSetup".to_owned(),
            String::new(),
        ),
        None => ("?".to_owned(), "?".to_owned(), String::new()),
    };
    Start {
        sid,
        parent,
        ts,
        short_at: pkg.len().checked_add(1).and_then(|n| u32::try_from(n).ok()),
        full_target: format!("{pkg}:{name} ({what})"),
        cfg,
        category,
        key_hash,
        kind: start.kind,
    }
}

pub(super) fn project_end(sid: u64, dur: i64, end: Box<buck2_data::ActionExecutionEnd>) -> End {
    let error_kind = super::scan::action_error_kind(&end);
    let mut cmds = Vec::with_capacity(end.commands.len());
    let mut reports = Vec::with_capacity(end.commands.len());
    let mut last = LastCmd::default();
    let mut digest = None;
    let n_commands = end.commands.len();
    for (index, command) in end.commands.into_iter().enumerate() {
        let md = command.details.as_ref().and_then(|d| d.metadata.as_ref());
        cmds.push(CmdRec {
            status: status_name(&command),
            wall_time_us: md.and_then(|m| m.wall_time.as_ref()).map(dur_us),
            start_us: md.and_then(|m| m.start_time.as_ref()).map(ts_us),
            client_start_us: md.and_then(|m| m.client_start_time.as_ref()).map(ts_us),
            elastic: command
                .details
                .as_ref()
                .and_then(super::scan::re_worker)
                .is_some_and(|w| w.elastic_capacity),
        });
        if digest.is_none() {
            use buck2_data::command_execution_kind::Command;
            let kind = command
                .details
                .as_ref()
                .and_then(|d| d.command_kind.as_ref());
            let value = kind.and_then(|kind| match &kind.command {
                Some(Command::LocalCommand(c)) => Some(&c.action_digest),
                Some(Command::RemoteCommand(c)) => Some(&c.action_digest),
                Some(Command::WorkerCommand(c)) => Some(&c.action_digest),
                Some(Command::OmittedLocalCommand(c)) => Some(&c.action_digest),
                _ => None,
            });
            digest = value.filter(|value| !value.is_empty()).cloned();
        }
        if index + 1 == n_commands {
            last = project_last(&command);
        }
        reports.push(project_attempt(command, !end.failed));
    }
    let producers = end.graph_producers.unwrap_or_default();
    End {
        sid,
        key_hash: end.key.as_ref().map(key_hash),
        dur,
        failed: end.failed,
        error_kind,
        execution_kind: end.execution_kind,
        cache_upload_result: end.cache_upload_result,
        dep_file_cache_upload_result: end.dep_file_cache_upload_result,
        wall_time_us: end.wall_time.as_ref().map(dur_us),
        prepare_us: end.prepare_command_us,
        finish_us: end.finish_action_us,
        output_size: end.output_size,
        input_files_bytes: end.input_files_bytes,
        invalidated: end.invalidation_info.is_some(),
        hostname: end.hostname,
        cmds,
        reports,
        last,
        digest,
        dep_span_ids: producers.span_ids,
        dep_action_keys: producers.reused_ids.into_iter().map(u128::from).collect(),
        dep_tset_ids: producers.tset_ids,
    }
}

fn project_last(command: &buck2_data::CommandExecution) -> LastCmd {
    let mut last = LastCmd::default();
    if let Some(details) = &command.details {
        let md = details.metadata.as_ref();
        let stats = md.and_then(|m| m.execution_stats.as_ref());
        last.signed_exit_code = details.signed_exit_code;
        last.exec_us = md.and_then(|m| m.execution_time.as_ref()).map(dur_us);
        last.queue_us = md.and_then(|m| m.queue_duration.as_ref()).map(dur_us);
        last.input_fetch_us = md
            .and_then(|m| m.input_materialization_duration.as_ref())
            .map(dur_us);
        let worker = super::scan::re_worker(details);
        last.upload_us = worker
            .and_then(|w| w.output_upload_us)
            .and_then(super::scan::us_i64);
        last.worker_wall_us = worker
            .and_then(|w| w.worker_wall_time_us)
            .and_then(super::scan::us_i64);
        last.worker_bytes_in = worker.and_then(|w| w.input_download_bytes);
        last.worker_bytes_out = worker.and_then(|w| w.output_upload_bytes);
        last.attempts = worker.and_then(|w| w.execution_attempts).filter(|n| *n > 1);
        last.final_queue_us = worker.and_then(|w| w.final_queue_us);
        last.hashing_us = md.and_then(|m| m.hashing_duration.as_ref()).map(dur_us);
        last.mem_peak = stats.and_then(|s| s.memory_peak);
        last.cpu_us = worker.and_then(super::scan::cpu_total_us);
        last.cpu_user_us = worker.and_then(|w| w.cpu_time_user_us);
        last.cpu_sys_us = worker.and_then(|w| w.cpu_time_system_us);
        last.elastic = worker.is_some_and(|w| w.elastic_capacity);
        if let Some(kind) = &details.command_kind {
            use buck2_data::command_execution_kind::Command;
            last.hashing_local_outputs = matches!(
                &kind.command,
                Some(
                    Command::LocalCommand(_)
                        | Command::OmittedLocalCommand(_)
                        | Command::WorkerCommand(_)
                )
            );
            match &kind.command {
                Some(Command::LocalCommand(c)) => {
                    last.local_digest = nonempty(c.action_digest.clone());
                    last.cwd = c.working_directory.clone();
                    last.env_rule = c.environment_rule;
                }
                Some(Command::RemoteCommand(c)) => {
                    last.remote = true;
                    last.reused = c.cache_hit || remote_cache_origin(c.cache_hit_type);
                    last.remote_queue_us = c.queue_time.as_ref().map(dur_us);
                    last.argv = Some(vec!["<remote>".to_owned(), c.action_digest.clone()]);
                }
                Some(Command::OmittedLocalCommand(c)) => {
                    last.local_digest = nonempty(c.action_digest.clone());
                }
                _ => {}
            }
        }
    }
    last
}

pub(super) fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

pub(super) fn logged_command(
    argv: Vec<String>,
    env: Vec<buck2_data::EnvironmentEntry>,
    fallback_exe: Vec<String>,
) -> execution::Command {
    execution::Command {
        argv,
        env: env
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect(),
        fallback_exe,
    }
}

/// A proto string has no presence bit. Even on failures/cancellation, an
/// empty value does not prove that the actual program produced no output.
fn project_output(text: String, remote: bool) -> (execution::Output, Option<String>) {
    use execution::OutputState;
    if text.is_empty() {
        return (
            execution::Output {
                state: OutputState::EmptyOrUnavailable,
                ..Default::default()
            },
            None,
        );
    }
    let producer_notice = text == "<<omitted>>"
        || (remote
            && [
                "Result too large to display",
                "Result could not be downloaded",
            ]
            .iter()
            .any(|prefix| {
                text == *prefix
                    || text.strip_prefix(*prefix).is_some_and(|suffix| {
                        suffix.starts_with(" - to view type `frecli cas download-blob ")
                            && suffix.ends_with('`')
                    })
            }));
    (
        execution::Output {
            state: if producer_notice {
                OutputState::ProducerNotice
            } else {
                OutputState::Recorded
            },
            note: producer_notice.then(|| {
                "Matches Buck's output-notice format; the retained text is not proof of program output."
                    .to_owned()
            }),
            ..Default::default()
        },
        Some(text),
    )
}

fn project_attempt(
    command: buck2_data::CommandExecution,
    action_succeeded: bool,
) -> PendingAttempt {
    use buck2_data::command_execution::Status;
    use buck2_data::command_execution_kind::Command;

    // The action-level formatter deliberately omits stdout only when both
    // the action execution and this command report succeeded. A build can
    // fail elsewhere without changing that per-action logging policy.
    let stdout_omitted = action_succeeded && matches!(&command.status, Some(Status::Success(_)));

    let mut report = execution::Attempt {
        status: status_name(&command).to_owned(),
        executor: "unknown".to_owned(),
        // The reducer assigns the checked report index in stream order.
        ..Default::default()
    };
    report.error = match command.status {
        Some(Status::Error(error)) => Some(format!("{}: {}", error.stage, error.error)),
        Some(Status::Timeout(timeout)) => timeout.duration.map(|duration| {
            format!(
                "Command timed out after {}s {}ns",
                duration.seconds, duration.nanos
            )
        }),
        _ => None,
    };
    let Some(details) = command.details else {
        return PendingAttempt {
            report,
            stdout: None,
            stderr: None,
        };
    };
    report.exit_code = details.signed_exit_code;
    if let Some(metadata) = details.metadata {
        // Worker start_time is not a client-clock attempt anchor.
        report.start_us = metadata.client_start_time.as_ref().map(ts_us);
        report.wall_time_us = metadata.wall_time.as_ref().map(dur_us);
    }
    if let Some(message) = details.additional_message.filter(|s| !s.is_empty()) {
        match &mut report.error {
            Some(error) => {
                error.push_str("\nAdditional message: ");
                error.push_str(&message);
            }
            None => report.error = Some(format!("Additional message: {message}")),
        }
    }
    match details.command_kind.and_then(|kind| kind.command) {
        Some(Command::LocalCommand(command)) => {
            report.executor = "local".to_owned();
            report.action_digest = nonempty(command.action_digest);
            report.command = Some(logged_command(command.argv, command.env, Vec::new()));
        }
        Some(Command::OmittedLocalCommand(command)) => {
            report.executor = "local".to_owned();
            report.action_digest = nonempty(command.action_digest);
        }
        Some(Command::WorkerCommand(command)) => {
            report.executor = "worker".to_owned();
            report.action_digest = nonempty(command.action_digest);
            report.command = Some(logged_command(
                command.argv,
                command.env,
                command.fallback_exe,
            ));
        }
        Some(Command::WorkerInitCommand(command)) => {
            report.executor = "worker_init".to_owned();
            report.command = Some(logged_command(command.argv, command.env, Vec::new()));
        }
        Some(Command::InFlightCommand(command)) => {
            report.executor = "in_flight".to_owned();
            report.action_digest = nonempty(command.action_digest);
        }
        Some(Command::RemoteCommand(command)) => {
            report.executor = "remote".to_owned();
            report.action_digest = nonempty(command.action_digest);
            report.use_case = command
                .details
                .and_then(|details| nonempty(details.use_case));
        }
        None => {}
    }
    let remote = report.executor == "remote";
    let (mut stdout, stdout_text) = project_output(details.cmd_stdout, remote);
    if stdout_omitted && stdout_text.is_none() {
        stdout.state = execution::OutputState::NotRecorded;
        stdout.note = Some(
            "Buck omits stdout for successful command reports when the action succeeds.".to_owned(),
        );
    }
    let (stderr, stderr_text) = project_output(details.cmd_stderr, remote);
    report.stdout = stdout;
    report.stderr = stderr;
    PendingAttempt {
        report,
        stdout: stdout_text,
        stderr: stderr_text,
    }
}

fn strings_heap(values: &[String], capacity: usize) -> usize {
    capacity.saturating_mul(size_of::<String>()).saturating_add(
        values
            .iter()
            .map(String::capacity)
            .fold(0usize, usize::saturating_add),
    )
}

#[inline]
fn scalar<T>(_: &T) {
    const { assert!(!needs_drop::<T>()) };
}

fn last_heap(last: &LastCmd) -> usize {
    let LastCmd {
        signed_exit_code,
        exec_us,
        reused,
        queue_us,
        input_fetch_us,
        upload_us,
        worker_wall_us,
        worker_bytes_in,
        worker_bytes_out,
        attempts,
        final_queue_us,
        hashing_us,
        hashing_local_outputs,
        mem_peak,
        cpu_us,
        cpu_user_us,
        cpu_sys_us,
        elastic,
        remote,
        remote_queue_us,
        argv,
        local_digest,
        cwd,
        env_rule,
    } = last;
    macro_rules! scalars { ($($value:ident),* $(,)?) => { $(scalar($value);)* }; }
    scalars!(
        signed_exit_code,
        exec_us,
        reused,
        queue_us,
        input_fetch_us,
        upload_us,
        worker_wall_us,
        worker_bytes_in,
        worker_bytes_out,
        attempts,
        final_queue_us,
        hashing_us,
        hashing_local_outputs,
        mem_peak,
        cpu_us,
        cpu_user_us,
        cpu_sys_us,
        elastic,
        remote,
        remote_queue_us,
        env_rule
    );
    argv.as_ref()
        .map_or(0, |argv| strings_heap(argv, argv.capacity()))
        .saturating_add(local_digest.as_ref().map_or(0, String::capacity))
        .saturating_add(cwd.as_ref().map_or(0, String::capacity))
}

impl Record {
    fn fast_heap_bytes(&self) -> usize {
        match self {
            Self::Raw(value) => super::heap_size::value_heap_bytes(value),
            Self::Start(start) => {
                let Start {
                    sid,
                    parent,
                    ts,
                    full_target,
                    short_at,
                    cfg,
                    category,
                    key_hash,
                    kind,
                } = start.as_ref();
                scalar(sid);
                scalar(parent);
                scalar(ts);
                scalar(short_at);
                scalar(key_hash);
                scalar(kind);
                size_of::<Start>()
                    .saturating_add(full_target.capacity())
                    .saturating_add(cfg.capacity())
                    .saturating_add(category.capacity())
            }
            Self::End(end) => {
                let End {
                    sid,
                    key_hash,
                    dur,
                    failed,
                    error_kind,
                    execution_kind,
                    cache_upload_result,
                    dep_file_cache_upload_result,
                    wall_time_us,
                    prepare_us,
                    finish_us,
                    output_size,
                    input_files_bytes,
                    invalidated,
                    hostname,
                    cmds,
                    reports,
                    last,
                    digest,
                    dep_span_ids,
                    dep_action_keys,
                    dep_tset_ids,
                } = end.as_ref();
                scalar(sid);
                scalar(key_hash);
                scalar(dur);
                scalar(failed);
                scalar(error_kind);
                scalar(execution_kind);
                scalar(cache_upload_result);
                scalar(dep_file_cache_upload_result);
                scalar(wall_time_us);
                scalar(prepare_us);
                scalar(finish_us);
                scalar(output_size);
                scalar(input_files_bytes);
                scalar(invalidated);
                const { assert!(!needs_drop::<CmdRec>()) };
                size_of::<End>()
                    .saturating_add(hostname.as_ref().map_or(0, String::capacity))
                    .saturating_add(digest.as_ref().map_or(0, String::capacity))
                    .saturating_add(cmds.capacity().saturating_mul(size_of::<CmdRec>()))
                    .saturating_add(allocative::size_of_unique_allocated_data(reports))
                    .saturating_add(last_heap(last))
                    .saturating_add(dep_span_ids.capacity().saturating_mul(size_of::<u64>()))
                    .saturating_add(dep_action_keys.capacity().saturating_mul(size_of::<u128>()))
                    .saturating_add(dep_tset_ids.capacity().saturating_mul(size_of::<u64>()))
            }
        }
    }

    pub fn heap_bytes(&self, mode: super::HeapMode) -> usize {
        match mode {
            #[cfg(test)]
            super::HeapMode::Reference => allocative::size_of_unique_allocated_data(self),
            super::HeapMode::Fast => self.fast_heap_bytes(),
            #[cfg(test)]
            super::HeapMode::Verify => {
                let fast = self.fast_heap_bytes();
                assert_eq!(
                    fast,
                    allocative::size_of_unique_allocated_data(self),
                    "normalized retained heap accounting differs"
                );
                fast
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_reports_preserve_outputs_environment_status_and_missingness() {
        use buck2_data::command_execution::Status;
        use buck2_data::command_execution_kind::Command;
        use execution::OutputState;

        let env = vec![
            buck2_data::EnvironmentEntry {
                key: "DUP".to_owned(),
                value: "first".to_owned(),
            },
            buck2_data::EnvironmentEntry {
                key: "DUP".to_owned(),
                value: "second".to_owned(),
            },
        ];
        let command = buck2_data::CommandExecution {
            status: Some(Status::Error(buck2_data::command_execution::Error {
                stage: "worker".to_owned(),
                error: "failed to connect".to_owned(),
            })),
            details: Some(buck2_data::CommandExecutionDetails {
                command_kind: Some(buck2_data::CommandExecutionKind {
                    command: Some(Command::WorkerCommand(buck2_data::WorkerCommand {
                        argv: vec!["--request".to_owned()],
                        env,
                        fallback_exe: vec!["worker-tool".to_owned()],
                        action_digest: "digest:1".to_owned(),
                    })),
                }),
                cmd_stdout: "stdout\0é\n".to_owned(),
                cmd_stderr: "\u{1b}[31mstderr\u{1b}[0m\n".to_owned(),
                signed_exit_code: Some(-9),
                additional_message: Some("backend diagnostic".to_owned()),
                metadata: Some(buck2_data::CommandExecutionMetadata {
                    start_time: Some(prost_types::Timestamp {
                        seconds: 900,
                        nanos: 0,
                    }),
                    client_start_time: Some(prost_types::Timestamp {
                        seconds: 100,
                        nanos: 5000,
                    }),
                    wall_time: Some(prost_types::Duration {
                        seconds: 2,
                        nanos: 0,
                    }),
                    ..Default::default()
                }),
            }),
            ..Default::default()
        };
        let projected = project_attempt(command, false);
        assert_eq!(projected.report.status, "Error");
        assert_eq!(projected.report.executor, "worker");
        assert_eq!(projected.report.exit_code, Some(-9));
        assert_eq!(projected.report.start_us, Some(100_000_005));
        assert_eq!(projected.report.wall_time_us, Some(2_000_000));
        assert_eq!(projected.stdout.as_deref(), Some("stdout\0é\n"));
        assert_eq!(
            projected.stderr.as_deref(),
            Some("\u{1b}[31mstderr\u{1b}[0m\n")
        );
        assert_eq!(projected.report.stdout.state, OutputState::Recorded);
        assert!(
            projected.report.stdout.content.is_none(),
            "only the reducer writes references"
        );
        assert_eq!(
            projected.report.error.as_deref(),
            Some("worker: failed to connect\nAdditional message: backend diagnostic")
        );
        let cmd = projected.report.command.unwrap();
        assert_eq!(cmd.argv, ["--request"]);
        assert_eq!(cmd.fallback_exe, ["worker-tool"]);
        assert_eq!(
            cmd.env,
            [
                ("DUP".to_owned(), "first".to_owned()),
                ("DUP".to_owned(), "second".to_owned())
            ]
        );

        let missing = project_attempt(Default::default(), false);
        assert_eq!(missing.report.stdout.state, OutputState::NotRecorded);
        assert!(missing.report.command.is_none());
        let empty = project_attempt(
            buck2_data::CommandExecution {
                details: Some(Default::default()),
                ..Default::default()
            },
            false,
        );
        assert_eq!(empty.report.stdout.state, OutputState::EmptyOrUnavailable);
        assert_eq!(empty.report.stderr.state, OutputState::EmptyOrUnavailable);
        assert!(empty.stdout.is_none() && empty.stderr.is_none());
    }

    #[test]
    fn successful_action_stdout_omission_is_not_an_empty_output_claim() {
        use buck2_data::command_execution::Status;
        use execution::OutputState;

        for failed in [false, true] {
            for status in [
                Some(Status::Success(Default::default())),
                Some(Status::Failure(Default::default())),
                Some(Status::Cancelled(Default::default())),
                None,
            ] {
                for stdout in ["", "real recorded stdout"] {
                    let omitted =
                        !failed && matches!(&status, Some(Status::Success(_))) && stdout.is_empty();
                    let end = project_end(
                        7,
                        10,
                        Box::new(buck2_data::ActionExecutionEnd {
                            failed,
                            commands: vec![buck2_data::CommandExecution {
                                status: status.clone(),
                                details: Some(buck2_data::CommandExecutionDetails {
                                    cmd_stdout: stdout.to_owned(),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }],
                            ..Default::default()
                        }),
                    );
                    let pending = &end.reports[0];
                    assert_eq!(
                        pending.report.stdout.state,
                        if omitted {
                            OutputState::NotRecorded
                        } else if stdout.is_empty() {
                            OutputState::EmptyOrUnavailable
                        } else {
                            OutputState::Recorded
                        }
                    );
                    assert_eq!(pending.report.stdout.note.is_some(), omitted);
                    assert_eq!(
                        pending.stdout.as_deref(),
                        (!stdout.is_empty()).then_some(stdout)
                    );
                    assert_eq!(
                        pending.report.stderr.state,
                        OutputState::EmptyOrUnavailable,
                        "successful-action stdout omission says nothing about empty stderr"
                    );
                }
            }
        }
    }

    #[test]
    fn producer_notice_text_is_retained_and_remote_commands_are_not_invented() {
        use buck2_data::command_execution_kind::Command;
        use execution::OutputState;
        let notice = "Result too large to display - to view type `frecli cas download-blob deadbeef:5000000`";
        let pending = project_attempt(
            buck2_data::CommandExecution {
                details: Some(buck2_data::CommandExecutionDetails {
                    command_kind: Some(buck2_data::CommandExecutionKind {
                        command: Some(Command::RemoteCommand(buck2_data::RemoteCommand {
                            action_digest: "remote:148".to_owned(),
                            details: Some(buck2_data::RemoteCommandDetails {
                                use_case: "buck2-user".to_owned(),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })),
                    }),
                    cmd_stderr: notice.to_owned(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            false,
        );
        assert_eq!(pending.report.stderr.state, OutputState::ProducerNotice);
        assert_eq!(pending.stderr.as_deref(), Some(notice));
        assert_eq!(pending.report.use_case.as_deref(), Some("buck2-user"));
        assert!(pending.report.command.is_none());
        assert_eq!(
            project_output(notice.to_owned(), false).0.state,
            OutputState::Recorded
        );
        assert_eq!(
            project_output("Result too large to display: my own error".to_owned(), true)
                .0
                .state,
            OutputState::Recorded
        );
    }

    #[test]
    fn pending_detail_heap_includes_spare_capacities_and_large_output() {
        let mut text = String::with_capacity(16_384);
        text.push_str("retained output");
        let mut report = project_attempt(Default::default(), false);
        report.stdout = Some(text);
        let mut argv = Vec::with_capacity(19);
        let mut arg = String::with_capacity(137);
        arg.push_str("tool");
        argv.push(arg);
        let mut env = Vec::with_capacity(29);
        env.push((String::with_capacity(31), String::with_capacity(127)));
        report.report.command = Some(execution::Command {
            argv,
            env,
            fallback_exe: vec!["exe".to_owned()],
        });
        report.report.stderr.note = Some(String::with_capacity(89));
        let mut end = project_end(1, 2, Box::default());
        end.reports = Vec::with_capacity(7);
        end.reports.push(report);
        end.last.local_digest = Some(String::with_capacity(113));
        Record::End(Box::new(end)).heap_bytes(super::super::HeapMode::Verify);
    }

    fn verify_start(start: buck2_data::ActionExecutionStart) {
        let reference = super::super::action_projection::project_action(
            17,
            10,
            20,
            start.clone(),
            Box::default(),
        );
        let normalized = project_start(17, 999, 10, start);
        assert_eq!(normalized.cfg, reference.cfg);
        assert_eq!(normalized.category, reference.category);
        let record = Record::Start(Box::new(normalized));
        record.heap_bytes(super::super::HeapMode::Verify);
        let Record::Start(start) = record else {
            unreachable!()
        };
        let actual = project_end(17, 20, Box::default()).into_action(*start, 0, 0);
        assert_eq!(actual, reference.record);
    }

    #[test]
    fn start_names_complete_key_hashes_and_owner_fallbacks_match_reference() {
        use buck2_data::action_key::Owner;
        verify_start(buck2_data::ActionExecutionStart::default());
        for owner in [
            Some(Owner::TargetLabel(buck2_data::ConfiguredTargetLabel {
                label: Some(buck2_data::TargetLabel {
                    package: "fbcode//démo\"".to_owned(),
                    name: "target\\name".to_owned(),
                }),
                configuration: Some(buck2_data::Configuration {
                    full_name: "cfg:a".to_owned(),
                }),
                ..Default::default()
            })),
            Some(Owner::TargetLabel(Default::default())),
            Some(Owner::BxlKey(Default::default())),
            Some(Owner::TestTargetLabel(Default::default())),
            Some(Owner::AnonTarget(Default::default())),
            Some(Owner::LocalResourceSetup(Default::default())),
            None,
        ] {
            verify_start(buck2_data::ActionExecutionStart {
                key: Some(buck2_data::ActionKey {
                    owner,
                    ..Default::default()
                }),
                name: Some(buck2_data::ActionName {
                    category: " rustc ".to_owned(),
                    identifier: " lib.rs \u{2003}".to_owned(),
                }),
                kind: i32::MAX,
            });
        }
    }

    #[test]
    fn upload_outcomes_preserve_wire_values_without_inferring_from_permission_flags() {
        use prost::Message as _;

        for (cache, dep_file) in [(0, 0), (6, 1), (1, 16), (i32::MAX, -1)] {
            for allowed in [false, true] {
                let raw = buck2_data::ActionExecutionEnd {
                    cache_upload_result: cache,
                    dep_file_cache_upload_result: dep_file,
                    allows_cache_upload: allowed,
                    allows_dep_file_cache_upload: allowed,
                    ..Default::default()
                };
                // Unknown enum values must survive the real protobuf decoder.
                let bytes = raw.encode_to_vec();
                let decoded =
                    Box::new(buck2_data::ActionExecutionEnd::decode(bytes.as_slice()).unwrap());
                let reference = super::super::action_projection::project_action(
                    8,
                    3,
                    7,
                    Default::default(),
                    decoded.clone(),
                );
                let record = Record::End(Box::new(project_end(8, 7, decoded)));
                record.heap_bytes(super::super::HeapMode::Verify);
                let Record::End(end) = record else {
                    unreachable!()
                };
                let action = end.into_action(project_start(8, 0, 3, Default::default()), 0, 0);
                assert_eq!(action.cache_upload_result, cache);
                assert_eq!(action.dep_file_cache_upload_result, dep_file);
                assert_eq!(action, reference.record);
            }
        }
    }

    #[test]
    fn end_projection_moves_owned_vectors_and_matches_all_command_fields() {
        use buck2_data::command_execution_kind::Command;
        let mut argv = Vec::with_capacity(31);
        let mut arg = String::with_capacity(127);
        arg.push_str("compiler");
        let arg_pointer = arg.as_ptr();
        argv.push(arg);
        let argv_pointer = argv.as_ptr();
        let mut spans = Vec::with_capacity(29);
        spans.extend([1, 9, u64::MAX]);
        let spans_pointer = spans.as_ptr();
        let mut tsets = Vec::with_capacity(23);
        tsets.extend([5, 7]);
        let tsets_pointer = tsets.as_ptr();
        let command = buck2_data::CommandExecution {
            details: Some(buck2_data::CommandExecutionDetails {
                command_kind: Some(buck2_data::CommandExecutionKind {
                    command: Some(Command::LocalCommand(buck2_data::LocalCommand {
                        argv,
                        action_digest: "first-digest".to_owned(),
                        working_directory: Some("fbcode".to_owned()),
                        environment_rule: buck2_data::local_command::EnvironmentRule::TestAllowlist
                            as i32,
                        ..Default::default()
                    })),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let end = Box::new(buck2_data::ActionExecutionEnd {
            commands: vec![command],
            graph_producers: Some(buck2_data::ActionGraphProducers {
                span_ids: spans,
                reused_ids: vec![0],
                tset_ids: tsets,
            }),
            hostname: Some("local-host".to_owned()),
            execution_kind: 1,
            input_files_bytes: Some(0),
            failed: true,
            prepare_command_us: Some(400),
            finish_action_us: Some(600),
            ..Default::default()
        });
        let reference = super::super::action_projection::project_action(
            8,
            3,
            -2,
            Default::default(),
            end.clone(),
        );
        let projected = project_end(8, -2, end);
        let command = projected
            .reports
            .last()
            .unwrap()
            .report
            .command
            .as_ref()
            .unwrap();
        assert_eq!(command.argv.as_ptr(), argv_pointer);
        assert_eq!(command.argv[0].as_ptr(), arg_pointer);
        assert_eq!(projected.dep_span_ids.as_ptr(), spans_pointer);
        assert_eq!(projected.dep_tset_ids.as_ptr(), tsets_pointer);
        assert!(projected.has_deps());
        let record = Record::End(Box::new(projected));
        record.heap_bytes(super::super::HeapMode::Verify);
        let Record::End(end) = record else {
            unreachable!()
        };
        let action = end.into_action(project_start(8, 0, 3, Default::default()), 0, 0);
        assert_eq!(action, reference.record);
        assert_eq!(
            (
                action.prepare_us,
                action.finish_us,
                action.last.cwd.as_deref(),
                action.last.env_rule
            ),
            (
                Some(400),
                Some(600),
                Some("fbcode"),
                buck2_data::local_command::EnvironmentRule::TestAllowlist as i32
            ),
            "the recorded preparation, finishing, working directory and environment rule"
        );
    }

    #[test]
    fn final_command_kind_missingness_and_first_digest_match_reference() {
        use buck2_data::command_execution_kind::Command;
        let mut kinds = vec![None];
        for argv in [Vec::new(), vec!["compiler".to_owned()]] {
            kinds.push(Some(Command::LocalCommand(buck2_data::LocalCommand {
                argv: argv.clone(),
                action_digest: "last-local".to_owned(),
                ..Default::default()
            })));
            kinds.push(Some(Command::WorkerCommand(buck2_data::WorkerCommand {
                argv,
                action_digest: "last-worker".to_owned(),
                ..Default::default()
            })));
        }
        kinds.push(Some(Command::OmittedLocalCommand(
            buck2_data::OmittedLocalCommand {
                action_digest: "last-omitted".to_owned(),
            },
        )));
        for (cache_hit, cache_hit_type) in [(false, 0), (true, 0), (false, 1), (false, 2)] {
            let zero = prost_types::Duration {
                seconds: 0,
                nanos: 0,
            };
            kinds.push(Some(Command::RemoteCommand(buck2_data::RemoteCommand {
                cache_hit,
                cache_hit_type,
                action_digest: "last-remote".to_owned(),
                queue_time: Some(zero),
                re_worker: (!cache_hit).then(|| {
                    Box::new(buck2_data::ReWorkerDetails {
                        output_upload_us: Some(0),
                        worker_wall_time_us: Some(0),
                        elastic_capacity: false,
                        cpu_time_user_us: Some(u64::MAX),
                        cpu_time_system_us: None,
                        execution_attempts: Some(3),
                        final_queue_us: Some(2_000),
                        ..Default::default()
                    })
                }),
                ..Default::default()
            })));
        }
        for kind in kinds {
            let retried =
                matches!(&kind, Some(Command::RemoteCommand(remote)) if remote.re_worker.is_some());
            for previous_digest in ["", "first-command"] {
                let zero = prost_types::Duration {
                    seconds: 0,
                    nanos: 0,
                };
                let mut metadata = buck2_data::CommandExecutionMetadata {
                    wall_time: Some(zero),
                    execution_time: Some(prost_types::Duration {
                        seconds: i64::MAX,
                        nanos: i32::MAX,
                    }),
                    start_time: Some(prost_types::Timestamp {
                        seconds: i64::MIN,
                        nanos: i32::MIN,
                    }),
                    client_start_time: None,
                    queue_duration: Some(prost_types::Duration {
                        seconds: -1,
                        nanos: 0,
                    }),
                    input_materialization_duration: Some(zero),
                    hashing_duration: Some(zero),
                    execution_stats: Some(Default::default()),
                    ..Default::default()
                };
                let stats = metadata.execution_stats.as_mut().unwrap();
                stats.memory_peak = Some(0);
                let end = Box::new(buck2_data::ActionExecutionEnd {
                    execution_kind: 999,
                    commands: vec![
                        buck2_data::CommandExecution {
                            details: Some(buck2_data::CommandExecutionDetails {
                                command_kind: Some(buck2_data::CommandExecutionKind {
                                    command: Some(Command::LocalCommand(
                                        buck2_data::LocalCommand {
                                            action_digest: previous_digest.to_owned(),
                                            ..Default::default()
                                        },
                                    )),
                                }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        buck2_data::CommandExecution {
                            details: Some(buck2_data::CommandExecutionDetails {
                                signed_exit_code: Some(-7),
                                metadata: Some(metadata),
                                command_kind: Some(buck2_data::CommandExecutionKind {
                                    command: kind.clone(),
                                }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                });
                let reference = super::super::action_projection::project_action(
                    3,
                    5,
                    7,
                    Default::default(),
                    end.clone(),
                );
                let projected = Record::End(Box::new(project_end(3, 7, end)));
                projected.heap_bytes(super::super::HeapMode::Verify);
                let Record::End(end) = projected else {
                    unreachable!()
                };
                let action = end.into_action(project_start(3, 0, 5, Default::default()), 0, 0);
                assert_eq!(action, reference.record);
                if retried {
                    assert_eq!(
                        (action.last.attempts, action.last.final_queue_us),
                        (Some(3), Some(2_000))
                    );
                }
            }
        }
    }

    #[test]
    fn output_hash_source_comes_from_the_final_report_not_argv_or_action_kind() {
        use buck2_data::command_execution_kind::Command;
        for (command, local_outputs) in [
            (Some(Command::LocalCommand(Default::default())), true),
            (Some(Command::OmittedLocalCommand(Default::default())), true),
            (Some(Command::WorkerCommand(Default::default())), true),
            (Some(Command::WorkerInitCommand(Default::default())), false),
            (Some(Command::RemoteCommand(Default::default())), false),
            (None, false),
        ] {
            let last = project_last(&buck2_data::CommandExecution {
                details: Some(buck2_data::CommandExecutionDetails {
                    command_kind: Some(buck2_data::CommandExecutionKind { command }),
                    metadata: Some(buck2_data::CommandExecutionMetadata {
                        hashing_duration: Some(prost_types::Duration {
                            seconds: 1,
                            nanos: 0,
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            });
            assert_eq!(last.hashing_local_outputs, local_outputs);
            assert_eq!(last.hashing_us, Some(1_000_000));
        }
    }

    #[test]
    fn negative_action_duration_is_treated_like_a_missing_duration() {
        for duration in [
            None,
            Some(prost_types::Duration {
                seconds: -1,
                nanos: 0,
            }),
            Some(prost_types::Duration {
                seconds: 0,
                nanos: -1,
            }),
        ] {
            let record = project(StreamValue::Event(Box::new(buck2_data::BuckEvent {
                timestamp: Some(prost_types::Timestamp {
                    seconds: 1,
                    nanos: 0,
                }),
                data: Some(buck2_data::buck_event::Data::SpanEnd(
                    buck2_data::SpanEndEvent {
                        duration,
                        data: Some(buck2_data::span_end_event::Data::ActionExecution(
                            Default::default(),
                        )),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            })));
            let Record::End(end) = record else {
                panic!("action end should be normalized");
            };
            assert_eq!(end.dur, 0);
        }
    }

    #[test]
    fn missing_timestamps_and_non_action_markers_remain_exact_raw_values() {
        for value in [
            StreamValue::PartialResult(Box::default()),
            StreamValue::Result(Box::default()),
            StreamValue::Event(Box::new(buck2_data::BuckEvent {
                timestamp: None,
                data: Some(buck2_data::buck_event::Data::SpanStart(
                    buck2_data::SpanStartEvent {
                        data: Some(buck2_data::span_start_event::Data::ActionExecution(
                            Default::default(),
                        )),
                    },
                )),
                ..Default::default()
            })),
            StreamValue::Event(Box::new(buck2_data::BuckEvent {
                timestamp: Some(prost_types::Timestamp {
                    seconds: i64::MAX,
                    nanos: i32::MAX,
                }),
                span_id: u64::MAX,
                parent_id: u64::MAX,
                data: Some(buck2_data::buck_event::Data::SpanStart(
                    buck2_data::SpanStartEvent { data: None },
                )),
                ..Default::default()
            })),
        ] {
            let expected = serde_json::to_value(&value).unwrap();
            let record = project(value);
            record.heap_bytes(super::super::HeapMode::Verify);
            let Record::Raw(value) = record else {
                panic!("unconsumed marker was normalized")
            };
            assert_eq!(serde_json::to_value(value).unwrap(), expected);
        }
    }
}
