/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![allow(deprecated)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::env;
use std::io::Write;

use bazel_bep_proto::blaze;
use bazel_bep_proto::build_event_stream as bep;
use bazel_bep_proto::build_event_stream::build_event;
use bazel_bep_proto::build_event_stream::build_event_id;
use bazel_bep_proto::command_line as cl;
use bazel_bep_proto::devtools::build::lib::packages::metrics as bep_package_metrics;
use bazel_bep_proto::failure_details as failure;
use chrono::DateTime;
use chrono::Utc;
use flate2::Compression;
use flate2::write::GzEncoder;
use prost::Message as _;
use prost_types::Any;
use prost_types::Struct;
use prost_types::Timestamp;
use prost_types::Value;
use prost_types::value;

pub(crate) const BEP_EVENT_TYPE_URL: &str = "type.googleapis.com/build_event_stream.BuildEvent";

const BUILD_TOOL_VERSION: &str = "buck2";
const DEFAULT_CONFIGURATION_ID: &str = "buck2";
const GENERIC_TARGET_KIND: &str = "buck2 rule";
const TEST_TARGET_KIND: &str = "buck2_test rule";
const INTERRUPTED_EXIT_CODE: i32 = 8;
// Bazel's ExitCode values for a build with failed targets and for tests that did not pass.
const BUILD_FAILURE_EXIT_CODE: i32 = 1;
const TESTS_FAILED_EXIT_CODE: i32 = 3;
const MAX_INLINE_FILE_BYTES: usize = 16 * 1024;
const CANONICAL_COMMAND_LINE_LABEL: &str = "canonical";
const ORIGINAL_COMMAND_LINE_LABEL: &str = "original";
const TOOL_COMMAND_LINE_LABEL: &str = "tool";
const STRUCT_TYPE_URL: &str = "type.googleapis.com/google.protobuf.Struct";
const BUILDBUDDY_VISIBILITY_KEY: &str = "VISIBILITY";
const BUILDBUDDY_PUBLIC_VISIBILITY: &str = "PUBLIC";
const COMMAND_PROFILE_NAME: &str = "command.profile.gz";
const MAX_PROFILE_SPANS: usize = 50_000;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TargetKey {
    label: String,
    configuration: String,
}

#[derive(Clone, Debug)]
struct CompletedTargetState {
    success: bool,
}

/// A test run that ended before its per-test results arrived. buck2's test runner reports
/// those after the run's span ends (buck2_test_runner src/runner.rs), so the run's TestResult
/// and TestSummary wait here until its cases are in, or the results end.
#[derive(Debug)]
struct HeldTestEnd {
    event: buck2_data::BuckEvent,
    span_end: buck2_data::SpanEndEvent,
    test_end: buck2_data::TestRunEnd,
    expected_cases: usize,
}

#[derive(Clone, Debug)]
struct TestCaseState {
    name: String,
    status: i32,
    duration: Option<prost_types::Duration>,
    details: String,
    message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FinishedEventSignature {
    children: Vec<Vec<u8>>,
    exit_code: i32,
    exit_name: String,
    overall_success: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum ProfileLane {
    Command,
    Loading,
    Analysis,
    Actions,
    ActionStages,
    Tests,
    Materialization,
    Cache,
    Dice,
    Other,
}

impl ProfileLane {
    fn label(self) -> &'static str {
        match self {
            Self::Command => "Buck2 command",
            Self::Loading => "Buck2 loading",
            Self::Analysis => "Buck2 analysis",
            Self::Actions => "Buck2 actions",
            Self::ActionStages => "Buck2 action stages",
            Self::Tests => "Buck2 tests",
            Self::Materialization => "Buck2 materialization",
            Self::Cache => "Buck2 cache and remote",
            Self::Dice => "Buck2 DICE",
            Self::Other => "Buck2 other",
        }
    }

    fn track_base(self) -> u64 {
        match self {
            Self::Command => 1,
            Self::Loading => 1_000,
            Self::Analysis => 2_000,
            Self::Actions => 3_000,
            Self::ActionStages => 4_000,
            Self::Tests => 5_000,
            Self::Materialization => 6_000,
            Self::Cache => 7_000,
            Self::Dice => 8_000,
            Self::Other => 9_000,
        }
    }

    fn tid(self, index: usize) -> u64 {
        self.track_base()
            .saturating_add(u64::try_from(index).unwrap_or(u64::MAX))
    }
}

#[derive(Clone, Debug)]
struct ProfileOpenSpan {
    span_id: u64,
    parent_id: u64,
    name: String,
    lane: ProfileLane,
    category: &'static str,
    start_us: i64,
    args: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug)]
struct ProfileCompletedSpan {
    span_id: u64,
    parent_id: u64,
    name: String,
    lane: ProfileLane,
    category: &'static str,
    start_us: i64,
    duration_us: i64,
    args: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug)]
struct ProfileSpanDetails {
    name: String,
    lane: ProfileLane,
    category: &'static str,
    args: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug)]
struct ProfileFallbackSpan {
    name: String,
    start_us: i64,
    duration_us: i64,
    args: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Default)]
struct CommandProfileBuilder {
    command_started_at_us: Option<i64>,
    command_name: Option<String>,
    open_spans: BTreeMap<u64, ProfileOpenSpan>,
    completed_spans: Vec<ProfileCompletedSpan>,
    dropped_spans: usize,
}

#[derive(Debug, Default)]
struct ActionDataState {
    actions_executed: u64,
    first_started_ms: Option<i64>,
    last_ended_ms: Option<i64>,
}

impl ActionDataState {
    fn record(&mut self, start_ms: i64, end_ms: i64) {
        self.actions_executed = self.actions_executed.saturating_add(1);
        self.first_started_ms = Some(
            self.first_started_ms
                .map_or(start_ms, |current| current.min(start_ms)),
        );
        self.last_ended_ms = Some(
            self.last_ended_ms
                .map_or(end_ms, |current| current.max(end_ms)),
        );
    }
}

#[derive(Debug, Default)]
struct PackageLoadMetricState {
    load_duration: Option<prost_types::Duration>,
    num_targets: Option<u64>,
    computation_steps: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BesProgressSpanState {
    Started,
    Running,
    Finished,
}

#[derive(Debug, Default)]
struct BesProgressSpanMap<T> {
    spans: BTreeMap<u64, (BesProgressSpanState, T)>,
    running: u64,
    finished: u64,
    cancelled: u64,
    min_started: u64,
    min_finished: u64,
}

impl<T> BesProgressSpanMap<T> {
    fn started(&mut self, span_id: u64, data: T) {
        self.spans
            .insert(span_id, (BesProgressSpanState::Started, data));
        self.cancelled = self.cancelled.saturating_sub(1);
    }

    fn running(&mut self, span_id: u64) -> Option<&mut T> {
        if let Some((state, data)) = self.spans.get_mut(&span_id) {
            if *state == BesProgressSpanState::Started {
                *state = BesProgressSpanState::Running;
                self.running = self.running.saturating_add(1);
            }
            Some(data)
        } else {
            None
        }
    }

    fn finished(&mut self, span_id: u64) -> Option<&mut T> {
        match self.spans.get_mut(&span_id) {
            Some((state, data)) => {
                match state {
                    BesProgressSpanState::Started => {
                        self.finished = self.finished.saturating_add(1);
                    }
                    BesProgressSpanState::Running => {
                        self.running = self.running.saturating_sub(1);
                        self.finished = self.finished.saturating_add(1);
                    }
                    BesProgressSpanState::Finished => {}
                }
                *state = BesProgressSpanState::Finished;
                Some(data)
            }
            None => None,
        }
    }

    fn cancelled(&mut self, span_id: u64) -> Option<T> {
        self.spans.remove(&span_id).map(|(state, data)| {
            match state {
                BesProgressSpanState::Started => {}
                BesProgressSpanState::Running => {
                    self.running = self.running.saturating_sub(1);
                }
                BesProgressSpanState::Finished => {
                    self.finished = self.finished.saturating_sub(1);
                }
            }
            self.cancelled = self.cancelled.saturating_add(1);
            data
        })
    }

    fn stats(&self) -> BesProgressPhaseStats {
        BesProgressPhaseStats {
            started: std::cmp::max(self.min_started, self.spans.len() as u64 + self.cancelled),
            finished: std::cmp::max(self.finished, self.min_finished),
            running: self.running,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct BesProgressPhaseStats {
    started: u64,
    finished: u64,
    running: u64,
}

impl BesProgressPhaseStats {
    fn pending(&self) -> u64 {
        self.started.saturating_sub(self.finished)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BesProgressPhase {
    Loading,
    Analysis,
    Actions,
    Validations,
    Command,
}

#[derive(Clone, Debug, Default)]
struct BesTrackedActionSpan {
    running_local: bool,
    running_remote: bool,
    display: String,
    started_at_us: Option<i64>,
    stages: BTreeMap<u64, BesTrackedActionStage>,
}

#[derive(Clone, Debug)]
struct BesTrackedActionStage {
    display: String,
}

impl BesTrackedActionSpan {
    fn new(action: &buck2_data::ActionExecutionStart, started_at_us: Option<i64>) -> Self {
        Self {
            display: bes_action_display(action),
            started_at_us,
            ..Default::default()
        }
    }

    fn stage_display(&self) -> Option<String> {
        let mut stages = self.stages.values();
        let first = stages.next()?;
        let mut display = first.display.clone();
        let additional = stages.count();
        if additional > 0 {
            display.push_str(&format!(" + {additional}"));
        }
        Some(display)
    }
}

#[derive(Debug, Default)]
struct BesProgressCounters {
    dirs_read: u64,
    targets: u64,
    actions_declared: u64,
    artifacts_declared: u64,
    running_local: u64,
    running_remote: u64,
    exec_time_ms: u64,
    cached_exec_time_ms: u64,
    local_actions: u64,
    remote_actions: u64,
    cached_actions: u64,
    remote_dep_file_cached_actions: u64,
}

impl BesProgressCounters {
    fn total_cached_actions(&self) -> u64 {
        self.cached_actions
            .saturating_add(self.remote_dep_file_cached_actions)
    }

    fn total_actions(&self) -> u64 {
        self.local_actions
            .saturating_add(self.remote_actions)
            .saturating_add(self.total_cached_actions())
    }

    fn total_cache_hit_percentage(&self) -> u8 {
        let total = self.total_actions();
        if total == 0 {
            return 0;
        }
        let rate = self.total_cached_actions() as f64 * 100.0 / total as f64;
        if rate == 0.0 || rate == 100.0 {
            rate as u8
        } else {
            rate.round().clamp(1.0, 99.0) as u8
        }
    }
}

const BES_PROGRESS_MIN_INTERVAL_US: i64 = 1_000_000;
const BES_PROGRESS_MAX_ACTION_ROWS: usize = 10;
const BES_PROGRESS_CLEAR_FROM_CURSOR: &str = "\x1b[J";
const BES_PROGRESS_SEPARATOR: &str = "------------------------------------------------------------------------------------------------------------------";

#[derive(Clone, Debug)]
struct BesProgressTimedSnapshot {
    timestamp_us: i64,
    snapshot: buck2_data::Snapshot,
}

#[derive(Debug, Default)]
struct BesProgressState {
    build_id: Option<String>,
    command_name: Option<String>,
    command_started_at_us: Option<i64>,
    command_duration_us: Option<i64>,
    re_session_id: Option<String>,
    first_snapshot: Option<buck2_data::Snapshot>,
    penultimate_snapshot: Option<BesProgressTimedSnapshot>,
    last_snapshot: Option<BesProgressTimedSnapshot>,
    loads: BesProgressSpanMap<()>,
    analyses: BesProgressSpanMap<()>,
    actions: BesProgressSpanMap<BesTrackedActionSpan>,
    validations: BesProgressSpanMap<()>,
    counters: BesProgressCounters,
    last_emitted_at_us: Option<i64>,
    last_rendered: Option<String>,
    last_rendered_line_count: usize,
}

impl BesProgressState {
    fn handle_event(&mut self, event: &buck2_data::BuckEvent) -> Option<String> {
        let before_phase = self.current_phase();
        let before_pending = self.pending_work();
        let allow_periodic = progress_event_can_render(event);
        let mut force = false;
        if self.build_id.is_none() && !event.trace_id.is_empty() {
            self.build_id = Some(event.trace_id.clone());
            force = true;
        }

        match event.data.as_ref() {
            Some(buck2_data::buck_event::Data::SpanStart(span_start)) => {
                self.handle_span_start(event, span_start, &mut force);
            }
            Some(buck2_data::buck_event::Data::SpanEnd(span_end)) => {
                self.handle_span_end(event, span_end, &mut force);
            }
            Some(buck2_data::buck_event::Data::Instant(instant)) => {
                self.handle_instant(event, instant, &mut force);
            }
            Some(buck2_data::buck_event::Data::Record(record)) => {
                self.handle_record(record, &mut force);
            }
            None => {}
        }

        let after_phase = self.current_phase();
        let after_pending = self.pending_work();
        force |= before_phase != after_phase;
        force |= before_pending > 0 && after_pending == 0;

        self.maybe_render(event, force, allow_periodic)
    }

    fn handle_span_start(
        &mut self,
        event: &buck2_data::BuckEvent,
        span_start: &buck2_data::SpanStartEvent,
        force: &mut bool,
    ) {
        match span_start.data.as_ref() {
            Some(buck2_data::span_start_event::Data::Command(command)) => {
                self.command_name = Some(command_name(command));
                self.command_started_at_us = timestamp_micros(event.timestamp.as_ref());
                self.command_duration_us = None;
                *force = true;
            }
            Some(buck2_data::span_start_event::Data::Load(_)) => {
                self.loads.started(event.span_id, ());
                self.loads.running(event.span_id);
            }
            Some(buck2_data::span_start_event::Data::Analysis(_)) => {
                self.analyses.started(event.span_id, ());
            }
            Some(buck2_data::span_start_event::Data::AnalysisStage(stage)) => {
                if matches!(
                    stage.stage.as_ref(),
                    Some(buck2_data::analysis_stage_start::Stage::EvaluateRule(_))
                ) {
                    self.analyses.running(event.parent_id);
                }
            }
            Some(buck2_data::span_start_event::Data::ActionExecution(action)) => {
                self.actions.started(
                    event.span_id,
                    BesTrackedActionSpan::new(action, timestamp_micros(event.timestamp.as_ref())),
                );
            }
            Some(buck2_data::span_start_event::Data::ExecutorStage(stage)) => {
                self.handle_executor_stage_start(event, stage);
            }
            _ => {}
        }
    }

    fn handle_span_end(
        &mut self,
        event: &buck2_data::BuckEvent,
        span_end: &buck2_data::SpanEndEvent,
        force: &mut bool,
    ) {
        match span_end.data.as_ref() {
            Some(buck2_data::span_end_event::Data::Command(command)) => {
                if self.command_name.is_none() {
                    self.command_name = Some(command_end_name(command).to_owned());
                }
                self.command_duration_us =
                    span_end.duration.as_ref().map(duration_micros).or_else(|| {
                        self.command_started_at_us
                            .zip(timestamp_micros(event.timestamp.as_ref()))
                            .map(|(start, end)| end.saturating_sub(start).max(0))
                    });
                *force = true;
            }
            Some(buck2_data::span_end_event::Data::Load(load)) => {
                self.loads.finished(event.span_id);
                if let Some(target_count) = load.target_count {
                    self.counters.targets = self.counters.targets.saturating_add(target_count);
                }
            }
            Some(buck2_data::span_end_event::Data::Analysis(analysis)) => {
                self.counters.actions_declared = self
                    .counters
                    .actions_declared
                    .saturating_add(analysis.declared_actions.unwrap_or_default());
                self.counters.artifacts_declared = self
                    .counters
                    .artifacts_declared
                    .saturating_add(analysis.declared_artifacts.unwrap_or_default());
                self.analyses.finished(event.span_id);
            }
            Some(buck2_data::span_end_event::Data::ActionExecution(action)) => {
                if let Some(data) = self.actions.finished(event.span_id) {
                    let data = data.clone();
                    self.action_finished(data);
                }
                self.record_completed_action(action);
            }
            Some(buck2_data::span_end_event::Data::ExecutorStage(_)) => {
                if let Some((_, action)) = self.actions.spans.get_mut(&event.parent_id) {
                    action.stages.remove(&event.span_id);
                }
            }
            Some(buck2_data::span_end_event::Data::SpanCancelled(_)) => {
                self.loads.cancelled(event.span_id);
                self.analyses.cancelled(event.span_id);
                if let Some(data) = self.actions.cancelled(event.span_id) {
                    self.action_finished(data);
                }
            }
            _ => {}
        }
    }

    fn handle_instant(
        &mut self,
        event: &buck2_data::BuckEvent,
        instant: &buck2_data::InstantEvent,
        force: &mut bool,
    ) {
        match instant.data.as_ref() {
            Some(buck2_data::instant_event::Data::DiceStateSnapshot(snapshot)) => {
                self.handle_dice_state_snapshot(snapshot);
            }
            Some(buck2_data::instant_event::Data::ReSession(session)) => {
                if !session.session_id.is_empty() {
                    self.re_session_id = Some(session.session_id.clone());
                    *force = true;
                }
            }
            Some(buck2_data::instant_event::Data::Snapshot(snapshot)) => {
                self.handle_snapshot(event, snapshot);
            }
            _ => {}
        }
    }

    fn handle_record(&mut self, record: &buck2_data::RecordEvent, force: &mut bool) {
        if let Some(buck2_data::record_event::Data::InvocationRecord(record)) = record.data.as_ref()
        {
            self.command_name = Some(invocation_command_name(record));
            if let Some(command_duration) = record.command_duration.as_ref() {
                self.command_duration_us = Some(duration_micros(command_duration));
            }
            let remote_cache_hits = record
                .run_action_cache_count
                .saturating_add(record.run_remote_dep_file_cache_count);
            let actions = record
                .run_local_count
                .saturating_add(record.run_remote_count)
                .saturating_add(remote_cache_hits)
                .saturating_add(record.run_skipped_count);
            self.actions.min_started = self.actions.min_started.max(actions);
            self.actions.min_finished = self.actions.min_finished.max(actions);
            self.counters.local_actions = self.counters.local_actions.max(record.run_local_count);
            self.counters.remote_actions =
                self.counters.remote_actions.max(record.run_remote_count);
            self.counters.cached_actions = self
                .counters
                .cached_actions
                .max(record.run_action_cache_count);
            self.counters.remote_dep_file_cached_actions = self
                .counters
                .remote_dep_file_cached_actions
                .max(record.run_remote_dep_file_cache_count);
            *force = true;
        }
    }

    fn handle_executor_stage_start(
        &mut self,
        event: &buck2_data::BuckEvent,
        stage: &buck2_data::ExecutorStageStart,
    ) {
        let stage_display = bes_executor_stage_display(stage.stage.as_ref());
        let Some(action) = self.actions.running(event.parent_id) else {
            return;
        };
        if let Some(display) = stage_display {
            action
                .stages
                .insert(event.span_id, BesTrackedActionStage { display });
        }
        match stage.stage.as_ref() {
            Some(buck2_data::executor_stage_start::Stage::Re(re_stage))
                if matches!(
                    re_stage.stage.as_ref(),
                    Some(buck2_data::re_stage::Stage::Execute(_))
                ) =>
            {
                if !action.running_remote {
                    action.running_remote = true;
                    self.counters.running_remote = self.counters.running_remote.saturating_add(1);
                }
            }
            Some(buck2_data::executor_stage_start::Stage::Local(local_stage))
                if matches!(
                    local_stage.stage.as_ref(),
                    Some(buck2_data::local_stage::Stage::Execute(_))
                ) =>
            {
                if !action.running_local {
                    action.running_local = true;
                    self.counters.running_local = self.counters.running_local.saturating_add(1);
                }
            }
            _ => {}
        }
    }

    fn handle_dice_state_snapshot(&mut self, snapshot: &buck2_data::DiceStateSnapshot) {
        if let Some(read_dir_states) = snapshot.key_states.get("ReadDirKey") {
            self.counters.dirs_read = read_dir_states.finished as u64;
        }

        let mut analysis_min_started = 0;
        let mut analysis_min_finished = 0;
        for key in ["AnalysisKey", "AnonTargetKey", "DeferredCompute"] {
            if let Some(states) = snapshot.key_states.get(key) {
                analysis_min_started += states.started as u64;
                analysis_min_finished += states.finished as u64;
            }
        }
        self.analyses.min_started = analysis_min_started;
        self.analyses.min_finished = analysis_min_finished;

        if let Some(states) = snapshot.key_states.get("BuildKey") {
            self.actions.min_started = states.started as u64;
            self.actions.min_finished = states.finished as u64;
        }

        if let Some(states) = snapshot.key_states.get("SingleValidationKey") {
            self.validations.min_started = states.started as u64;
            self.validations.min_finished = states.finished as u64;
        }
    }

    fn handle_snapshot(&mut self, event: &buck2_data::BuckEvent, snapshot: &buck2_data::Snapshot) {
        if self.first_snapshot.is_none() {
            self.first_snapshot = Some(snapshot.clone());
        }
        let Some(timestamp_us) = timestamp_micros(event.timestamp.as_ref()) else {
            return;
        };
        let snapshot = BesProgressTimedSnapshot {
            timestamp_us,
            snapshot: snapshot.clone(),
        };
        self.penultimate_snapshot = self.last_snapshot.replace(snapshot);
    }

    fn action_finished(&mut self, action: BesTrackedActionSpan) {
        if action.running_local {
            self.counters.running_local = self.counters.running_local.saturating_sub(1);
        }
        if action.running_remote {
            self.counters.running_remote = self.counters.running_remote.saturating_sub(1);
        }
    }

    fn record_completed_action(&mut self, action: &buck2_data::ActionExecutionEnd) {
        let exec_time_ms = last_command_execution_time_ms(action);
        self.counters.exec_time_ms = self.counters.exec_time_ms.saturating_add(exec_time_ms);

        if action.kind != buck2_data::ActionKind::Run as i32 {
            return;
        }

        match buck2_data::ActionExecutionKind::try_from(action.execution_kind) {
            Ok(buck2_data::ActionExecutionKind::Local)
            | Ok(buck2_data::ActionExecutionKind::LocalWorker) => {
                self.counters.local_actions = self.counters.local_actions.saturating_add(1);
            }
            Ok(buck2_data::ActionExecutionKind::Remote)
            | Ok(buck2_data::ActionExecutionKind::RemoteWorker) => {
                self.counters.remote_actions = self.counters.remote_actions.saturating_add(1);
            }
            Ok(buck2_data::ActionExecutionKind::ActionCache) => {
                self.counters.cached_actions = self.counters.cached_actions.saturating_add(1);
                self.counters.cached_exec_time_ms = self
                    .counters
                    .cached_exec_time_ms
                    .saturating_add(exec_time_ms);
            }
            Ok(buck2_data::ActionExecutionKind::RemoteDepFileCache) => {
                self.counters.remote_dep_file_cached_actions = self
                    .counters
                    .remote_dep_file_cached_actions
                    .saturating_add(1);
                self.counters.cached_exec_time_ms = self
                    .counters
                    .cached_exec_time_ms
                    .saturating_add(exec_time_ms);
            }
            _ => {}
        }
    }

    fn maybe_render(
        &mut self,
        event: &buck2_data::BuckEvent,
        force: bool,
        allow_periodic: bool,
    ) -> Option<String> {
        let now_us = timestamp_micros(event.timestamp.as_ref());
        let due = self
            .last_emitted_at_us
            .zip(now_us)
            .is_none_or(|(last, now)| now.saturating_sub(last) >= BES_PROGRESS_MIN_INTERVAL_US);
        if !force && (!allow_periodic || !due) {
            return None;
        }
        if self.command_name.is_none() && !self.has_progress() {
            return None;
        }

        let rendered = self.render(now_us);
        if rendered.is_empty() || self.last_rendered.as_ref() == Some(&rendered) {
            return None;
        }
        let rendered_line_count = rendered.lines().count();
        let mut output = if self.last_rendered_line_count > 0 {
            // BuildBuddy's event log writer interprets ANSI cursor control, so
            // rewrite the previous progress block instead of appending another
            // full console snapshot.
            format!(
                "\x1b[{}A{}{}",
                self.last_rendered_line_count, BES_PROGRESS_CLEAR_FROM_CURSOR, rendered
            )
        } else {
            rendered.clone()
        };
        output.push('\n');
        self.last_rendered = Some(rendered);
        self.last_rendered_line_count = rendered_line_count;
        self.last_emitted_at_us = now_us;
        Some(output)
    }

    fn has_progress(&self) -> bool {
        self.loads.stats().started > 0
            || self.analyses.stats().started > 0
            || self.actions.stats().started > 0
            || self.validations.stats().started > 0
    }

    fn render(&self, now_us: Option<i64>) -> String {
        let mut lines = Vec::new();
        let loads = self.loads.stats();
        let analyses = self.analyses.stats();
        let actions = self.actions.stats();
        let validations = self.validations.stats();

        if let Some(build_id) = self.build_id.as_deref() {
            lines.push(format!("Build ID:  {build_id}"));
        }
        if let Some(network) = self.network_header() {
            lines.push(network);
        }
        if loads.started > 0 {
            let mut line = render_progress_phase("Loading targets.    ", &loads, loads.running);
            let extra = self.render_loads_extra();
            if !extra.is_empty() {
                line.push_str("  ");
                line.push_str(&extra);
            }
            lines.push(line);
        }
        if analyses.started > 0 {
            let mut line =
                render_progress_phase("Analyzing targets.  ", &analyses, analyses.running);
            let extra = self.render_analyses_extra();
            if !extra.is_empty() {
                line.push_str("  ");
                line.push_str(&extra);
            }
            lines.push(line);
        }
        if actions.started == 0 {
            lines.push(self.render_command_summary(now_us));
        } else {
            let mut line = self.render_actions(&actions);
            let extra = self.render_actions_extra();
            if !extra.is_empty() {
                line.push_str("  ");
                line.push_str(&extra);
            }
            lines.push(line);
            if validations.started > 0 {
                lines.push(render_progress_phase(
                    "Running validations.",
                    &validations,
                    validations.running,
                ));
            }
            lines.push(self.render_command_summary(now_us));
            let active_actions = self.render_active_actions(now_us);
            if !active_actions.is_empty() {
                lines.push(BES_PROGRESS_SEPARATOR.to_owned());
                lines.extend(active_actions);
            }
        }

        lines.join("\n")
    }

    fn render_actions(&self, actions: &BesProgressPhaseStats) -> String {
        let mut running = Vec::new();
        if self.counters.running_local > 0 || self.counters.local_actions > 0 {
            running.push(format!("{} local", self.counters.running_local));
        }
        if self.counters.running_remote > 0 || self.counters.remote_actions > 0 {
            running.push(format!("{} remote", self.counters.running_remote));
        }
        let running = if running.is_empty() {
            "0".to_owned()
        } else {
            running.join(", ")
        };
        render_progress_phase_with_running("Executing actions.  ", actions, &running)
    }

    fn render_active_actions(&self, now_us: Option<i64>) -> Vec<String> {
        let mut actions = self
            .actions
            .spans
            .iter()
            .filter(|(_, (state, _))| *state != BesProgressSpanState::Finished)
            .collect::<Vec<_>>();
        actions.sort_by_key(|entry| {
            let (span_id, (_, action)) = *entry;
            (action.started_at_us.unwrap_or(i64::MAX), *span_id)
        });

        let mut lines = actions
            .iter()
            .take(BES_PROGRESS_MAX_ACTION_ROWS)
            .map(|entry| {
                let (_, (_, action)) = *entry;
                render_active_action(action, now_us)
            })
            .collect::<Vec<_>>();
        let hidden = actions.len().saturating_sub(BES_PROGRESS_MAX_ACTION_ROWS);
        if hidden > 0 {
            lines.push(format!("... and {hidden} more currently executing"));
        }
        lines
    }

    fn render_loads_extra(&self) -> String {
        let mut extra = Vec::new();
        if self.counters.dirs_read > 0 {
            extra.push(format!("{} dirs read", self.counters.dirs_read));
        }
        if self.counters.targets > 0 {
            extra.push(format!("{} targets declared", self.counters.targets));
        }
        extra.join(", ")
    }

    fn render_analyses_extra(&self) -> String {
        let mut extra = Vec::new();
        if self.counters.actions_declared > 0 {
            extra.push(format!("{} actions", self.counters.actions_declared));
        }
        if self.counters.artifacts_declared > 0 {
            extra.push(format!(
                "{} artifacts declared",
                self.counters.artifacts_declared
            ));
        }
        extra.join(", ")
    }

    fn render_actions_extra(&self) -> String {
        if self.counters.exec_time_ms == 0 {
            String::new()
        } else {
            format!("{}ms exec time total", self.counters.exec_time_ms)
        }
    }

    fn render_action_stats(&self) -> String {
        let mut res_types = Vec::new();
        if self.counters.local_actions > 0 {
            res_types.push(format!("{} local", self.counters.local_actions));
        }
        if self.counters.remote_actions > 0 {
            res_types.push(format!("{} remote", self.counters.remote_actions));
        }
        if self.counters.total_cached_actions() > 0 {
            res_types.push(format!(
                "{} cache ({}% hit)",
                self.counters.total_cached_actions(),
                self.counters.total_cache_hit_percentage()
            ));
        }
        if res_types.is_empty() {
            String::new()
        } else {
            format!("Finished {}", res_types.join(", "))
        }
    }

    fn command_header(&self) -> String {
        format!(
            "Command: {}.",
            self.command_name.as_deref().unwrap_or("unknown")
        )
    }

    fn render_command_summary(&self, now_us: Option<i64>) -> String {
        let mut line = self.command_header();
        let action_stats = self.render_action_stats();
        if !action_stats.is_empty() {
            line = format!("{line:<18} {action_stats}").trim_end().to_owned();
        }
        if let Some(elapsed) = self.command_elapsed(now_us) {
            line.push_str("     Time elapsed: ");
            line.push_str(&format_bes_command_duration_us(elapsed));
        }
        line
    }

    fn command_elapsed(&self, now_us: Option<i64>) -> Option<i64> {
        self.command_duration_us.or_else(|| {
            self.command_started_at_us
                .zip(now_us)
                .map(|(start, now)| now.saturating_sub(start).max(0))
        })
    }

    fn network_header(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let (Some(first), Some(last)) = (&self.first_snapshot, &self.last_snapshot)
            && (last.snapshot.re_upload_bytes > 0
                || last.snapshot.re_download_bytes > 0
                || last.snapshot.http_download_bytes > 0)
        {
            let upload_bytes = last
                .snapshot
                .re_upload_bytes
                .saturating_sub(first.re_upload_bytes);
            let download_bytes = last
                .snapshot
                .re_download_bytes
                .saturating_add(last.snapshot.http_download_bytes)
                .saturating_sub(
                    first
                        .re_download_bytes
                        .saturating_add(first.http_download_bytes),
                );
            let upload_rate = self
                .snapshot_bytes_per_second(|snapshot| snapshot.re_upload_bytes)
                .unwrap_or_default();
            let download_rate = self
                .snapshot_bytes_per_second(|snapshot| {
                    snapshot
                        .re_download_bytes
                        .saturating_add(snapshot.http_download_bytes)
                })
                .unwrap_or_default();
            parts.push(format!(
                "Up: {} {}  Down: {} {}",
                format_bes_bytes_fixed(upload_bytes),
                format_bes_bytes_per_second_fixed(upload_rate),
                format_bes_bytes_fixed(download_bytes),
                format_bes_bytes_per_second_fixed(download_rate),
            ));
        }
        if let Some(session_id) = self.re_session_id.as_deref() {
            parts.push(format!("({session_id})"));
        }
        if parts.is_empty() {
            None
        } else {
            Some(format!("Network: {}", parts.join("  ")))
        }
    }

    fn snapshot_bytes_per_second(
        &self,
        field: impl Fn(&buck2_data::Snapshot) -> u64,
    ) -> Option<u64> {
        let previous = self.penultimate_snapshot.as_ref()?;
        let last = self.last_snapshot.as_ref()?;
        let elapsed_us = last.timestamp_us.checked_sub(previous.timestamp_us)?;
        if elapsed_us <= 0 {
            return None;
        }
        let delta = field(&last.snapshot).checked_sub(field(&previous.snapshot))?;
        Some(delta.saturating_mul(1_000_000) / elapsed_us as u64)
    }

    fn current_phase(&self) -> BesProgressPhase {
        if self.actions.stats().pending() > 0 {
            BesProgressPhase::Actions
        } else if self.validations.stats().pending() > 0 {
            BesProgressPhase::Validations
        } else if self.analyses.stats().pending() > 0 {
            BesProgressPhase::Analysis
        } else if self.loads.stats().pending() > 0 {
            BesProgressPhase::Loading
        } else {
            BesProgressPhase::Command
        }
    }

    fn pending_work(&self) -> u64 {
        self.loads
            .stats()
            .pending()
            .saturating_add(self.analyses.stats().pending())
            .saturating_add(self.actions.stats().pending())
            .saturating_add(self.validations.stats().pending())
    }
}

fn progress_event_can_render(event: &buck2_data::BuckEvent) -> bool {
    match event.data.as_ref() {
        Some(buck2_data::buck_event::Data::SpanStart(_))
        | Some(buck2_data::buck_event::Data::SpanEnd(_))
        | Some(buck2_data::buck_event::Data::Record(_)) => true,
        Some(buck2_data::buck_event::Data::Instant(instant)) => matches!(
            instant.data.as_ref(),
            Some(
                buck2_data::instant_event::Data::DiceStateSnapshot(_)
                    | buck2_data::instant_event::Data::ReSession(_)
                    | buck2_data::instant_event::Data::Snapshot(_)
            )
        ),
        None => false,
    }
}

fn render_active_action(action: &BesTrackedActionSpan, now_us: Option<i64>) -> String {
    let mut line = if action.display.is_empty() {
        "action".to_owned()
    } else {
        action.display.clone()
    };
    if let Some(stage) = action.stage_display() {
        line.push_str(&format!(" [{stage}]"));
    }
    if let Some(duration) = bes_progress_elapsed(action.started_at_us, now_us) {
        line.push(' ');
        line.push_str(&duration);
    }
    line
}

fn bes_action_display(action: &buck2_data::ActionExecutionStart) -> String {
    let name = bes_action_name(action.name.as_ref(), action.kind);
    match action
        .key
        .as_ref()
        .and_then(action_owner)
        .and_then(label_for_configured_target)
    {
        Some(label) => format!("{label} -- action ({name})"),
        None => format!("action ({name})"),
    }
}

fn bes_action_name(name: Option<&buck2_data::ActionName>, kind: i32) -> String {
    let category = profile_action_mnemonic(name, kind);
    match name
        .map(|name| name.identifier.as_str())
        .filter(|identifier| !identifier.is_empty())
    {
        Some(identifier) => format!("{category} {identifier}"),
        None => category,
    }
}

fn bes_executor_stage_display(
    stage: Option<&buck2_data::executor_stage_start::Stage>,
) -> Option<String> {
    profile_executor_stage_name(stage).map(|name| name.replace(' ', "_").replace('-', "_"))
}

fn bes_progress_elapsed(started_at_us: Option<i64>, now_us: Option<i64>) -> Option<String> {
    let started_at_us = started_at_us?;
    let now_us = now_us?;
    Some(format_bes_progress_duration_us(
        now_us.saturating_sub(started_at_us).max(0),
    ))
}

fn format_bes_progress_duration_us(elapsed_us: i64) -> String {
    let elapsed_seconds = elapsed_us / 1_000_000;
    if elapsed_seconds < 60 {
        return format!("{:.1}s", elapsed_us as f64 / 1_000_000.0);
    }
    if elapsed_seconds < 60 * 60 {
        return format!("{}m{:02}s", elapsed_seconds / 60, elapsed_seconds % 60);
    }
    format!(
        "{}h{:02}m",
        elapsed_seconds / (60 * 60),
        (elapsed_seconds % (60 * 60)) / 60
    )
}

fn format_bes_command_duration_us(elapsed_us: i64) -> String {
    let elapsed_us = elapsed_us.max(0);
    let elapsed_seconds = elapsed_us / 1_000_000;
    if elapsed_seconds < 60 {
        return format!("{:.1}s", elapsed_us as f64 / 1_000_000.0);
    }
    if elapsed_seconds < 60 * 60 {
        let minutes = elapsed_seconds / 60;
        let seconds_us = elapsed_us.saturating_sub(minutes * 60 * 1_000_000);
        return format!("{minutes}:{:04.1}s", seconds_us as f64 / 1_000_000.0);
    }
    format!(
        "{}:{:02}:{:02}",
        elapsed_seconds / (60 * 60),
        (elapsed_seconds % (60 * 60)) / 60,
        elapsed_seconds % 60
    )
}

fn format_bes_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}B")
    } else {
        format!("{value:.0}{}", UNITS[unit])
    }
}

fn format_bes_bytes_fixed(bytes: u64) -> String {
    format!("{:>6}", format_bes_bytes(bytes))
}

fn format_bes_bytes_per_second_fixed(bytes_per_second: u64) -> String {
    if bytes_per_second == 0 {
        " ".repeat(8)
    } else {
        format!("{:>8}", format!("{}/s", format_bes_bytes(bytes_per_second)))
    }
}

fn render_progress_phase(header: &str, stats: &BesProgressPhaseStats, running: u64) -> String {
    render_progress_phase_with_running(header, stats, &running.to_string())
}

fn render_progress_phase_with_running(
    header: &str,
    stats: &BesProgressPhaseStats,
    running: &str,
) -> String {
    format!(
        "{header} Remaining {:>5}/{:<5} (running: {running})",
        stats.pending(),
        stats.started,
    )
}

#[derive(Debug, Default)]
pub(crate) struct BazelEventConverter {
    build_metadata: BTreeMap<String, String>,
    skip_successful_action_events: bool,
    saw_started: bool,
    saw_finished: bool,
    progress_count: i32,
    completed_targets: BTreeMap<TargetKey, CompletedTargetState>,
    action_children: BTreeMap<TargetKey, BTreeMap<String, bep::BuildEventId>>,
    test_children: BTreeMap<TargetKey, BTreeMap<String, bep::BuildEventId>>,
    emitted_action_child_counts: BTreeMap<TargetKey, usize>,
    emitted_test_child_counts: BTreeMap<TargetKey, usize>,
    emitted_configured_targets: BTreeMap<String, Vec<u8>>,
    target_outputs: BTreeMap<TargetKey, BTreeMap<String, bep::File>>,
    target_directory_outputs: BTreeMap<TargetKey, BTreeMap<String, bep::File>>,
    target_test_statuses: BTreeMap<TargetKey, i32>,
    target_tags: BTreeMap<TargetKey, Vec<String>>,
    emitted_output_counts: BTreeMap<TargetKey, usize>,
    emitted_completed_targets: BTreeSet<TargetKey>,
    test_cases: BTreeMap<TargetKey, Vec<TestCaseState>>,
    held_test_ends: BTreeMap<TargetKey, HeldTestEnd>,
    test_progress_counts: BTreeMap<TargetKey, i32>,
    test_timeouts: BTreeMap<TargetKey, prost_types::Duration>,
    announced_event_ids: BTreeMap<Vec<u8>, bep::BuildEventId>,
    posted_event_ids: BTreeSet<Vec<u8>>,
    configured_target_labels: BTreeSet<String>,
    pending_patterns: Option<Vec<String>>,
    pattern_expanded_emitted: bool,
    defer_target_setup_until_first_result: bool,
    pending_workspace_status: Option<bep::BuildEvent>,
    pending_configured_events: Vec<bep::BuildEvent>,
    final_progress_emitted: bool,
    emitted_build_tool_logs: bool,
    emitted_convenience_symlinks: bool,
    last_finished_signature: Option<FinishedEventSignature>,
    last_build_tool_logs_signature: Option<Vec<u8>>,
    declared_actions_count: u64,
    declared_artifacts_count: u64,
    action_data: BTreeMap<String, ActionDataState>,
    top_level_targets: BTreeSet<TargetKey>,
    loaded_packages: BTreeSet<String>,
    package_load_metrics: BTreeMap<String, PackageLoadMetricState>,
    output_artifacts_seen_size: u64,
    output_artifacts_seen_count: u64,
    output_artifacts_from_action_cache_size: u64,
    output_artifacts_from_action_cache_count: u64,
    command_profile: CommandProfileBuilder,
    bes_progress: BesProgressState,
    pending_pre_start_events: Vec<bep::BuildEvent>,
    /// The test executor's exit code from EndOfTestResults, which the client turns into
    /// buck2's own exit code when the build had no errors.
    test_executor_exit_code: Option<i32>,
    saw_failing_test_result: bool,
}

impl BazelEventConverter {
    pub(crate) fn new_with_options<I>(
        build_metadata: I,
        upload_successful_action_events: bool,
    ) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        Self {
            build_metadata: build_metadata.into_iter().collect(),
            skip_successful_action_events: !upload_successful_action_events,
            ..Self::default()
        }
    }

    pub(crate) fn convert(
        &mut self,
        sequence_hint: i64,
        event: &buck2_data::BuckEvent,
    ) -> Vec<bep::BuildEvent> {
        let mut events = Vec::new();

        if !self.saw_started
            && let Some(record) = invocation_record(event)
        {
            events.push(self.started_event_from_invocation(event, record));
            self.saw_started = true;
        }

        if !self.saw_started
            && let Some(command) = command_start(event)
        {
            events.push(self.started_event(event, Some(command)));
            self.saw_started = true;
        }

        match event.data.as_ref() {
            Some(buck2_data::buck_event::Data::SpanStart(span_start)) => {
                self.convert_span_start(event, span_start, &mut events);
            }
            Some(buck2_data::buck_event::Data::SpanEnd(span_end)) => {
                self.convert_span_end(event, span_end, &mut events);
            }
            Some(buck2_data::buck_event::Data::Instant(instant)) => {
                self.convert_instant(sequence_hint, event, instant, &mut events);
            }
            Some(buck2_data::buck_event::Data::Record(record)) => {
                self.convert_record(event, record, &mut events);
            }
            None => {}
        }

        if !self.final_progress_emitted
            && let Some(progress) = self.bes_progress.handle_event(event)
        {
            events.push(self.progress_event(sequence_hint, None, Some(progress)));
        }

        self.emit_pending_target_setup_before_target_events(&mut events);

        if !self.saw_started {
            if should_buffer_before_started(event) {
                self.pending_pre_start_events.extend(events);
                return Vec::new();
            }
            events.splice(
                0..0,
                std::iter::once(self.started_event(event, None))
                    .chain(std::mem::take(&mut self.pending_pre_start_events)),
            );
            self.saw_started = true;
        } else if !self.pending_pre_start_events.is_empty() {
            let insert_at = usize::from(
                events
                    .first()
                    .is_some_and(|event| event.id.as_ref() == Some(&started_id())),
            );
            events.splice(
                insert_at..insert_at,
                std::mem::take(&mut self.pending_pre_start_events),
            );
        }

        let finishes_invocation = events.iter().any(is_build_finished_event);
        self.record_event_graph(&events);
        if finishes_invocation {
            self.emit_aborted_events_for_missing_ids(&mut events);
        }

        events
    }

    fn convert_span_start(
        &mut self,
        event: &buck2_data::BuckEvent,
        span_start: &buck2_data::SpanStartEvent,
        events: &mut Vec<bep::BuildEvent>,
    ) {
        self.command_profile.record_span_start(event, span_start);

        match span_start.data.as_ref() {
            Some(buck2_data::span_start_event::Data::Command(command)) => {
                if !self.saw_started {
                    events.push(self.started_event(event, Some(command)));
                    self.saw_started = true;
                }
                self.remember_cli_target_patterns(command);
                self.defer_target_setup_until_first_result =
                    is_test_or_coverage_command(&command_name(command));
                events.push(unstructured_command_line_event(command));
                events.push(original_structured_command_line_event(command));
                events.push(canonical_structured_command_line_event(command));
                events.push(tool_structured_command_line_event());
                events.push(options_parsed_event(command));
                events.push(default_configuration_event());
                events.push(workspace_info_event(command));
                self.push_or_defer_workspace_status(
                    workspace_status_event(command, event.timestamp.as_ref()),
                    events,
                );
                events.push(build_metadata_event(&build_metadata_from_command(
                    command,
                    &self.build_metadata,
                )));
            }
            Some(buck2_data::span_start_event::Data::CommandCritical(command)) => {
                let metadata = metadata_map_from_hash_map(&command.metadata);
                if !metadata.is_empty() || !self.build_metadata.is_empty() {
                    events.push(self.build_metadata_event(&metadata));
                }
            }
            Some(buck2_data::span_start_event::Data::Analysis(analysis)) => {
                self.push_configured_event(configured_event_from_analysis_start(analysis), events);
            }
            Some(buck2_data::span_start_event::Data::TestDiscovery(test_discovery)) => {
                self.remember_test_tags(
                    test_discovery.target_label.as_ref(),
                    &test_discovery.labels,
                );
                self.push_configured_event(
                    configured_event_from_test_label(
                        test_discovery.target_label.as_ref(),
                        TEST_TARGET_KIND,
                        &test_discovery.labels,
                    ),
                    events,
                );
            }
            Some(buck2_data::span_start_event::Data::TestRun(test_run)) => {
                if let Some(suite) = test_run.suite.as_ref() {
                    self.remember_test_tags(suite.target_label.as_ref(), &suite.labels);
                    self.push_configured_event(configured_event_from_test_suite(suite), events);
                }
            }
            _ => {}
        }
    }

    fn convert_span_end(
        &mut self,
        event: &buck2_data::BuckEvent,
        span_end: &buck2_data::SpanEndEvent,
        events: &mut Vec<bep::BuildEvent>,
    ) {
        let tests_failed = self.tests_failed();
        self.command_profile
            .record_span_end(event, span_end, tests_failed);

        match span_end.data.as_ref() {
            Some(buck2_data::span_end_event::Data::Command(command)) => {
                self.push_all_held_test_events(events);
                self.emit_pending_pattern_expanded(&[], events, true);
                self.emit_completed_updates_for_actions(events);
                self.push_convenience_symlinks_identified(&[], events);
                let outcome = command_outcome(command, tests_failed);
                self.push_finished(finished_event_from_command_end(event, outcome), events);
                self.push_final_progress(events);
                self.push_build_tool_logs(
                    build_tool_logs_event_from_command_end(
                        event,
                        span_end,
                        command,
                        outcome,
                        &self.command_profile,
                    ),
                    events,
                );
            }
            Some(buck2_data::span_end_event::Data::Analysis(analysis)) => {
                if let Some(declared_actions) = analysis.declared_actions {
                    self.declared_actions_count =
                        self.declared_actions_count.saturating_add(declared_actions);
                }
                if let Some(declared_artifacts) = analysis.declared_artifacts {
                    self.declared_artifacts_count = self
                        .declared_artifacts_count
                        .saturating_add(declared_artifacts);
                }
                if let Some(completed) = completed_event_from_analysis_end(analysis) {
                    self.remember_completed(&completed);
                }
            }
            Some(buck2_data::span_end_event::Data::LoadPackage(load)) => {
                if !load.path.is_empty() {
                    self.loaded_packages.insert(load.path.clone());
                    let metrics = self
                        .package_load_metrics
                        .entry(load.path.clone())
                        .or_default();
                    if metrics.load_duration.is_none() {
                        metrics.load_duration = span_end.duration.clone();
                    }
                }
            }
            Some(buck2_data::span_end_event::Data::Load(load)) => {
                if (load.target_count.is_some() || load.starlark_tick_count.is_some())
                    && let Some(package) = package_from_build_file_module(&load.module_id)
                {
                    let metrics = self.package_load_metrics.entry(package).or_default();
                    if metrics.num_targets.is_none() {
                        metrics.num_targets = load.target_count;
                    }
                    if metrics.computation_steps.is_none() {
                        metrics.computation_steps = load.starlark_tick_count;
                    }
                }
            }
            Some(buck2_data::span_end_event::Data::ActionExecution(action)) => {
                self.record_action_data(event, span_end, action);
                let output_size = action
                    .outputs
                    .iter()
                    .fold(0u64, |total, output| total.saturating_add(output.size));
                self.output_artifacts_seen_size =
                    self.output_artifacts_seen_size.saturating_add(output_size);
                self.output_artifacts_seen_count = self
                    .output_artifacts_seen_count
                    .saturating_add(action.outputs.len() as u64);
                if action_outputs_from_local_action_cache(action) {
                    self.output_artifacts_from_action_cache_size = self
                        .output_artifacts_from_action_cache_size
                        .saturating_add(output_size);
                    self.output_artifacts_from_action_cache_count = self
                        .output_artifacts_from_action_cache_count
                        .saturating_add(action.outputs.len() as u64);
                }
                if !self.skip_successful_action_events || action.failed {
                    if let Some(action_event) = action_event(event, span_end, action) {
                        self.remember_action(&action_event);
                        self.remember_target_outputs(action);
                        events.push(action_event);
                    }
                } else if let Some(key) = action_target_key(action) {
                    // Bazel without --build_event_publish_all_actions posts no ActionExecuted
                    // for a successful action, but the action's outputs and its target's
                    // completion still reach TargetComplete and NamedSetOfFiles.
                    self.completed_targets
                        .entry(key)
                        .or_insert(CompletedTargetState { success: true });
                    self.remember_target_outputs(action);
                }
            }
            Some(buck2_data::span_end_event::Data::TestDiscovery(test_discovery)) => {
                self.remember_test_tags(
                    test_discovery.target_label.as_ref(),
                    &test_discovery.labels,
                );
                self.push_configured_event(
                    configured_event_from_test_label(
                        test_discovery.target_label.as_ref(),
                        TEST_TARGET_KIND,
                        &test_discovery.labels,
                    ),
                    events,
                );
            }
            Some(buck2_data::span_end_event::Data::TestRun(test_end)) => {
                if self.should_defer_target_setup()
                    && let Some(suite) = test_end.suite.as_ref()
                {
                    self.push_configured_event(configured_event_from_test_suite(suite), events);
                }
                let expected_cases = test_end
                    .suite
                    .as_ref()
                    .map_or(1, |suite| suite.test_names.len().max(1));
                match test_run_key(test_end) {
                    Some(key) if self.test_cases.get(&key).map_or(0, Vec::len) < expected_cases => {
                        self.held_test_ends.insert(
                            key,
                            HeldTestEnd {
                                event: event.clone(),
                                span_end: span_end.clone(),
                                test_end: (**test_end).clone(),
                                expected_cases,
                            },
                        );
                    }
                    _ => self.push_test_events(event, span_end, test_end, events),
                }
            }
            Some(buck2_data::span_end_event::Data::Materialization(materialization)) => {
                if let Some(fetch) = fetch_event_from_materialization(materialization) {
                    events.push(fetch);
                }
            }
            Some(buck2_data::span_end_event::Data::CreateOutputSymlinks(symlinks)) => {
                self.push_convenience_symlinks_identified(&symlinks.symlinks, events);
            }
            _ => {}
        }
    }

    fn convert_instant(
        &mut self,
        sequence_hint: i64,
        _event: &buck2_data::BuckEvent,
        instant: &buck2_data::InstantEvent,
        events: &mut Vec<bep::BuildEvent>,
    ) {
        match instant.data.as_ref() {
            Some(buck2_data::instant_event::Data::ConsoleMessage(message)) => {
                events.push(self.progress_event(
                    sequence_hint,
                    None,
                    Some(message.message.clone()),
                ));
            }
            Some(buck2_data::instant_event::Data::ConsoleWarning(warning)) => {
                events.push(self.progress_event(
                    sequence_hint,
                    None,
                    Some(warning.message.clone()),
                ));
            }
            Some(buck2_data::instant_event::Data::StreamingOutput(output)) => {
                events.push(self.progress_event(sequence_hint, Some(output.message.clone()), None));
            }
            Some(buck2_data::instant_event::Data::ActionError(action_error)) => {
                events.push(self.progress_event(
                    sequence_hint,
                    None,
                    Some(action_error_message(action_error)),
                ));
            }
            Some(buck2_data::instant_event::Data::StructuredError(error)) => {
                events.push(self.progress_event(sequence_hint, None, Some(error.payload.clone())));
            }
            Some(buck2_data::instant_event::Data::ReSession(session)) => {
                let mut metadata =
                    BTreeMap::from([("REMOTE_EXECUTION_ENABLED".to_owned(), "1".to_owned())]);
                if !session.session_id.is_empty() {
                    metadata.insert("BUCK2_RE_SESSION_ID".to_owned(), session.session_id.clone());
                }
                events.push(self.build_metadata_event(&metadata));
            }
            Some(buck2_data::instant_event::Data::ReLogStreamAvailable(streams)) => {
                if let Some(event) = self.test_progress_event_from_re_log_stream(streams) {
                    events.push(event);
                }
            }
            Some(buck2_data::instant_event::Data::TargetPatterns(patterns)) => {
                self.remember_target_patterns(patterns);
            }
            Some(buck2_data::instant_event::Data::TargetCfg(cfg)) => {
                let mut metadata = BTreeMap::new();
                if !cfg.target_platforms.is_empty() {
                    metadata.insert("TARGET_PLATFORM".to_owned(), cfg.target_platforms.join(","));
                }
                if !cfg.cli_modifiers.is_empty() {
                    metadata.insert(
                        "BUCK2_CLI_MODIFIERS".to_owned(),
                        cfg.cli_modifiers.join(","),
                    );
                }
                if !metadata.is_empty() {
                    events.push(self.build_metadata_event(&metadata));
                }
            }
            Some(buck2_data::instant_event::Data::BuildGraphInfo(info)) => {
                let mut metadata = BTreeMap::new();
                if info.num_nodes > 0 {
                    metadata.insert(
                        "BUCK2_BUILD_GRAPH_NUM_NODES".to_owned(),
                        info.num_nodes.to_string(),
                    );
                }
                if info.num_edges > 0 {
                    metadata.insert(
                        "BUCK2_BUILD_GRAPH_NUM_EDGES".to_owned(),
                        info.num_edges.to_string(),
                    );
                }
                if let Some(command_name) = info.command_name.as_ref()
                    && !command_name.is_empty()
                {
                    metadata.insert("BUCK2_BUILD_GRAPH_COMMAND".to_owned(), command_name.clone());
                }
                if let Some(backend_name) = info.backend_name.as_ref()
                    && !backend_name.is_empty()
                {
                    metadata.insert("BUCK2_BUILD_GRAPH_BACKEND".to_owned(), backend_name.clone());
                }
                if let Some(isolation_dir) = info.isolation_dir.as_ref()
                    && !isolation_dir.is_empty()
                {
                    metadata.insert("BUCK2_ISOLATION_DIR".to_owned(), isolation_dir.clone());
                }
                if !metadata.is_empty() {
                    events.push(self.build_metadata_event(&metadata));
                }
            }
            Some(buck2_data::instant_event::Data::VersionControlRevision(revision)) => {
                let mut metadata = BTreeMap::new();
                add_version_control_metadata(&mut metadata, revision);
                if !metadata.is_empty() {
                    events.push(self.build_metadata_event(&metadata));
                }
            }
            Some(buck2_data::instant_event::Data::TestDiscovery(discovery)) => {
                if let Some(buck2_data::test_discovery::Data::Tests(suite)) =
                    discovery.data.as_ref()
                {
                    self.remember_test_tags(suite.target_label.as_ref(), &suite.labels);
                    self.push_configured_event(configured_event_from_test_suite(suite), events);
                }
                if let Some(buck2_data::test_discovery::Data::Session(session)) =
                    discovery.data.as_ref()
                    && !session.info.is_empty()
                {
                    events.push(self.progress_event(
                        sequence_hint,
                        Some(session.info.clone()),
                        None,
                    ));
                }
            }
            Some(buck2_data::instant_event::Data::TestResult(result)) => {
                self.remember_test_case(result);
                if let Some(message) = test_result_progress_message(result) {
                    events.push(self.progress_event(sequence_hint, None, Some(message)));
                }
                self.push_held_test_events_once_complete(result, events);
            }
            Some(buck2_data::instant_event::Data::EndOfTestResults(end)) => {
                self.test_executor_exit_code = Some(end.exit_code);
                self.push_all_held_test_events(events);
            }
            Some(buck2_data::instant_event::Data::RunExecRequest(request)) => {
                events.push(exec_request_constructed_event(request));
            }
            Some(buck2_data::instant_event::Data::ExternalResourceFetch(fetch)) => {
                if let Some(fetch) = fetch_event_from_external_resource(fetch) {
                    events.push(fetch);
                }
            }
            _ => {}
        }
    }

    fn record_action_data(
        &mut self,
        event: &buck2_data::BuckEvent,
        span_end: &buck2_data::SpanEndEvent,
        action: &buck2_data::ActionExecutionEnd,
    ) {
        if !action_counts_as_executed(action) {
            return;
        }
        let mnemonic = action_mnemonic(action);
        let end_ms = timestamp_millis(event.timestamp.as_ref());
        let start_ms = start_time_from_span_end(span_end, event.timestamp.as_ref())
            .as_ref()
            .map(|timestamp| timestamp_millis(Some(timestamp)))
            .unwrap_or(end_ms);
        self.action_data
            .entry(mnemonic)
            .or_default()
            .record(start_ms, end_ms);
    }

    fn convert_record(
        &mut self,
        event: &buck2_data::BuckEvent,
        record: &buck2_data::RecordEvent,
        events: &mut Vec<bep::BuildEvent>,
    ) {
        match record.data.as_ref() {
            Some(buck2_data::record_event::Data::InvocationRecord(record)) => {
                self.remember_invocation_target_patterns(record);
                self.emit_invocation_command_context(event, record, events);
                self.emit_pending_pattern_expanded(&[], events, true);
                self.emit_completed_updates_for_actions(events);
                events.push(self.build_metadata_event(&build_metadata_from_invocation(record)));
                self.push_convenience_symlinks_identified(&[], events);
                self.push_all_held_test_events(events);
                self.push_finished(finished_event_from_invocation_record(event, record), events);
                self.push_final_progress(events);
                events.push(build_metrics_event(
                    record,
                    self.declared_actions_count,
                    self.declared_artifacts_count,
                    &self.action_data,
                    self.loaded_packages.len() as u64,
                    &self.package_load_metrics,
                    self.output_artifacts_seen_size,
                    self.output_artifacts_seen_count,
                    self.output_artifacts_from_action_cache_size,
                    self.output_artifacts_from_action_cache_count,
                    top_level_artifact_metrics(&self.top_level_targets, &self.target_outputs),
                ));
                self.push_build_tool_logs(
                    build_tool_logs_event_from_invocation(record, &self.command_profile),
                    events,
                );
            }
            Some(buck2_data::record_event::Data::BuildGraphStats(stats)) => {
                if self.should_defer_target_setup() {
                    self.push_build_graph_targets(stats, events);
                    self.emit_pending_pattern_expanded(&stats.build_targets, events, false);
                } else {
                    self.emit_pending_pattern_expanded(&stats.build_targets, events, false);
                    self.push_build_graph_targets(stats, events);
                }
            }
            _ => {}
        }
    }

    fn push_build_graph_targets(
        &mut self,
        stats: &buck2_data::BuildGraphStats,
        events: &mut Vec<bep::BuildEvent>,
    ) {
        for target in &stats.build_targets {
            if let Some(key) = target_key_from_build_target(target) {
                self.top_level_targets.insert(key);
            }
            self.push_configured_event(configured_event_from_build_target(target), events);
            if let Some(completed) = completed_event_from_build_target(target) {
                self.remember_completed(&completed);
            }
        }
    }

    fn push_finished(&mut self, event: bep::BuildEvent, events: &mut Vec<bep::BuildEvent>) {
        let Some(signature) = finished_event_signature(&event) else {
            events.push(event);
            return;
        };
        if self.saw_finished && self.last_finished_signature.as_ref() == Some(&signature) {
            return;
        }
        self.last_finished_signature = Some(signature);
        self.saw_finished = true;
        events.push(event);
    }

    fn push_build_tool_logs(&mut self, event: bep::BuildEvent, events: &mut Vec<bep::BuildEvent>) {
        let signature = event.encode_to_vec();
        if self.emitted_build_tool_logs
            && self.last_build_tool_logs_signature.as_ref() == Some(&signature)
        {
            return;
        }
        self.last_build_tool_logs_signature = Some(signature);
        self.emitted_build_tool_logs = true;
        events.push(event);
    }

    fn push_convenience_symlinks_identified(
        &mut self,
        symlinks: &[buck2_data::OutputSymlink],
        events: &mut Vec<bep::BuildEvent>,
    ) {
        if self.emitted_convenience_symlinks {
            return;
        }
        self.emitted_convenience_symlinks = true;
        events.push(convenience_symlinks_identified_event(symlinks));
    }

    fn record_event_graph(&mut self, events: &[bep::BuildEvent]) {
        for event in events {
            for child in &event.children {
                self.announced_event_ids
                    .entry(event_id_key(child))
                    .or_insert_with(|| child.clone());
            }
        }
        for event in events {
            if let Some(id) = event.id.as_ref() {
                self.posted_event_ids.insert(event_id_key(id));
            }
        }
    }

    fn emit_aborted_events_for_missing_ids(&mut self, events: &mut Vec<bep::BuildEvent>) {
        let reason = abort_reason_from_finished_events(events);
        let missing = self
            .announced_event_ids
            .iter()
            .filter(|(key, _)| !self.posted_event_ids.contains(*key))
            .map(|(key, id)| (key.clone(), id.clone()))
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return;
        }

        let aborted = missing
            .into_iter()
            .map(|(key, id)| {
                self.posted_event_ids.insert(key);
                aborted_event(id, reason)
            })
            .collect::<Vec<_>>();
        let insert_at = events
            .iter()
            .position(|event| event.last_message)
            .unwrap_or(events.len());
        events.splice(insert_at..insert_at, aborted);
    }

    fn emit_invocation_command_context(
        &mut self,
        event: &buck2_data::BuckEvent,
        record: &buck2_data::InvocationRecord,
        events: &mut Vec<bep::BuildEvent>,
    ) {
        if record.cli_args.is_empty() {
            return;
        }

        let command_name = invocation_command_name(record);
        let metadata = invocation_metadata(record);
        self.defer_target_setup_until_first_result = is_test_or_coverage_command(&command_name);
        self.push_unposted_event(
            unstructured_command_line_event_from_args(&record.cli_args),
            events,
        );
        self.push_unposted_event(
            original_structured_command_line_event_from_args(&record.cli_args, &command_name),
            events,
        );
        self.push_unposted_event(
            canonical_structured_command_line_event_from_args(
                &record.cli_args,
                &command_name,
                client_env_options_from_metadata(metadata),
            ),
            events,
        );
        self.push_unposted_event(tool_structured_command_line_event(), events);
        self.push_unposted_event(
            options_parsed_event_from_args(&record.cli_args, &command_name),
            events,
        );
        self.push_unposted_event(default_configuration_event(), events);
        self.push_unposted_event(workspace_info_event_from_metadata(metadata), events);
        self.push_or_defer_workspace_status(
            workspace_status_event_from_invocation(record, event.timestamp.as_ref()),
            events,
        );
    }

    fn push_unposted_event(&mut self, event: bep::BuildEvent, events: &mut Vec<bep::BuildEvent>) {
        let Some(id) = event.id.as_ref() else {
            events.push(event);
            return;
        };
        let key = event_id_key(id);
        if self.posted_event_ids.contains(&key)
            || events
                .iter()
                .filter_map(|event| event.id.as_ref())
                .any(|id| event_id_key(id) == key)
        {
            return;
        }
        events.push(event);
    }

    fn build_metadata_event(&self, metadata: &BTreeMap<String, String>) -> bep::BuildEvent {
        build_metadata_event_with_defaults(&self.build_metadata, metadata)
    }

    fn remember_target_patterns(&mut self, patterns: &buck2_data::ParsedTargetPatterns) {
        let patterns = target_patterns_from_invocation_record(patterns);
        self.remember_target_pattern_values(patterns);
    }

    fn remember_cli_target_patterns(&mut self, command: &buck2_data::CommandStart) {
        let patterns = target_patterns_from_cli(command);
        self.remember_target_pattern_values(patterns);
    }

    fn remember_target_pattern_values(&mut self, patterns: Vec<String>) {
        if patterns.is_empty() {
            return;
        }
        if self.pending_patterns.as_ref() == Some(&patterns) {
            return;
        }
        if let Some(existing_patterns) = self.pending_patterns.as_ref()
            && self.pattern_id_announced_or_posted(existing_patterns)
        {
            return;
        }
        self.pending_patterns = Some(patterns);
        self.pattern_expanded_emitted = false;
    }

    fn pattern_id_announced_or_posted(&self, patterns: &[String]) -> bool {
        let id = pattern_expanded_id(patterns.to_vec());
        let key = event_id_key(&id);
        self.announced_event_ids.contains_key(&key) || self.posted_event_ids.contains(&key)
    }

    fn remember_invocation_target_patterns(&mut self, record: &buck2_data::InvocationRecord) {
        self.remember_target_pattern_values(target_patterns_from_invocation(record));
    }

    fn emit_pending_pattern_expanded(
        &mut self,
        targets: &[buck2_data::BuildTarget],
        events: &mut Vec<bep::BuildEvent>,
        force: bool,
    ) {
        if self.pattern_expanded_emitted {
            self.flush_pending_target_setup(events);
            return;
        }
        let Some(patterns) = self.pending_patterns.clone() else {
            self.flush_pending_target_setup(events);
            return;
        };
        let exact_children = configured_target_children_matching_exact_patterns(
            &patterns,
            &self.known_configured_target_labels(targets),
        );
        let mut children = if !exact_children.is_empty() {
            exact_children
        } else if let Some(root_cell) = self.build_metadata.get("ROOT_CELL") {
            configured_target_children_from_exact_patterns_for_root_cell(&patterns, root_cell)
        } else {
            Vec::new()
        };
        if children.is_empty() {
            children = if targets.is_empty() {
                configured_target_children_from_events(&self.pending_configured_events)
            } else {
                configured_target_children_from_build_targets(targets)
            };
        }
        if children.is_empty() && self.should_defer_target_setup() {
            children = configured_target_children_from_labels(&self.configured_target_labels);
        }
        if children.is_empty()
            && self.should_defer_target_setup()
            && (force
                || !self.pending_configured_events.is_empty()
                || !self.configured_target_labels.is_empty())
        {
            children = configured_target_children_from_exact_patterns(&patterns);
        }
        if !force && self.should_defer_target_setup() && children.is_empty() {
            return;
        }
        events.push(pattern_expanded_event(patterns, children));
        self.pattern_expanded_emitted = true;
        self.flush_pending_target_setup(events);
    }

    fn push_or_defer_workspace_status(
        &mut self,
        event: bep::BuildEvent,
        events: &mut Vec<bep::BuildEvent>,
    ) {
        if self.should_defer_workspace_status() {
            self.pending_workspace_status = Some(event);
        } else {
            self.push_unposted_event(event, events);
        }
    }

    fn flush_pending_target_setup(&mut self, events: &mut Vec<bep::BuildEvent>) {
        events.extend(std::mem::take(&mut self.pending_configured_events));
        if let Some(event) = self.pending_workspace_status.take() {
            events.push(event);
        }
    }

    fn known_configured_target_labels(
        &self,
        targets: &[buck2_data::BuildTarget],
    ) -> BTreeSet<String> {
        let mut labels = self.configured_target_labels.clone();
        labels.extend(configured_target_labels_from_events(
            &self.pending_configured_events,
        ));
        labels.extend(configured_target_labels_from_build_targets(targets));
        labels
    }

    fn emit_pending_target_setup_before_target_events(
        &mut self,
        events: &mut Vec<bep::BuildEvent>,
    ) {
        if !self.defer_target_setup_until_first_result {
            return;
        }
        if self.pending_configured_events.is_empty() {
            return;
        }
        if !events.iter().any(is_target_lifecycle_event) {
            return;
        }
        let mut setup = Vec::new();
        self.emit_pending_pattern_expanded(&[], &mut setup, true);
        events.splice(0..0, setup);
    }

    fn push_configured_event(
        &mut self,
        event: Option<bep::BuildEvent>,
        events: &mut Vec<bep::BuildEvent>,
    ) {
        let Some(event) = event else {
            return;
        };
        let Some(build_event_id::Id::TargetConfigured(id)) =
            event.id.as_ref().and_then(|id| id.id.as_ref())
        else {
            events.push(event);
            return;
        };
        let signature = event.encode_to_vec();
        if self
            .emitted_configured_targets
            .get(&id.label)
            .is_some_and(|emitted| *emitted == signature)
        {
            return;
        }
        self.configured_target_labels.insert(id.label.clone());
        self.emitted_configured_targets
            .insert(id.label.clone(), signature);
        if self.should_defer_target_setup() {
            self.pending_configured_events.push(event);
            return;
        }
        self.announce_late_test_target(&event, events);
        events.push(event);
    }

    /// BuildBuddy's target tracker follows only the targets a PatternExpanded event names
    /// (target_tracker.go handleExpandedEvent and handleEvent at v2.310.0), and a test command's
    /// PatternExpanded goes out at its first target event, so a test target configured after it,
    /// as under `//...` once actions run, would be left off the test grid. It gets a
    /// PatternExpanded of its own, its label as the pattern, before its TargetConfigured.
    fn announce_late_test_target(
        &self,
        configured: &bep::BuildEvent,
        events: &mut Vec<bep::BuildEvent>,
    ) {
        if !self.defer_target_setup_until_first_result || !self.pattern_expanded_emitted {
            return;
        }
        if !is_test_configured_event(configured) {
            return;
        }
        let Some(id) = configured.id.as_ref() else {
            return;
        };
        let Some(build_event_id::Id::TargetConfigured(configured_id)) = id.id.as_ref() else {
            return;
        };
        let key = event_id_key(id);
        let announced = self.announced_event_ids.contains_key(&key)
            || events
                .iter()
                .flat_map(|event| &event.children)
                .any(|child| event_id_key(child) == key);
        if !announced {
            events.push(pattern_expanded_event(
                vec![configured_id.label.clone()],
                vec![id.clone()],
            ));
        }
    }

    fn should_defer_target_setup(&self) -> bool {
        self.defer_target_setup_until_first_result
            && self.pending_patterns.is_some()
            && !self.pattern_expanded_emitted
    }

    fn should_defer_workspace_status(&self) -> bool {
        self.defer_target_setup_until_first_result && !self.pattern_expanded_emitted
    }

    fn started_event(
        &self,
        event: &buck2_data::BuckEvent,
        command: Option<&buck2_data::CommandStart>,
    ) -> bep::BuildEvent {
        let cli_args = command.map(|c| c.cli_args.as_slice()).unwrap_or(&[]);
        let command_name = command
            .map(command_name)
            .unwrap_or_else(|| "unknown".to_owned());
        let patterns = command.map(target_patterns_from_cli).unwrap_or_default();
        self.started_event_from_parts(
            event,
            cli_args,
            &command_name,
            command.map(|c| &c.metadata),
            None,
            patterns,
        )
    }

    fn started_event_from_invocation(
        &self,
        event: &buck2_data::BuckEvent,
        record: &buck2_data::InvocationRecord,
    ) -> bep::BuildEvent {
        let command_name = invocation_command_name(record);
        self.started_event_from_parts(
            event,
            &record.cli_args,
            &command_name,
            invocation_metadata(record),
            record.repo_path.as_deref().filter(|path| !path.is_empty()),
            target_patterns_from_invocation(record),
        )
    }

    fn started_event_from_parts(
        &self,
        event: &buck2_data::BuckEvent,
        cli_args: &[String],
        command_name: &str,
        metadata: Option<&HashMap<String, String>>,
        workspace_directory_fallback: Option<&str>,
        patterns: Vec<String>,
    ) -> bep::BuildEvent {
        let options_description = if cli_args.is_empty() {
            BUILD_TOOL_VERSION.to_owned()
        } else {
            cli_args.join(" ")
        };
        let mut children = vec![
            unstructured_command_line_id(),
            structured_command_line_id(ORIGINAL_COMMAND_LINE_LABEL),
            structured_command_line_id(CANONICAL_COMMAND_LINE_LABEL),
            structured_command_line_id(TOOL_COMMAND_LINE_LABEL),
            progress_id(0),
            options_parsed_id(),
            workspace_status_id(),
            build_metadata_id(),
            configuration_event_id(DEFAULT_CONFIGURATION_ID),
            workspace_config_id(),
            convenience_symlinks_identified_id(),
            build_finished_id(),
        ];
        if !patterns.is_empty() {
            children.push(pattern_expanded_id(patterns));
        }

        bep::BuildEvent {
            id: Some(started_id()),
            children,
            payload: Some(build_event::Payload::Started(bep::BuildStarted {
                uuid: event.trace_id.clone(),
                start_time_millis: timestamp_millis(event.timestamp.as_ref()),
                start_time: event.timestamp.clone(),
                build_tool_version: BUILD_TOOL_VERSION.to_owned(),
                options_description,
                command: command_name.to_owned(),
                working_directory: metadata
                    .and_then(|m| first_metadata(m, &["CWD", "PWD", "BUILD_WORKING_DIRECTORY"]))
                    .unwrap_or_default(),
                workspace_directory: metadata
                    .and_then(|m| first_metadata(m, &["REPO_ROOT", "WORKSPACE_DIRECTORY"]))
                    .or_else(|| workspace_directory_fallback.map(str::to_owned))
                    .unwrap_or_default(),
                server_pid: i64::from(std::process::id()),
                host: metadata
                    .and_then(|m| first_metadata(m, &["HOST", "BUILD_HOST", "hostname", "host"]))
                    .or_else(|| env::var("HOSTNAME").ok())
                    .unwrap_or_default(),
                user: metadata
                    .and_then(|m| first_metadata(m, &["USER", "BUILD_USER", "username", "user"]))
                    .or_else(|| env::var("USER").ok())
                    .unwrap_or_default(),
            })),
            last_message: false,
        }
    }

    fn progress_event(
        &mut self,
        _sequence_hint: i64,
        stdout: Option<String>,
        stderr: Option<String>,
    ) -> bep::BuildEvent {
        let opaque_count = self.progress_count;
        self.progress_count = self.progress_count.saturating_add(1);
        let children = (opaque_count < i32::MAX)
            .then(|| progress_id(opaque_count.saturating_add(1)))
            .into_iter()
            .collect();
        bep::BuildEvent {
            id: Some(progress_id(opaque_count)),
            children,
            payload: Some(build_event::Payload::Progress(bep::Progress {
                stdout: stdout.unwrap_or_default(),
                stderr: stderr.unwrap_or_default(),
            })),
            last_message: false,
        }
    }

    fn push_final_progress(&mut self, events: &mut Vec<bep::BuildEvent>) {
        if self.final_progress_emitted {
            return;
        }
        self.final_progress_emitted = true;
        events.push(final_progress_event(self.progress_count));
        self.progress_count = self.progress_count.saturating_add(1);
    }

    fn remember_completed(&mut self, event: &bep::BuildEvent) {
        let Some(build_event_id::Id::TargetCompleted(id)) =
            event.id.as_ref().and_then(|id| id.id.as_ref())
        else {
            return;
        };
        let Some(key) = target_key_from_completed_id(id) else {
            return;
        };
        let success = match event.payload.as_ref() {
            Some(build_event::Payload::Completed(completed)) => completed.success,
            _ => true,
        };
        self.completed_targets
            .entry(key)
            .and_modify(|state| state.success &= success)
            .or_insert(CompletedTargetState { success });
    }

    fn remember_action(&mut self, event: &bep::BuildEvent) {
        let Some(build_event_id::Id::ActionCompleted(id)) =
            event.id.as_ref().and_then(|id| id.id.as_ref())
        else {
            return;
        };
        let Some(key) = target_key_from_action_id(id) else {
            return;
        };
        let action_succeeded = match event.payload.as_ref() {
            Some(build_event::Payload::Action(action)) => action.success,
            _ => true,
        };
        let target = self
            .completed_targets
            .entry(key.clone())
            .or_insert(CompletedTargetState {
                success: action_succeeded,
            });
        target.success &= action_succeeded;
        self.action_children.entry(key).or_default().insert(
            action_child_key(id),
            event.id.clone().expect("action event has id"),
        );
    }

    fn emit_completed_updates_for_actions(&mut self, events: &mut Vec<bep::BuildEvent>) {
        let keys = self.completed_targets.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            let children_by_id = self.action_children.get(&key);
            let child_count = children_by_id.map(BTreeMap::len).unwrap_or_default();
            let test_children_by_id = self.test_children.get(&key);
            let test_child_count = test_children_by_id.map(BTreeMap::len).unwrap_or_default();
            let output_count = self
                .target_outputs
                .get(&key)
                .map(BTreeMap::len)
                .unwrap_or_default();
            let is_first_completion = !self.emitted_completed_targets.contains(&key);
            let child_count_unchanged = self
                .emitted_action_child_counts
                .get(&key)
                .is_some_and(|emitted| *emitted == child_count);
            let test_child_count_unchanged = self
                .emitted_test_child_counts
                .get(&key)
                .is_some_and(|emitted| *emitted == test_child_count);
            let output_count_unchanged = self
                .emitted_output_counts
                .get(&key)
                .is_some_and(|emitted| *emitted == output_count);
            if !is_first_completion
                && child_count_unchanged
                && test_child_count_unchanged
                && output_count_unchanged
            {
                continue;
            }
            let Some(state) = self.completed_targets.get(&key) else {
                continue;
            };
            let mut children = children_by_id
                .into_iter()
                .flat_map(|children| children.values().cloned())
                .collect::<Vec<_>>();
            children.extend(
                test_children_by_id
                    .into_iter()
                    .flat_map(|children| children.values().cloned()),
            );
            children.push(target_summary_id(
                key.label.clone(),
                key.configuration.clone(),
            ));
            let (named_set_events, output_group) = self.output_group_events(&key, &mut children);
            let important_output = self.important_outputs_for_target(&key);
            let directory_output = self.directory_outputs_for_target(&key);
            let test_timeout = self.test_timeouts.get(&key).cloned();
            let tags = self.target_tags(&key);
            events.extend(named_set_events);
            events.push(completed_event_with_children_and_outputs(
                key.label.clone(),
                key.configuration.clone(),
                state.success,
                children,
                output_group,
                important_output,
                directory_output,
                tags,
                test_timeout,
            ));
            events.push(target_summary_event(
                &key,
                state.success,
                self.target_test_statuses.get(&key).copied(),
            ));
            self.emitted_action_child_counts
                .insert(key.clone(), child_count);
            self.emitted_test_child_counts
                .insert(key.clone(), test_child_count);
            self.emitted_output_counts.insert(key.clone(), output_count);
            self.emitted_completed_targets.insert(key);
        }
    }

    fn remember_target_outputs(&mut self, action: &buck2_data::ActionExecutionEnd) {
        let Some(key) = action_target_key(action) else {
            return;
        };
        let outputs = self.target_outputs.entry(key.clone()).or_default();
        for output in &action.outputs {
            let Some(file) = bep_file_from_action_output(output) else {
                continue;
            };
            if output.is_directory {
                self.target_directory_outputs
                    .entry(key.clone())
                    .or_default()
                    .entry(file.name.clone())
                    .or_insert_with(|| file.clone());
            }
            outputs.entry(file.name.clone()).or_insert(file);
        }
    }

    fn output_group_events(
        &self,
        key: &TargetKey,
        children: &mut Vec<bep::BuildEventId>,
    ) -> (Vec<bep::BuildEvent>, Vec<bep::OutputGroup>) {
        let Some(files) = self.target_outputs.get(key) else {
            return (Vec::new(), Vec::new());
        };
        if files.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let named_set = named_set_id_for_target(key);
        children.push(bep::BuildEventId {
            id: Some(build_event_id::Id::NamedSet(named_set.clone())),
        });
        let named_set_event = bep::BuildEvent {
            id: Some(bep::BuildEventId {
                id: Some(build_event_id::Id::NamedSet(named_set.clone())),
            }),
            children: Vec::new(),
            payload: Some(build_event::Payload::NamedSetOfFiles(
                bep::NamedSetOfFiles {
                    files: files.values().cloned().collect(),
                    file_sets: Vec::new(),
                },
            )),
            last_message: false,
        };
        (
            vec![named_set_event],
            vec![bep::OutputGroup {
                name: "default".to_owned(),
                file_sets: vec![named_set],
                incomplete: false,
                inline_files: Vec::new(),
            }],
        )
    }

    fn directory_outputs_for_target(&self, key: &TargetKey) -> Vec<bep::File> {
        self.target_directory_outputs
            .get(key)
            .into_iter()
            .flat_map(|outputs| outputs.values().cloned())
            .collect()
    }

    fn important_outputs_for_target(&self, key: &TargetKey) -> Vec<bep::File> {
        self.target_outputs
            .get(key)
            .into_iter()
            .flat_map(|outputs| outputs.values().cloned())
            .collect()
    }

    fn push_test_events(
        &mut self,
        event: &buck2_data::BuckEvent,
        span_end: &buck2_data::SpanEndEvent,
        test_end: &buck2_data::TestRunEnd,
        events: &mut Vec<bep::BuildEvent>,
    ) {
        if let Some(result) = self.test_result_event_from_test_end(event, span_end, test_end) {
            events.push(result);
        }
        if let Some(summary) = self.test_summary_event(event, span_end, test_end) {
            events.push(summary);
        }
    }

    fn push_held_test_events_once_complete(
        &mut self,
        result: &buck2_data::TestResult,
        events: &mut Vec<bep::BuildEvent>,
    ) {
        let Some(key) = result.target_label.as_ref().and_then(target_key_for) else {
            return;
        };
        let complete = self.held_test_ends.get(&key).is_some_and(|held| {
            self.test_cases.get(&key).map_or(0, Vec::len) >= held.expected_cases
        });
        if complete && let Some(held) = self.held_test_ends.remove(&key) {
            self.push_test_events(&held.event, &held.span_end, &held.test_end, events);
        }
    }

    /// Every held run, with whatever cases arrived, before the results end or the command
    /// finishes: a run that never reports a case still gets its TestResult and TestSummary.
    fn push_all_held_test_events(&mut self, events: &mut Vec<bep::BuildEvent>) {
        for (_, held) in std::mem::take(&mut self.held_test_ends) {
            self.push_test_events(&held.event, &held.span_end, &held.test_end, events);
        }
    }

    /// Whether the test run failed as buck2's client judges it: the executor's exit code when
    /// it reported one, which `buck2 test` exits with, and otherwise any failing result.
    fn tests_failed(&self) -> bool {
        match self.test_executor_exit_code {
            Some(exit_code) => exit_code != 0,
            None => self.saw_failing_test_result,
        }
    }

    fn remember_test_case(&mut self, result: &buck2_data::TestResult) {
        if is_failing_test_status(test_status(result.status)) {
            self.saw_failing_test_result = true;
        }
        let Some(target) = result.target_label.as_ref() else {
            return;
        };
        let Some(label) = label_for_configured_target(target) else {
            return;
        };
        let key = TargetKey {
            label,
            configuration: configuration_id_for_target(target),
        };
        self.test_cases.entry(key).or_default().push(TestCaseState {
            name: result.name.clone(),
            status: result.status,
            duration: result.duration.clone(),
            details: result.details.clone(),
            message: result.msg.as_ref().map(|message| message.msg.clone()),
        });
    }

    fn remember_test_children(&mut self, key: &TargetKey) {
        let children = self.test_children.entry(key.clone()).or_default();
        children.insert(
            "test_result".to_owned(),
            test_result_id(key.label.clone(), key.configuration.clone()),
        );
        children.insert(
            "test_summary".to_owned(),
            test_summary_id(key.label.clone(), key.configuration.clone()),
        );
        self.completed_targets
            .entry(key.clone())
            .or_insert(CompletedTargetState { success: true });
    }

    fn remember_test_tags(
        &mut self,
        target: Option<&buck2_data::ConfiguredTargetLabel>,
        tags: &[String],
    ) {
        if tags.is_empty() {
            return;
        }
        let Some(target) = target else {
            return;
        };
        let Some(label) = label_for_configured_target(target) else {
            return;
        };
        let key = TargetKey {
            label,
            configuration: configuration_id_for_target(target),
        };
        let stored_tags = self.target_tags.entry(key).or_default();
        for tag in tags {
            if !stored_tags.contains(tag) {
                stored_tags.push(tag.clone());
            }
        }
    }

    fn target_tags(&self, key: &TargetKey) -> Vec<String> {
        self.target_tags.get(key).cloned().unwrap_or_default()
    }

    fn test_progress_event_from_re_log_stream(
        &mut self,
        streams: &buck2_data::ReLogStreamAvailable,
    ) -> Option<bep::BuildEvent> {
        let key = streams.key.as_ref()?;
        let target = match key.owner.as_ref()? {
            buck2_data::action_key::Owner::TestTargetLabel(target) => target,
            _ => return None,
        };
        let label = label_for_configured_target(target)?;
        let configuration = configuration_id_for_target(target);
        let uri = if !streams.stdout_stream_name.is_empty() {
            &streams.stdout_stream_name
        } else {
            &streams.stderr_stream_name
        };
        if uri.is_empty() {
            return None;
        }
        let target_key = TargetKey {
            label: label.clone(),
            configuration: configuration.clone(),
        };
        let opaque_count = self
            .test_progress_counts
            .entry(target_key)
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);

        Some(test_progress_event(
            label,
            configuration,
            *opaque_count,
            uri.to_owned(),
        ))
    }

    fn test_result_event_from_test_end(
        &mut self,
        event: &buck2_data::BuckEvent,
        span_end: &buck2_data::SpanEndEvent,
        test_end: &buck2_data::TestRunEnd,
    ) -> Option<bep::BuildEvent> {
        let suite = test_end.suite.as_ref()?;
        let target = suite.target_label.as_ref()?;
        let label = label_for_configured_target(target)?;
        let configuration = configuration_id_for_target(target);
        let key = TargetKey {
            label: label.clone(),
            configuration: configuration.clone(),
        };
        let cases = self.test_cases.get(&key).cloned().unwrap_or_default();
        let status = aggregate_test_status(test_end.command_report.as_ref(), &cases);
        let end_time = event.timestamp.clone();
        let start_time = start_time_from_span_end(span_end, end_time.as_ref());
        let duration =
            test_duration(test_end.command_report.as_ref()).or_else(|| span_end.duration.clone());
        let outputs = test_action_outputs(&cases, test_end.command_report.as_ref(), status);
        self.remember_test_children(&key);
        self.remember_test_tags(Some(target), &suite.labels);
        if let Some(timeout) = test_end.timeout.as_ref() {
            self.test_timeouts.insert(key.clone(), timeout.clone());
        }
        self.target_test_statuses.insert(key.clone(), status);

        Some(bep::BuildEvent {
            id: Some(test_result_id(label, configuration)),
            children: Vec::new(),
            payload: Some(build_event::Payload::TestResult(bep::TestResult {
                status,
                cached_locally: false,
                test_attempt_start_millis_epoch: timestamp_millis(start_time.as_ref()),
                test_attempt_duration_millis: optional_duration_millis(duration.as_ref())
                    .unwrap_or_default(),
                test_action_output: outputs,
                warning: Vec::new(),
                execution_info: test_execution_info(test_end.command_report.as_ref()),
                status_details: test_status_details(&cases, test_end.command_report.as_ref()),
                test_attempt_start: start_time,
                test_attempt_duration: duration,
            })),
            last_message: false,
        })
    }

    fn test_summary_event(
        &mut self,
        event: &buck2_data::BuckEvent,
        span_end: &buck2_data::SpanEndEvent,
        test_end: &buck2_data::TestRunEnd,
    ) -> Option<bep::BuildEvent> {
        let suite = test_end.suite.as_ref()?;
        let target = suite.target_label.as_ref()?;
        let label = label_for_configured_target(target)?;
        let configuration = configuration_id_for_target(target);
        let key = TargetKey {
            label: label.clone(),
            configuration: configuration.clone(),
        };
        let cases = self.test_cases.remove(&key).unwrap_or_default();
        let status = aggregate_test_status(test_end.command_report.as_ref(), &cases);
        let end_time = event.timestamp.clone();
        let start_time = start_time_from_span_end(span_end, end_time.as_ref());
        let duration =
            test_duration(test_end.command_report.as_ref()).or_else(|| span_end.duration.clone());
        Some(bep::BuildEvent {
            id: Some(test_summary_id(label, configuration)),
            children: Vec::new(),
            payload: Some(build_event::Payload::TestSummary(bep::TestSummary {
                overall_status: status,
                total_run_count: 1,
                run_count: 1,
                attempt_count: 1,
                shard_count: 1,
                passed: test_summary_files(status, &cases, test_end.command_report.as_ref(), true),
                failed: test_summary_files(status, &cases, test_end.command_report.as_ref(), false),
                total_num_cached: i32::from(test_cached_remotely(test_end.command_report.as_ref())),
                first_start_time_millis: timestamp_millis(start_time.as_ref()),
                last_stop_time_millis: timestamp_millis(end_time.as_ref()),
                total_run_duration_millis: optional_duration_millis(duration.as_ref())
                    .unwrap_or_default(),
                first_start_time: start_time,
                last_stop_time: end_time,
                total_run_duration: duration,
            })),
            last_message: false,
        })
    }
}

impl CommandProfileBuilder {
    fn record_span_start(
        &mut self,
        event: &buck2_data::BuckEvent,
        span_start: &buck2_data::SpanStartEvent,
    ) {
        let Some(data) = span_start.data.as_ref() else {
            return;
        };
        if let buck2_data::span_start_event::Data::Command(command) = data {
            self.command_name = Some(command_name(command));
            self.command_started_at_us = timestamp_micros(event.timestamp.as_ref());
        }
        let Some(start_us) = timestamp_micros(event.timestamp.as_ref()) else {
            return;
        };
        let Some(details) = profile_span_start_details(data) else {
            return;
        };
        self.open_spans.insert(
            event.span_id,
            ProfileOpenSpan {
                span_id: event.span_id,
                parent_id: event.parent_id,
                name: details.name,
                lane: details.lane,
                category: details.category,
                start_us,
                args: details.args,
            },
        );
    }

    fn record_span_end(
        &mut self,
        event: &buck2_data::BuckEvent,
        span_end: &buck2_data::SpanEndEvent,
        tests_failed: bool,
    ) {
        let Some(data) = span_end.data.as_ref() else {
            return;
        };
        let duration_us = span_end
            .duration
            .as_ref()
            .map(duration_micros)
            .unwrap_or_default()
            .max(1);
        if let Some(mut open) = self.open_spans.remove(&event.span_id) {
            extend_json_map(&mut open.args, profile_span_end_args(data, tests_failed));
            self.push_completed(ProfileCompletedSpan {
                span_id: open.span_id,
                parent_id: open.parent_id,
                name: open.name,
                lane: open.lane,
                category: open.category,
                start_us: open.start_us,
                duration_us,
                args: open.args,
            });
            return;
        }

        let Some(end_us) = timestamp_micros(event.timestamp.as_ref()) else {
            return;
        };
        let Some(details) = profile_span_end_details(data) else {
            return;
        };
        let mut args = details.args;
        extend_json_map(&mut args, profile_span_end_args(data, tests_failed));
        self.push_completed(ProfileCompletedSpan {
            span_id: event.span_id,
            parent_id: event.parent_id,
            name: details.name,
            lane: details.lane,
            category: details.category,
            start_us: end_us.saturating_sub(duration_us),
            duration_us,
            args,
        });
    }

    fn push_completed(&mut self, span: ProfileCompletedSpan) {
        if self.completed_spans.len() >= MAX_PROFILE_SPANS {
            self.dropped_spans = self.dropped_spans.saturating_add(1);
            return;
        }
        self.completed_spans.push(span);
    }

    fn command_end_profile_gzip(
        &self,
        event: &buck2_data::BuckEvent,
        span_end: &buck2_data::SpanEndEvent,
        command: &buck2_data::CommandEnd,
        outcome: CommandOutcome,
    ) -> Option<Vec<u8>> {
        let command_duration_us = span_end
            .duration
            .as_ref()
            .map(duration_micros)
            .unwrap_or(1)
            .max(1);
        let end_us = timestamp_micros(event.timestamp.as_ref()).unwrap_or(command_duration_us);
        let command_name = self
            .command_name
            .clone()
            .unwrap_or_else(|| command_end_name(command).to_owned());
        let mut args = serde_json::Map::new();
        args.insert(
            "outcome".to_owned(),
            serde_json::json!(outcome.outcome_name()),
        );
        args.insert("exit_code".to_owned(), serde_json::json!(outcome.exit_code));
        args.insert(
            "trace_id".to_owned(),
            serde_json::json!(event.trace_id.as_str()),
        );
        let fallback = ProfileFallbackSpan {
            name: format!("buck2 {command_name}"),
            start_us: end_us.saturating_sub(command_duration_us),
            duration_us: command_duration_us,
            args,
        };
        gzip_profile_json(self.profile_json(Some(fallback)).ok()?)
    }

    fn invocation_profile_gzip(&self, record: &buck2_data::InvocationRecord) -> Option<Vec<u8>> {
        if self.completed_spans.is_empty() {
            return None;
        }
        let command_duration_us = record
            .command_duration
            .as_ref()
            .or(record.client_walltime.as_ref())
            .map(duration_micros)
            .unwrap_or_default()
            .max(1);
        let fallback_start = self
            .command_started_at_us
            .or_else(|| self.completed_spans.iter().map(|span| span.start_us).min())
            .unwrap_or_default();
        let command = record
            .command_name
            .as_deref()
            .or(self.command_name.as_deref())
            .map(str::to_owned)
            .unwrap_or_else(|| invocation_command_name(record));
        let mut args = serde_json::Map::new();
        args.insert(
            "outcome".to_owned(),
            serde_json::json!(invocation_outcome_name(record.outcome)),
        );
        if let Some(exit_code) = record.exit_code {
            args.insert("exit_code".to_owned(), serde_json::json!(exit_code));
        }
        let fallback = ProfileFallbackSpan {
            name: format!("buck2 {command}"),
            start_us: fallback_start,
            duration_us: command_duration_us,
            args,
        };
        gzip_profile_json(self.profile_json(Some(fallback)).ok()?)
    }

    fn profile_json(&self, fallback: Option<ProfileFallbackSpan>) -> serde_json::Result<String> {
        let mut spans = self.completed_spans.clone();
        let has_command_span = spans.iter().any(|span| span.lane == ProfileLane::Command);
        if !has_command_span && let Some(fallback) = fallback {
            spans.push(ProfileCompletedSpan {
                span_id: 0,
                parent_id: 0,
                name: fallback.name,
                lane: ProfileLane::Command,
                category: "command",
                start_us: fallback.start_us,
                duration_us: fallback.duration_us,
                args: fallback.args,
            });
        }
        let base_us = self
            .command_started_at_us
            .or_else(|| spans.iter().map(|span| span.start_us).min())
            .unwrap_or_default();

        spans.sort_by(|left, right| {
            left.start_us
                .cmp(&right.start_us)
                .then_with(|| right.duration_us.cmp(&left.duration_us))
                .then_with(|| left.span_id.cmp(&right.span_id))
        });

        let mut trace_events = Vec::new();
        let mut lane_tracks: BTreeMap<ProfileLane, Vec<i64>> = BTreeMap::new();
        let mut span_tracks: BTreeMap<u64, (u64, ProfileLane)> = BTreeMap::new();

        for span in spans {
            let start_us = span.start_us.saturating_sub(base_us).max(0);
            let duration_us = span.duration_us.max(1);
            let end_us = start_us.saturating_add(duration_us);
            let parent_track = (span.parent_id != 0)
                .then(|| span_tracks.get(&span.parent_id).copied())
                .flatten();
            let tid = if let Some((parent_tid, parent_lane)) = parent_track
                && parent_lane != ProfileLane::Command
                && (parent_lane == span.lane
                    || (parent_lane == ProfileLane::Actions
                        && span.lane == ProfileLane::ActionStages))
            {
                parent_tid
            } else {
                let tracks = lane_tracks.entry(span.lane).or_default();
                let track_index = tracks
                    .iter()
                    .position(|last_end_us| *last_end_us <= start_us)
                    .unwrap_or_else(|| {
                        tracks.push(0);
                        tracks.len() - 1
                    });
                tracks[track_index] = end_us;
                span.lane.tid(track_index)
            };
            span_tracks.insert(span.span_id, (tid, span.lane));
            trace_events.push(serde_json::json!({
                "name": span.name,
                "cat": span.category,
                "ph": "X",
                "ts": start_us,
                "dur": duration_us,
                "pid": 1,
                "tid": tid,
                "args": span.args,
            }));
        }

        if self.dropped_spans > 0 {
            trace_events.push(serde_json::json!({
                "name": "profile spans dropped",
                "cat": "profile",
                "ph": "i",
                "s": "g",
                "ts": 0,
                "pid": 1,
                "args": {
                    "dropped_spans": self.dropped_spans,
                    "max_profile_spans": MAX_PROFILE_SPANS,
                },
            }));
        }

        let mut metadata_events = vec![serde_json::json!({
            "name": "process_name",
            "ph": "M",
            "pid": 1,
            "args": { "name": "buck2" },
        })];
        for (lane, tracks) in &lane_tracks {
            for index in 0..tracks.len() {
                let tid = lane.tid(index);
                let name = if tracks.len() == 1 {
                    lane.label().to_owned()
                } else {
                    format!("{} {}", lane.label(), index + 1)
                };
                metadata_events.push(serde_json::json!({
                    "name": "thread_name",
                    "ph": "M",
                    "pid": 1,
                    "tid": tid,
                    "args": { "name": name },
                }));
            }
        }
        metadata_events.extend(trace_events);
        serialize_trace_events(&metadata_events)
    }
}

pub(crate) fn encode_bep_event(event: &bep::BuildEvent) -> Any {
    Any {
        type_url: BEP_EVENT_TYPE_URL.to_owned(),
        value: event.encode_to_vec(),
    }
}

fn command_start(event: &buck2_data::BuckEvent) -> Option<&buck2_data::CommandStart> {
    match event.data.as_ref() {
        Some(buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
            data: Some(buck2_data::span_start_event::Data::Command(command)),
        })) => Some(command),
        _ => None,
    }
}

fn should_buffer_before_started(event: &buck2_data::BuckEvent) -> bool {
    matches!(
        event.data.as_ref(),
        Some(buck2_data::buck_event::Data::Instant(
            buck2_data::InstantEvent {
                data: Some(
                    buck2_data::instant_event::Data::SystemInfo(_)
                        | buck2_data::instant_event::Data::Snapshot(_)
                        | buck2_data::instant_event::Data::RestartConfiguration(_)
                        | buck2_data::instant_event::Data::TagEvent(_)
                        | buck2_data::instant_event::Data::IoProviderInfo(_)
                        | buck2_data::instant_event::Data::StructuredError(_)
                        | buck2_data::instant_event::Data::VersionControlRevision(_),
                ),
            }
        ))
    )
}

fn invocation_record(event: &buck2_data::BuckEvent) -> Option<&buck2_data::InvocationRecord> {
    match event.data.as_ref() {
        Some(buck2_data::buck_event::Data::Record(buck2_data::RecordEvent {
            data: Some(buck2_data::record_event::Data::InvocationRecord(record)),
        })) => Some(record),
        _ => None,
    }
}

fn started_id() -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::Started(
            build_event_id::BuildStartedId {},
        )),
    }
}

fn structured_command_line_id(label: &str) -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::StructuredCommandLine(
            build_event_id::StructuredCommandLineId {
                command_line_label: label.to_owned(),
            },
        )),
    }
}

fn unstructured_command_line_id() -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::UnstructuredCommandLine(
            build_event_id::UnstructuredCommandLineId {},
        )),
    }
}

fn progress_id(opaque_count: i32) -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::Progress(build_event_id::ProgressId {
            opaque_count,
        })),
    }
}

fn final_progress_event(opaque_count: i32) -> bep::BuildEvent {
    bep::BuildEvent {
        id: Some(progress_id(opaque_count)),
        children: Vec::new(),
        payload: Some(build_event::Payload::Progress(bep::Progress {
            stdout: String::new(),
            stderr: String::new(),
        })),
        last_message: false,
    }
}

fn options_parsed_id() -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::OptionsParsed(
            build_event_id::OptionsParsedId {},
        )),
    }
}

fn workspace_status_id() -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::WorkspaceStatus(
            build_event_id::WorkspaceStatusId {},
        )),
    }
}

fn workspace_config_id() -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::Workspace(
            build_event_id::WorkspaceConfigId {},
        )),
    }
}

fn build_metadata_id() -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::BuildMetadata(
            build_event_id::BuildMetadataId {},
        )),
    }
}

fn build_finished_id() -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::BuildFinished(
            build_event_id::BuildFinishedId {},
        )),
    }
}

fn build_tool_logs_id() -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::BuildToolLogs(
            build_event_id::BuildToolLogsId {},
        )),
    }
}

fn exec_request_id() -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::ExecRequest(
            build_event_id::ExecRequestId {},
        )),
    }
}

fn event_id_key(id: &bep::BuildEventId) -> Vec<u8> {
    id.encode_to_vec()
}

fn configuration_id(id: impl Into<String>) -> build_event_id::ConfigurationId {
    build_event_id::ConfigurationId { id: id.into() }
}

fn configuration_event_id(id: impl Into<String>) -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::Configuration(configuration_id(id))),
    }
}

fn configuration_id_from_bep(configuration: Option<&build_event_id::ConfigurationId>) -> String {
    configuration
        .map(|configuration| configuration.id.clone())
        .filter(|configuration| !configuration.is_empty())
        .unwrap_or_else(|| DEFAULT_CONFIGURATION_ID.to_owned())
}

fn target_key_from_completed_id(id: &build_event_id::TargetCompletedId) -> Option<TargetKey> {
    if id.label.is_empty() {
        return None;
    }
    Some(TargetKey {
        label: id.label.clone(),
        configuration: configuration_id_from_bep(id.configuration.as_ref()),
    })
}

fn target_key_from_action_id(id: &build_event_id::ActionCompletedId) -> Option<TargetKey> {
    if id.label.is_empty() {
        return None;
    }
    Some(TargetKey {
        label: id.label.clone(),
        configuration: configuration_id_from_bep(id.configuration.as_ref()),
    })
}

fn target_key_from_build_target(target: &buck2_data::BuildTarget) -> Option<TargetKey> {
    Some(TargetKey {
        label: normalize_buck_label(&target.target)?,
        configuration: configuration_id_for_build_target(target),
    })
}

fn action_child_key(id: &build_event_id::ActionCompletedId) -> String {
    format!(
        "{}\0{}",
        id.primary_output,
        configuration_id_from_bep(id.configuration.as_ref())
    )
}

fn named_set_id_for_target(key: &TargetKey) -> build_event_id::NamedSetOfFilesId {
    build_event_id::NamedSetOfFilesId {
        id: format!("{}|{}|default", key.label, key.configuration),
    }
}

fn target_configured_id(label: String) -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::TargetConfigured(
            build_event_id::TargetConfiguredId {
                label,
                aspect: String::new(),
            },
        )),
    }
}

fn target_completed_id(label: String, configuration: String) -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::TargetCompleted(
            build_event_id::TargetCompletedId {
                label,
                aspect: String::new(),
                configuration: Some(configuration_id(configuration)),
            },
        )),
    }
}

fn target_summary_id(label: String, configuration: String) -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::TargetSummary(
            build_event_id::TargetSummaryId {
                label,
                configuration: Some(configuration_id(configuration)),
            },
        )),
    }
}

fn test_result_id(label: String, configuration: String) -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::TestResult(
            build_event_id::TestResultId {
                label,
                run: 1,
                shard: 1,
                attempt: 1,
                configuration: Some(configuration_id(configuration)),
            },
        )),
    }
}

fn test_result_progress_message(result: &buck2_data::TestResult) -> Option<String> {
    let status = buck2_data::TestStatus::try_from(result.status).ok()?;
    let details = result.details.trim_end_matches(['\r', '\n']);
    if matches!(
        status,
        buck2_data::TestStatus::Pass | buck2_data::TestStatus::ListingSuccess
    ) && details.is_empty()
    {
        return None;
    }

    let mut message = format!(
        "{}: {}",
        test_result_status_prefix(status),
        test_result_display_name(result)
    );
    if let Some(duration) = result.duration.as_ref() {
        message.push_str(&format!(
            " ({})",
            format_bes_progress_duration_us(duration_micros(duration))
        ));
    }
    if !details.is_empty() {
        message.push('\n');
        message.push_str(details);
    }
    message.push('\n');
    Some(message)
}

fn test_result_status_prefix(status: buck2_data::TestStatus) -> &'static str {
    match status {
        buck2_data::TestStatus::Fail => "✗ Fail",
        buck2_data::TestStatus::Skip => "↷ Skip",
        buck2_data::TestStatus::Omitted => "\u{20E0} Omitted",
        buck2_data::TestStatus::Fatal => "⚠ Fatal",
        buck2_data::TestStatus::Timeout => "✉ Timeout",
        buck2_data::TestStatus::InfraFailure => "🛠 Infra Failure",
        buck2_data::TestStatus::Pass => "✓ Pass",
        buck2_data::TestStatus::ListingSuccess => "✓ Listing success",
        buck2_data::TestStatus::Unknown => "? Unknown",
        buck2_data::TestStatus::Rerun => "↻ Rerun",
        buck2_data::TestStatus::ListingFailed => "⚠ Listing failed",
        buck2_data::TestStatus::NotSetTestStatus => "? Unknown",
    }
}

fn test_result_display_name(result: &buck2_data::TestResult) -> String {
    if !result.name.is_empty() {
        return result.name.clone();
    }
    result
        .target_label
        .as_ref()
        .and_then(label_for_configured_target)
        .unwrap_or_else(|| "unknown test".to_owned())
}

fn test_progress_id(label: String, configuration: String, opaque_count: i32) -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::TestProgress(
            build_event_id::TestProgressId {
                label,
                configuration: Some(configuration_id(configuration)),
                run: 1,
                shard: 1,
                attempt: 1,
                opaque_count,
            },
        )),
    }
}

fn test_progress_event(
    label: String,
    configuration: String,
    opaque_count: i32,
    uri: String,
) -> bep::BuildEvent {
    bep::BuildEvent {
        id: Some(test_progress_id(label, configuration, opaque_count)),
        children: Vec::new(),
        payload: Some(build_event::Payload::TestProgress(bep::TestProgress {
            uri,
        })),
        last_message: false,
    }
}

fn test_summary_id(label: String, configuration: String) -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::TestSummary(
            build_event_id::TestSummaryId {
                label,
                configuration: Some(configuration_id(configuration)),
            },
        )),
    }
}

fn target_summary_event(
    key: &TargetKey,
    overall_build_success: bool,
    overall_test_status: Option<i32>,
) -> bep::BuildEvent {
    bep::BuildEvent {
        id: Some(target_summary_id(
            key.label.clone(),
            key.configuration.clone(),
        )),
        children: Vec::new(),
        payload: Some(build_event::Payload::TargetSummary(bep::TargetSummary {
            overall_build_success,
            overall_test_status: overall_test_status.unwrap_or(bep::TestStatus::NoStatus as i32),
        })),
        last_message: false,
    }
}

fn command_name(command: &buck2_data::CommandStart) -> String {
    use buck2_data::command_start::Data;

    match command.data.as_ref() {
        Some(Data::Build(_)) => Some("build"),
        Some(Data::Targets(_)) => Some("targets"),
        Some(Data::Query(_)) => Some("query"),
        Some(Data::Cquery(_)) => Some("cquery"),
        Some(Data::Test(_)) => Some("test"),
        Some(Data::Audit(_)) => Some("audit"),
        Some(Data::Docs(_)) => Some("docs"),
        Some(Data::Clean(_)) => Some("clean"),
        Some(Data::Aquery(_)) => Some("aquery"),
        Some(Data::Install(_)) => Some("install"),
        Some(Data::Materialize(_)) => Some("materialize"),
        Some(Data::Profile(_)) => Some("profile"),
        Some(Data::Bxl(_)) => Some("bxl"),
        Some(Data::Lsp(_)) => Some("lsp"),
        Some(Data::FileStatus(_)) => Some("file-status"),
        Some(Data::Starlark(_)) => Some("starlark"),
        Some(Data::Subscribe(_)) => Some("subscribe"),
        Some(Data::Trace(_)) => Some("trace"),
        Some(Data::Ctargets(_)) => Some("ctargets"),
        Some(Data::StarlarkDebugAttach(_)) => Some("starlark-debug-attach"),
        Some(Data::Explain(_)) => Some("explain"),
        Some(Data::ExpandExternalCell(_)) => Some("expand-external-cell"),
        Some(Data::Complete(_)) => Some("complete"),
        Some(Data::Hydration(_)) => Some("hydration"),
        None => command_name_from_args(&command.cli_args),
    }
    .unwrap_or("unknown")
    .to_owned()
}

fn command_end_name(command: &buck2_data::CommandEnd) -> &'static str {
    command_end_name_opt(command).unwrap_or("unknown")
}

fn is_test_or_coverage_command(command_name: &str) -> bool {
    matches!(command_name, "test" | "coverage")
}

fn command_end_name_opt(command: &buck2_data::CommandEnd) -> Option<&'static str> {
    use buck2_data::command_end::Data;

    match command.data.as_ref() {
        Some(Data::Build(_)) => Some("build"),
        Some(Data::Targets(_)) => Some("targets"),
        Some(Data::Query(_)) => Some("query"),
        Some(Data::Cquery(_)) => Some("cquery"),
        Some(Data::Test(_)) => Some("test"),
        Some(Data::Audit(_)) => Some("audit"),
        Some(Data::Docs(_)) => Some("docs"),
        Some(Data::Clean(_)) => Some("clean"),
        Some(Data::Aquery(_)) => Some("aquery"),
        Some(Data::Install(_)) => Some("install"),
        Some(Data::Materialize(_)) => Some("materialize"),
        Some(Data::Profile(_)) => Some("profile"),
        Some(Data::Bxl(_)) => Some("bxl"),
        Some(Data::Lsp(_)) => Some("lsp"),
        Some(Data::FileStatus(_)) => Some("file-status"),
        Some(Data::Starlark(_)) => Some("starlark"),
        Some(Data::Subscribe(_)) => Some("subscribe"),
        Some(Data::Trace(_)) => Some("trace"),
        Some(Data::Ctargets(_)) => Some("ctargets"),
        Some(Data::StarlarkDebugAttach(_)) => Some("starlark-debug-attach"),
        Some(Data::Explain(_)) => Some("explain"),
        Some(Data::ExpandExternalCell(_)) => Some("expand-external-cell"),
        Some(Data::Complete(_)) => Some("complete"),
        Some(Data::Hydration(_)) => Some("hydration"),
        None => None,
    }
}

fn invocation_command_name(record: &buck2_data::InvocationRecord) -> String {
    if let Some(command_name) = record.command_name.as_ref()
        && !command_name.is_empty()
    {
        return command_name.clone();
    }
    record
        .command_end
        .as_ref()
        .and_then(command_end_name_opt)
        .or_else(|| command_name_from_args(&record.cli_args))
        .unwrap_or("unknown")
        .to_owned()
}

fn command_name_from_args(args: &[String]) -> Option<&'static str> {
    let mut i = if args
        .first()
        .is_some_and(|arg| known_command_name(arg).is_some())
    {
        0
    } else {
        1
    };
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            break;
        }
        if let Some(command_name) = known_command_name(arg) {
            return Some(command_name);
        }
        if let Some(option) = option_from_arg(arg) {
            i += if option.option_value.is_empty()
                && i + 1 < args.len()
                && option_takes_value(&option.option_name)
            {
                2
            } else {
                1
            };
        } else {
            i += 1;
        }
    }
    None
}

fn known_command_name(arg: &str) -> Option<&'static str> {
    match arg {
        "build" => Some("build"),
        "targets" => Some("targets"),
        "query" => Some("query"),
        "cquery" => Some("cquery"),
        "test" => Some("test"),
        "audit" => Some("audit"),
        "docs" => Some("docs"),
        "clean" => Some("clean"),
        "aquery" => Some("aquery"),
        "install" => Some("install"),
        "materialize" => Some("materialize"),
        "profile" => Some("profile"),
        "bxl" => Some("bxl"),
        "lsp" => Some("lsp"),
        "file-status" => Some("file-status"),
        "starlark" => Some("starlark"),
        "subscribe" => Some("subscribe"),
        "trace" => Some("trace"),
        "ctargets" => Some("ctargets"),
        "starlark-debug-attach" => Some("starlark-debug-attach"),
        "explain" => Some("explain"),
        "expand-external-cell" => Some("expand-external-cell"),
        "complete" => Some("complete"),
        "hydration" => Some("hydration"),
        _ => None,
    }
}

fn invocation_metadata(record: &buck2_data::InvocationRecord) -> Option<&HashMap<String, String>> {
    record.metadata.as_ref().map(|metadata| &metadata.strings)
}

fn first_metadata_opt(metadata: Option<&HashMap<String, String>>, keys: &[&str]) -> Option<String> {
    metadata.and_then(|metadata| first_metadata(metadata, keys))
}

fn unstructured_command_line_event(command: &buck2_data::CommandStart) -> bep::BuildEvent {
    unstructured_command_line_event_from_args(&command.cli_args)
}

fn unstructured_command_line_event_from_args(args: &[String]) -> bep::BuildEvent {
    bep::BuildEvent {
        id: Some(unstructured_command_line_id()),
        children: Vec::new(),
        payload: Some(build_event::Payload::UnstructuredCommandLine(
            bep::UnstructuredCommandLine {
                args: args.to_vec(),
            },
        )),
        last_message: false,
    }
}

fn original_structured_command_line_event(command: &buck2_data::CommandStart) -> bep::BuildEvent {
    original_structured_command_line_event_from_args(&command.cli_args, &command_name(command))
}

fn original_structured_command_line_event_from_args(
    args: &[String],
    command_name: &str,
) -> bep::BuildEvent {
    let mut sections = Vec::new();
    if let Some(executable) = args.first() {
        sections.push(chunk_section(
            "executable",
            vec![display_executable(executable)],
        ));
    }
    sections.push(chunk_section("command", vec![command_name.to_owned()]));

    let parsed = ParsedCliArgs::from_args(args, command_name);
    if !parsed.options.is_empty() {
        sections.push(option_section("command options", parsed.options));
    }
    if !parsed.residue.is_empty() {
        sections.push(chunk_section("residual", parsed.residue));
    }

    bep::BuildEvent {
        id: Some(structured_command_line_id(ORIGINAL_COMMAND_LINE_LABEL)),
        children: Vec::new(),
        payload: Some(build_event::Payload::StructuredCommandLine(
            cl::CommandLine {
                command_line_label: ORIGINAL_COMMAND_LINE_LABEL.to_owned(),
                sections,
            },
        )),
        last_message: false,
    }
}

fn canonical_structured_command_line_event(command: &buck2_data::CommandStart) -> bep::BuildEvent {
    canonical_structured_command_line_event_from_args(
        &command.cli_args,
        &command_name(command),
        client_env_options(command),
    )
}

fn canonical_structured_command_line_event_from_args(
    args: &[String],
    command_name: &str,
    client_env_options: Vec<cl::Option>,
) -> bep::BuildEvent {
    let mut sections = Vec::new();
    if let Some(executable) = args.first() {
        sections.push(chunk_section("executable", vec![executable.clone()]));
    }
    sections.push(chunk_section("command", vec![command_name.to_owned()]));

    let parsed = ParsedCliArgs::from_args(args, command_name);
    let mut options = parsed.options;
    options.extend(client_env_options);
    if !options.is_empty() {
        sections.push(option_section("command options", options));
    }
    if !parsed.residue.is_empty() {
        sections.push(chunk_section("residue", parsed.residue));
    }

    bep::BuildEvent {
        id: Some(structured_command_line_id(CANONICAL_COMMAND_LINE_LABEL)),
        children: Vec::new(),
        payload: Some(build_event::Payload::StructuredCommandLine(
            cl::CommandLine {
                command_line_label: CANONICAL_COMMAND_LINE_LABEL.to_owned(),
                sections,
            },
        )),
        last_message: false,
    }
}

fn tool_structured_command_line_event() -> bep::BuildEvent {
    bep::BuildEvent {
        id: Some(structured_command_line_id(TOOL_COMMAND_LINE_LABEL)),
        children: Vec::new(),
        payload: Some(build_event::Payload::StructuredCommandLine(
            cl::CommandLine {
                command_line_label: String::new(),
                sections: Vec::new(),
            },
        )),
        last_message: false,
    }
}

fn chunk_section(label: &str, chunks: Vec<String>) -> cl::CommandLineSection {
    cl::CommandLineSection {
        section_label: label.to_owned(),
        section_type: Some(cl::command_line_section::SectionType::ChunkList(
            cl::ChunkList { chunk: chunks },
        )),
    }
}

fn option_section(label: &str, options: Vec<cl::Option>) -> cl::CommandLineSection {
    cl::CommandLineSection {
        section_label: label.to_owned(),
        section_type: Some(cl::command_line_section::SectionType::OptionList(
            cl::OptionList { option: options },
        )),
    }
}

struct ParsedCliArgs {
    options: Vec<cl::Option>,
    residue: Vec<String>,
}

impl ParsedCliArgs {
    fn from_args(args: &[String], command_name: &str) -> Self {
        let mut options = Vec::new();
        let mut residue = Vec::new();
        let mut i = 1;
        let mut skipped_command = false;
        let mut force_residue = false;
        while i < args.len() {
            let arg = &args[i];
            if force_residue {
                residue.push(arg.clone());
                i += 1;
                continue;
            }
            if arg == "--" {
                force_residue = true;
                i += 1;
                continue;
            }
            if !skipped_command && arg.as_str() == command_name {
                skipped_command = true;
                i += 1;
                continue;
            }
            if let Some(mut option) = option_from_arg(arg) {
                if option.option_value.is_empty()
                    && i + 1 < args.len()
                    && should_consume_option_value(
                        &option.option_name,
                        &args[i + 1],
                        command_name,
                        skipped_command,
                    )
                {
                    option.option_value = args[i + 1].clone();
                    option.combined_form = format!("{}={}", arg, option.option_value);
                    i += 1;
                }
                if option.option_value.is_empty() {
                    option.option_value = "1".to_owned();
                }
                options.push(option);
            } else {
                residue.push(arg.clone());
            }
            i += 1;
        }

        Self { options, residue }
    }
}

fn display_executable(executable: &str) -> String {
    executable
        .rsplit(|c| c == '/' || c == '\\')
        .find(|component| !component.is_empty())
        .unwrap_or(executable)
        .to_owned()
}

fn should_consume_option_value(
    option_name: &str,
    next_arg: &str,
    command_name: &str,
    skipped_command: bool,
) -> bool {
    if next_arg == "--" || next_arg.starts_with('-') || is_boolean_option(option_name) {
        return false;
    }
    if option_takes_value(option_name) {
        return true;
    }
    if !skipped_command && next_arg == command_name {
        return false;
    }
    !looks_like_residue_arg(next_arg)
}

fn option_takes_value(option_name: &str) -> bool {
    matches!(
        option_name,
        "build_report"
            | "build_report_options"
            | "config"
            | "config_file"
            | "fake_architecture"
            | "fake_host"
            | "isolation_dir"
            | "num_threads"
            | "streaming_build_report"
            | "target_platforms"
            | "tool_tag"
            | "unstable_target_platforms"
    )
}

fn is_boolean_option(option_name: &str) -> bool {
    matches!(
        option_name,
        "local_only"
            | "prefer_local"
            | "prefer_remote"
            | "remote_only"
            | "unstable_no_execution"
            | "unstable_no_remote_cache"
    )
}

fn looks_like_residue_arg(arg: &str) -> bool {
    arg.starts_with("//") || arg.starts_with(':') || arg.starts_with('@')
}

fn option_from_arg(arg: &str) -> Option<cl::Option> {
    let raw = arg.strip_prefix("--")?;
    if raw.is_empty() {
        return None;
    }
    let had_equals = raw.contains('=');
    let (raw_name, value) = raw
        .split_once('=')
        .map(|(name, value)| (name, value.to_owned()))
        .unwrap_or_else(|| (raw, String::new()));
    let (name, value) = raw_name
        .strip_prefix("no")
        .filter(|name| !name.is_empty())
        .map(|name| (name, "0".to_owned()))
        .unwrap_or((raw_name, value));

    Some(cl::Option {
        combined_form: arg.to_owned(),
        option_name: normalize_option_name(name),
        option_value: if had_equals || value == "0" {
            value
        } else {
            String::new()
        },
        effect_tags: Vec::new(),
        metadata_tags: Vec::new(),
        source: "command line options".to_owned(),
    })
}

fn normalize_option_name(name: &str) -> String {
    name.trim_start_matches('-').replace('-', "_")
}

fn client_env_options(command: &buck2_data::CommandStart) -> Vec<cl::Option> {
    client_env_options_from_metadata(Some(&command.metadata))
}

fn client_env_options_from_metadata(metadata: Option<&HashMap<String, String>>) -> Vec<cl::Option> {
    let mut values = BTreeMap::new();
    insert_env_option(
        &mut values,
        "USER",
        first_metadata_opt(metadata, &["USER", "BUILD_USER", "username", "user"])
            .or_else(|| env::var("USER").ok()),
    );
    insert_env_option(
        &mut values,
        "HOST",
        first_metadata_opt(metadata, &["HOST", "BUILD_HOST", "hostname", "host"])
            .or_else(|| env::var("HOSTNAME").ok()),
    );
    for key in [
        "CI",
        "GITHUB_ACTOR",
        "GITHUB_REPOSITORY",
        "GITHUB_REF",
        "GITHUB_HEAD_REF",
        "GITHUB_SHA",
        "BUILDKITE_BUILD_CREATOR",
        "BUILDKITE_REPO",
        "BUILDKITE_BRANCH",
        "BUILDKITE_COMMIT",
        "CIRCLE_USERNAME",
        "CIRCLE_REPOSITORY_URL",
        "CIRCLE_BRANCH",
        "CIRCLE_SHA1",
        "GIT_REPOSITORY_URL",
        "GIT_URL",
        "GIT_BRANCH",
        "GIT_COMMIT",
        "REPO_URL",
        "COMMIT_SHA",
    ] {
        insert_env_option(
            &mut values,
            key,
            first_metadata_opt(metadata, &[key]).or_else(|| env::var(key).ok()),
        );
    }

    values
        .into_iter()
        .map(|(key, value)| cl::Option {
            combined_form: format!("--client_env={key}={value}"),
            option_name: "client_env".to_owned(),
            option_value: format!("{key}={value}"),
            effect_tags: Vec::new(),
            metadata_tags: Vec::new(),
            source: "Buck2 metadata".to_owned(),
        })
        .collect()
}

fn insert_env_option(values: &mut BTreeMap<String, String>, key: &str, value: Option<String>) {
    if let Some(value) = value
        && !value.is_empty()
    {
        values.insert(key.to_owned(), value);
    }
}

fn options_parsed_event(command: &buck2_data::CommandStart) -> bep::BuildEvent {
    options_parsed_event_from_args(&command.cli_args, &command_name(command))
}

fn options_parsed_event_from_args(args: &[String], command_name: &str) -> bep::BuildEvent {
    let parsed = ParsedCliArgs::from_args(args, command_name);
    let tool_tag = tool_tag_from_options(&parsed.options);
    let cmd_line = parsed
        .options
        .iter()
        .map(|option| option.combined_form.clone())
        .collect::<Vec<_>>();

    bep::BuildEvent {
        id: Some(options_parsed_id()),
        children: Vec::new(),
        payload: Some(build_event::Payload::OptionsParsed(bep::OptionsParsed {
            startup_options: Vec::new(),
            explicit_startup_options: Vec::new(),
            cmd_line: cmd_line.clone(),
            explicit_cmd_line: cmd_line,
            invocation_policy: None,
            tool_tag,
        })),
        last_message: false,
    }
}

fn tool_tag_from_options(options: &[cl::Option]) -> String {
    options
        .iter()
        .rev()
        .find(|option| option.option_name == "tool_tag")
        .map(|option| option.option_value.clone())
        .unwrap_or_default()
}

fn workspace_status_event(
    command: &buck2_data::CommandStart,
    timestamp: Option<&Timestamp>,
) -> bep::BuildEvent {
    workspace_status_event_from_metadata(
        Some(&command.metadata),
        target_patterns_from_cli(command),
        timestamp_millis_u64(timestamp),
    )
}

fn workspace_status_event_from_invocation(
    record: &buck2_data::InvocationRecord,
    timestamp: Option<&Timestamp>,
) -> bep::BuildEvent {
    workspace_status_event_from_metadata(
        invocation_metadata(record),
        target_patterns_from_invocation(record),
        invocation_start_time_millis(record, timestamp),
    )
}

fn workspace_status_event_from_metadata(
    metadata: Option<&HashMap<String, String>>,
    patterns: Vec<String>,
    timestamp_millis: Option<u64>,
) -> bep::BuildEvent {
    let mut items = Vec::new();
    add_workspace_item_value(
        &mut items,
        "BUILD_EMBED_LABEL",
        first_metadata_opt(metadata, &["BUILD_EMBED_LABEL", "EMBED_LABEL"]).unwrap_or_default(),
    );
    add_workspace_item(
        &mut items,
        "BUILD_USER",
        first_metadata_opt(metadata, &["USER", "BUILD_USER", "username", "user"])
            .or_else(|| env::var("USER").ok()),
    );
    add_workspace_item(
        &mut items,
        "BUILD_HOST",
        first_metadata_opt(metadata, &["HOST", "BUILD_HOST", "hostname", "host"])
            .or_else(|| env::var("HOSTNAME").ok()),
    );
    let timestamp_millis = workspace_status_timestamp_millis(metadata, timestamp_millis);
    if let Some(timestamp_millis) = timestamp_millis {
        add_workspace_item(
            &mut items,
            "BUILD_TIMESTAMP",
            Some((timestamp_millis / 1000).to_string()),
        );
        add_workspace_item(
            &mut items,
            "FORMATTED_DATE",
            formatted_workspace_status_date(timestamp_millis),
        );
    }
    add_workspace_item(
        &mut items,
        "BUILD_WORKING_DIRECTORY",
        first_metadata_opt(metadata, &["CWD", "PWD", "BUILD_WORKING_DIRECTORY"]),
    );
    add_workspace_item(&mut items, "ROLE", first_metadata_opt(metadata, &["ROLE"]));
    add_workspace_item(
        &mut items,
        "REPO_URL",
        first_metadata_opt(metadata, &["REPO_URL", "GIT_REPOSITORY_URL"]),
    );
    add_workspace_item(
        &mut items,
        "GIT_BRANCH",
        first_metadata_opt(metadata, &["BRANCH_NAME", "GIT_BRANCH"]),
    );
    add_workspace_item(
        &mut items,
        "COMMIT_SHA",
        first_metadata_opt(metadata, &["COMMIT_SHA", "GIT_COMMIT"]),
    );
    if !patterns.is_empty() {
        add_workspace_item(&mut items, "PATTERN", Some(patterns.join(" ")));
    }

    bep::BuildEvent {
        id: Some(workspace_status_id()),
        children: Vec::new(),
        payload: Some(build_event::Payload::WorkspaceStatus(
            bep::WorkspaceStatus { item: items },
        )),
        last_message: false,
    }
}

fn invocation_start_time_millis(
    record: &buck2_data::InvocationRecord,
    timestamp: Option<&Timestamp>,
) -> Option<u64> {
    record
        .wrapper_start_time
        .or_else(|| timestamp_millis_u64(timestamp))
}

fn workspace_status_timestamp_millis(
    metadata: Option<&HashMap<String, String>>,
    fallback: Option<u64>,
) -> Option<u64> {
    first_metadata_opt(metadata, &["SOURCE_DATE_EPOCH"])
        .and_then(|value| value.trim().parse::<u64>().ok())
        .and_then(|seconds| seconds.checked_mul(1000))
        .or(fallback)
}

fn formatted_workspace_status_date(timestamp_millis: u64) -> Option<String> {
    let seconds = i64::try_from(timestamp_millis / 1000).ok()?;
    let date = DateTime::<Utc>::from_timestamp(seconds, 0)?;
    Some(date.format("%Y %b %d %H %M %S %a").to_string())
}

fn add_workspace_item(
    items: &mut Vec<bep::workspace_status::Item>,
    key: &str,
    value: Option<String>,
) {
    if let Some(value) = value
        && !value.is_empty()
    {
        items.push(bep::workspace_status::Item {
            key: key.to_owned(),
            value,
        });
    }
}

fn add_workspace_item_value(
    items: &mut Vec<bep::workspace_status::Item>,
    key: &str,
    value: String,
) {
    items.push(bep::workspace_status::Item {
        key: key.to_owned(),
        value,
    });
}

fn add_metadata_alias(metadata: &mut BTreeMap<String, String>, key: &str, value: Option<String>) {
    if let Some(value) = value
        && !value.is_empty()
    {
        metadata.entry(key.to_owned()).or_insert(value);
    }
}

fn default_configuration_event() -> bep::BuildEvent {
    bep::BuildEvent {
        id: Some(configuration_event_id(DEFAULT_CONFIGURATION_ID)),
        children: Vec::new(),
        payload: Some(build_event::Payload::Configuration(bep::Configuration {
            mnemonic: DEFAULT_CONFIGURATION_ID.to_owned(),
            platform_name: String::new(),
            cpu: env::consts::ARCH.to_owned(),
            make_variable: HashMap::new(),
            is_tool: false,
        })),
        last_message: false,
    }
}

fn workspace_info_event(command: &buck2_data::CommandStart) -> bep::BuildEvent {
    workspace_info_event_from_metadata(Some(&command.metadata))
}

fn workspace_info_event_from_metadata(
    metadata: Option<&HashMap<String, String>>,
) -> bep::BuildEvent {
    let local_exec_root = first_metadata_opt(
        metadata,
        &["BUCK_OUT", "BUCK2_EXEC_ROOT", "EXEC_ROOT", "REPO_ROOT"],
    )
    .unwrap_or_default();
    bep::BuildEvent {
        id: Some(workspace_config_id()),
        children: Vec::new(),
        payload: Some(build_event::Payload::WorkspaceInfo(bep::WorkspaceConfig {
            local_exec_root,
        })),
        last_message: false,
    }
}

fn build_metadata_from_command(
    command: &buck2_data::CommandStart,
    defaults: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut metadata = defaults.clone();
    metadata.extend(command.metadata.iter().map(|(k, v)| (k.clone(), v.clone())));
    add_metadata_alias(
        &mut metadata,
        "USER",
        first_metadata(
            &command.metadata,
            &["USER", "BUILD_USER", "username", "user"],
        )
        .or_else(|| env::var("USER").ok()),
    );
    add_metadata_alias(
        &mut metadata,
        "HOST",
        first_metadata(
            &command.metadata,
            &["HOST", "BUILD_HOST", "hostname", "host"],
        )
        .or_else(|| env::var("HOSTNAME").ok()),
    );
    add_metadata_alias(
        &mut metadata,
        "REPO_URL",
        first_metadata(&command.metadata, &["REPO_URL", "GIT_REPOSITORY_URL"]),
    );
    add_metadata_alias(
        &mut metadata,
        "BRANCH_NAME",
        first_metadata(&command.metadata, &["BRANCH_NAME", "GIT_BRANCH"]),
    );
    add_metadata_alias(
        &mut metadata,
        "COMMIT_SHA",
        first_metadata(&command.metadata, &["COMMIT_SHA", "GIT_COMMIT"]),
    );
    let patterns = target_patterns_from_cli(command);
    if !patterns.is_empty() {
        metadata.insert("PATTERN".to_owned(), patterns.join(" "));
    }
    if env::var("CI").is_ok_and(|ci| !ci.is_empty()) {
        metadata.entry("ROLE".to_owned()).or_insert("CI".to_owned());
    }
    metadata
        .entry(BUILDBUDDY_VISIBILITY_KEY.to_owned())
        .or_insert_with(|| BUILDBUDDY_PUBLIC_VISIBILITY.to_owned());
    metadata
}

fn build_metadata_from_invocation(
    record: &buck2_data::InvocationRecord,
) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::new();
    if let Some(record_metadata) = invocation_metadata(record) {
        metadata.extend(
            record_metadata
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    let command_name = invocation_command_name(record);
    if command_name != "unknown" {
        metadata.insert("BUCK2_COMMAND".to_owned(), command_name);
    }
    if !record.re_session_id.is_empty() {
        metadata.insert(
            "BUCK2_RE_SESSION_ID".to_owned(),
            record.re_session_id.clone(),
        );
    }
    if let Some(version_control_revision) = record.version_control_revision.as_ref() {
        add_version_control_metadata(&mut metadata, version_control_revision);
    }
    if let Some(revision) = record.hg_revision.as_ref()
        && !revision.is_empty()
    {
        metadata.insert("COMMIT_SHA".to_owned(), revision.clone());
    }
    if let Some(has_local_changes) = record.has_local_changes {
        metadata.insert(
            "HAS_LOCAL_CHANGES".to_owned(),
            has_local_changes.to_string(),
        );
    }
    let patterns = target_patterns_from_invocation(record);
    if !patterns.is_empty() {
        metadata.insert("PATTERN".to_owned(), patterns.join(" "));
    }
    add_metadata_alias(
        &mut metadata,
        "USER",
        first_metadata_opt(
            invocation_metadata(record),
            &["USER", "BUILD_USER", "username", "user"],
        )
        .or_else(|| env::var("USER").ok()),
    );
    add_metadata_alias(
        &mut metadata,
        "HOST",
        first_metadata_opt(
            invocation_metadata(record),
            &["HOST", "BUILD_HOST", "hostname", "host"],
        )
        .or_else(|| env::var("HOSTNAME").ok()),
    );
    add_metadata_alias(
        &mut metadata,
        "REPO_URL",
        first_metadata_opt(
            invocation_metadata(record),
            &["REPO_URL", "GIT_REPOSITORY_URL"],
        ),
    );
    add_metadata_alias(
        &mut metadata,
        "BRANCH_NAME",
        first_metadata_opt(invocation_metadata(record), &["BRANCH_NAME", "GIT_BRANCH"]),
    );
    add_metadata_alias(
        &mut metadata,
        "COMMIT_SHA",
        first_metadata_opt(invocation_metadata(record), &["COMMIT_SHA", "GIT_COMMIT"]),
    );
    if env::var("CI").is_ok_and(|ci| !ci.is_empty()) {
        metadata.entry("ROLE".to_owned()).or_insert("CI".to_owned());
    }
    metadata
        .entry(BUILDBUDDY_VISIBILITY_KEY.to_owned())
        .or_insert_with(|| BUILDBUDDY_PUBLIC_VISIBILITY.to_owned());
    metadata
}

fn add_version_control_metadata(
    metadata: &mut BTreeMap<String, String>,
    revision: &buck2_data::VersionControlRevision,
) {
    if let Some(revision) = revision.hg_revision.as_ref()
        && !revision.is_empty()
    {
        metadata.insert("COMMIT_SHA".to_owned(), revision.clone());
    }
    if let Some(has_local_changes) = revision.has_local_changes {
        metadata.insert(
            "HAS_LOCAL_CHANGES".to_owned(),
            has_local_changes.to_string(),
        );
    }
}

fn metadata_map_from_hash_map(metadata: &HashMap<String, String>) -> BTreeMap<String, String> {
    metadata
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn build_metadata_event_with_defaults(
    defaults: &BTreeMap<String, String>,
    metadata: &BTreeMap<String, String>,
) -> bep::BuildEvent {
    let mut merged = defaults.clone();
    merged.extend(metadata.iter().map(|(k, v)| (k.clone(), v.clone())));
    build_metadata_event(&merged)
}

fn build_metadata_event(metadata: &BTreeMap<String, String>) -> bep::BuildEvent {
    bep::BuildEvent {
        id: Some(build_metadata_id()),
        children: Vec::new(),
        payload: Some(build_event::Payload::BuildMetadata(bep::BuildMetadata {
            metadata: metadata
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        })),
        last_message: false,
    }
}

fn target_patterns_from_cli(command: &buck2_data::CommandStart) -> Vec<String> {
    target_patterns_from_args(&command.cli_args, &command_name(command))
}

fn target_patterns_from_invocation(record: &buck2_data::InvocationRecord) -> Vec<String> {
    if let Some(patterns) = record.parsed_target_patterns.as_ref() {
        let patterns = target_patterns_from_invocation_record(patterns);
        if !patterns.is_empty() {
            return patterns;
        }
    }
    target_patterns_from_args(&record.cli_args, &invocation_command_name(record))
}

fn target_patterns_from_args(args: &[String], command_name: &str) -> Vec<String> {
    ParsedCliArgs::from_args(args, command_name)
        .residue
        .into_iter()
        .filter(|arg| arg.starts_with("//") || arg.starts_with(':') || arg.starts_with('@'))
        .collect()
}

fn build_metrics_id() -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::BuildMetrics(
            build_event_id::BuildMetricsId {},
        )),
    }
}

fn convenience_symlinks_identified_id() -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::ConvenienceSymlinksIdentified(
            build_event_id::ConvenienceSymlinksIdentifiedId {},
        )),
    }
}

fn convenience_symlinks_identified_event(
    symlinks: &[buck2_data::OutputSymlink],
) -> bep::BuildEvent {
    bep::BuildEvent {
        id: Some(convenience_symlinks_identified_id()),
        children: Vec::new(),
        payload: Some(build_event::Payload::ConvenienceSymlinksIdentified(
            bep::ConvenienceSymlinksIdentified {
                convenience_symlinks: symlinks
                    .iter()
                    .map(|link| bep::ConvenienceSymlink {
                        path: link.path.clone(),
                        action: bep::convenience_symlink::Action::Create as i32,
                        target: link.target.clone(),
                    })
                    .collect(),
            },
        )),
        last_message: false,
    }
}

fn fetch_event_from_materialization(
    materialization: &buck2_data::MaterializationEnd,
) -> Option<bep::BuildEvent> {
    let method = materialization
        .method
        .and_then(|method| buck2_data::MaterializationMethod::try_from(method).ok())?;
    if method != buck2_data::MaterializationMethod::HttpDownload {
        return None;
    }
    let url = materialization.url.as_ref().filter(|url| !url.is_empty())?;
    Some(bep::BuildEvent {
        id: Some(bep::BuildEventId {
            id: Some(build_event_id::Id::Fetch(build_event_id::FetchId {
                url: url.clone(),
                downloader: build_event_id::fetch_id::Downloader::Http as i32,
            })),
        }),
        children: Vec::new(),
        payload: Some(build_event::Payload::Fetch(bep::Fetch {
            success: materialization.success,
        })),
        last_message: false,
    })
}

fn fetch_event_from_external_resource(
    fetch: &buck2_data::ExternalResourceFetch,
) -> Option<bep::BuildEvent> {
    let url = (!fetch.url.is_empty()).then(|| fetch.url.clone())?;
    let downloader = buck2_data::ExternalResourceDownloader::try_from(fetch.downloader).ok()?;
    let downloader = match downloader {
        buck2_data::ExternalResourceDownloader::UnknownDownloader => {
            build_event_id::fetch_id::Downloader::Unknown
        }
        buck2_data::ExternalResourceDownloader::HttpDownloader => {
            build_event_id::fetch_id::Downloader::Http
        }
        buck2_data::ExternalResourceDownloader::GrpcDownloader => {
            build_event_id::fetch_id::Downloader::Grpc
        }
    };
    Some(bep::BuildEvent {
        id: Some(bep::BuildEventId {
            id: Some(build_event_id::Id::Fetch(build_event_id::FetchId {
                url,
                downloader: downloader as i32,
            })),
        }),
        children: Vec::new(),
        payload: Some(build_event::Payload::Fetch(bep::Fetch {
            success: fetch.success,
        })),
        last_message: false,
    })
}

fn exec_request_constructed_event(request: &buck2_data::RunExecRequest) -> bep::BuildEvent {
    bep::BuildEvent {
        id: Some(exec_request_id()),
        children: Vec::new(),
        payload: Some(build_event::Payload::ExecRequest(
            bep::ExecRequestConstructed {
                working_directory: request.working_directory.as_bytes().to_vec(),
                argv: request
                    .argv
                    .iter()
                    .map(|arg| arg.as_bytes().to_vec())
                    .collect(),
                environment_variable: request
                    .env
                    .iter()
                    .map(|entry| bep::EnvironmentVariable {
                        name: entry.key.as_bytes().to_vec(),
                        value: entry.value.as_bytes().to_vec(),
                    })
                    .collect(),
                environment_variable_to_clear: request
                    .env_to_clear
                    .iter()
                    .map(|name| name.as_bytes().to_vec())
                    .collect(),
                should_exec: request.should_exec,
            },
        )),
        last_message: false,
    }
}

fn action_cache_statistics(
    record: &buck2_data::InvocationRecord,
) -> Option<blaze::ActionCacheStatistics> {
    let hits = i32_saturating_from_u64(record.run_action_cache_count);
    let misses = i32_saturating_from_u64(
        record
            .run_local_count
            .saturating_add(record.run_remote_count),
    );
    if hits == 0 && misses == 0 {
        return None;
    }

    let miss_details = (misses > 0)
        .then(|| blaze::action_cache_statistics::MissDetail {
            reason: blaze::action_cache_statistics::MissReason::NotCached as i32,
            count: misses,
        })
        .into_iter()
        .collect();

    Some(blaze::ActionCacheStatistics {
        size_in_bytes: 0,
        save_time_in_ms: 0,
        hits,
        misses,
        miss_details,
        load_time_in_ms: 0,
    })
}

fn i32_saturating_from_u64(count: u64) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

fn i64_saturating_from_u64(count: u64) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

const MAX_ACTION_DATA: usize = 20;

fn action_data_events(
    action_data: &BTreeMap<String, ActionDataState>,
) -> Vec<bep::build_metrics::action_summary::ActionData> {
    let mut entries = action_data.iter().collect::<Vec<_>>();
    entries.sort_by(|(left_mnemonic, left), (right_mnemonic, right)| {
        right
            .actions_executed
            .cmp(&left.actions_executed)
            .then_with(|| left_mnemonic.cmp(right_mnemonic))
    });
    entries
        .into_iter()
        .take(MAX_ACTION_DATA)
        .map(
            |(mnemonic, data)| bep::build_metrics::action_summary::ActionData {
                mnemonic: mnemonic.clone(),
                actions_executed: i64_saturating_from_u64(data.actions_executed),
                first_started_ms: data.first_started_ms.unwrap_or_default(),
                last_ended_ms: data.last_ended_ms.unwrap_or_default(),
                system_time: None,
                user_time: None,
                actions_created: 0,
            },
        )
        .collect()
}

fn package_load_metric_events(
    packages: &BTreeMap<String, PackageLoadMetricState>,
) -> Vec<bep_package_metrics::PackageLoadMetrics> {
    packages
        .iter()
        .map(|(name, metrics)| bep_package_metrics::PackageLoadMetrics {
            name: Some(name.clone()),
            load_duration: metrics.load_duration.clone(),
            num_targets: metrics.num_targets,
            computation_steps: metrics.computation_steps,
            num_transitive_loads: None,
            package_overhead: None,
        })
        .collect()
}

fn package_from_build_file_module(module_id: &str) -> Option<String> {
    let (package, _build_file) = module_id.split_once(':')?;
    (!package.is_empty()).then(|| package.to_owned())
}

fn top_level_artifact_metrics(
    top_level_targets: &BTreeSet<TargetKey>,
    target_outputs: &BTreeMap<TargetKey, BTreeMap<String, bep::File>>,
) -> Option<bep::build_metrics::artifact_metrics::FilesMetric> {
    let mut size_in_bytes = 0u64;
    let mut count = 0u64;
    for target in top_level_targets {
        let Some(outputs) = target_outputs.get(target) else {
            continue;
        };
        count = count.saturating_add(outputs.len() as u64);
        for output in outputs.values() {
            if let Ok(length) = u64::try_from(output.length) {
                size_in_bytes = size_in_bytes.saturating_add(length);
            }
        }
    }
    (count > 0).then(|| bep::build_metrics::artifact_metrics::FilesMetric {
        size_in_bytes: i64_saturating_from_u64(size_in_bytes),
        count: i32_saturating_from_u64(count),
    })
}

fn build_metrics_event(
    record: &buck2_data::InvocationRecord,
    declared_actions_count: u64,
    declared_artifacts_count: u64,
    action_data: &BTreeMap<String, ActionDataState>,
    loaded_packages_count: u64,
    package_load_metrics: &BTreeMap<String, PackageLoadMetricState>,
    output_artifacts_seen_size: u64,
    output_artifacts_seen_count: u64,
    output_artifacts_from_action_cache_size: u64,
    output_artifacts_from_action_cache_count: u64,
    top_level_artifacts: Option<bep::build_metrics::artifact_metrics::FilesMetric>,
) -> bep::BuildEvent {
    let remote_cache_hits = record
        .run_action_cache_count
        .saturating_add(record.run_remote_dep_file_cache_count);
    let actions_executed = record
        .run_local_count
        .saturating_add(record.run_remote_count)
        .saturating_add(remote_cache_hits);
    let actions_created = if declared_actions_count > 0 {
        declared_actions_count
    } else {
        actions_executed
    };
    let runner_count = [
        ("total", actions_executed, ""),
        ("local", record.run_local_count, "local"),
        ("remote", record.run_remote_count, "remote"),
        (
            "remote cache hit",
            record.run_action_cache_count,
            "remote-cache",
        ),
        (
            "remote dep file cache hit",
            record.run_remote_dep_file_cache_count,
            "remote-cache",
        ),
        ("skipped", record.run_skipped_count, "skipped"),
    ]
    .into_iter()
    .filter(|(name, count, _)| *count > 0 || *name == "total")
    .map(
        |(name, count, exec_kind)| bep::build_metrics::action_summary::RunnerCount {
            name: name.to_owned(),
            count: i32::try_from(count).unwrap_or(i32::MAX),
            exec_kind: exec_kind.to_owned(),
        },
    )
    .collect();

    let wall_time_in_ms = record
        .client_walltime
        .as_ref()
        .or(record.command_duration.as_ref())
        .map(duration_millis)
        .unwrap_or_default();
    let execution_phase_time_in_ms = match (
        record.time_to_first_action_execution_ms,
        record.time_to_last_action_execution_end_ms,
    ) {
        (Some(start), Some(end)) if end >= start => i64::try_from(end - start).unwrap_or(i64::MAX),
        _ => 0,
    };
    let packages_loaded = if loaded_packages_count > 0 {
        loaded_packages_count
    } else {
        record.load_count.unwrap_or_default()
    };
    let output_artifacts_seen = if output_artifacts_seen_count > 0 {
        bep::build_metrics::artifact_metrics::FilesMetric {
            size_in_bytes: i64::try_from(output_artifacts_seen_size).unwrap_or(i64::MAX),
            count: i32_saturating_from_u64(output_artifacts_seen_count),
        }
    } else {
        bep::build_metrics::artifact_metrics::FilesMetric {
            size_in_bytes: record
                .materialization_output_size
                .map(|size| i64::try_from(size).unwrap_or(i64::MAX))
                .unwrap_or_default(),
            count: record
                .materialization_files
                .map(i32_saturating_from_u64)
                .unwrap_or_default(),
        }
    };

    bep::BuildEvent {
        id: Some(build_metrics_id()),
        children: Vec::new(),
        payload: Some(build_event::Payload::BuildMetrics(bep::BuildMetrics {
            action_summary: Some(bep::build_metrics::ActionSummary {
                actions_created: i64_saturating_from_u64(actions_created),
                actions_created_not_including_aspects: i64_saturating_from_u64(actions_created),
                actions_executed: i64_saturating_from_u64(actions_executed),
                action_data: action_data_events(action_data),
                remote_cache_hits: i64_saturating_from_u64(remote_cache_hits),
                runner_count,
                action_cache_statistics: action_cache_statistics(record),
            }),
            memory_metrics: record.peak_process_memory_bytes.map(|peak| {
                bep::build_metrics::MemoryMetrics {
                    used_heap_size_post_build: i64::try_from(peak).unwrap_or(i64::MAX),
                    peak_post_gc_heap_size: i64::try_from(peak).unwrap_or(i64::MAX),
                    peak_post_gc_tenured_space_heap_size: 0,
                    garbage_metrics: Vec::new(),
                }
            }),
            target_metrics: record.analysis_count.map(|count| {
                let count = i64::try_from(count).unwrap_or(i64::MAX);
                bep::build_metrics::TargetMetrics {
                    targets_loaded: 0,
                    targets_configured: count,
                    targets_configured_not_including_aspects: count,
                }
            }),
            package_metrics: (packages_loaded > 0).then(|| bep::build_metrics::PackageMetrics {
                packages_loaded: i64::try_from(packages_loaded).unwrap_or(i64::MAX),
                package_load_metrics: package_load_metric_events(package_load_metrics),
            }),
            timing_metrics: Some(bep::build_metrics::TimingMetrics {
                cpu_time_in_ms: cpu_time_millis(record),
                wall_time_in_ms,
                analysis_phase_time_in_ms: record
                    .time_to_first_analysis_ms
                    .and_then(|start| {
                        record
                            .time_to_first_action_execution_ms
                            .and_then(|end| end.checked_sub(start))
                    })
                    .map(|duration| i64::try_from(duration).unwrap_or(i64::MAX))
                    .unwrap_or_default(),
                execution_phase_time_in_ms,
                actions_execution_start_in_ms: record
                    .time_to_first_action_execution_ms
                    .map(|value| i64::try_from(value).unwrap_or(i64::MAX))
                    .unwrap_or_default(),
                critical_path_time: record.critical_path_duration.clone(),
            }),
            cumulative_metrics: cumulative_metrics(record, actions_created, actions_executed),
            artifact_metrics: Some(bep::build_metrics::ArtifactMetrics {
                source_artifacts_read: None,
                output_artifacts_seen: Some(output_artifacts_seen),
                output_artifacts_from_action_cache: (output_artifacts_from_action_cache_count > 0)
                    .then(|| bep::build_metrics::artifact_metrics::FilesMetric {
                        size_in_bytes: i64_saturating_from_u64(
                            output_artifacts_from_action_cache_size,
                        ),
                        count: i32_saturating_from_u64(output_artifacts_from_action_cache_count),
                    }),
                top_level_artifacts,
            }),
            build_graph_metrics: build_graph_metrics(
                declared_actions_count,
                declared_artifacts_count,
                post_invocation_dice_key_count(record),
            ),
            worker_metrics: Vec::new(),
            network_metrics: network_metrics(record),
            worker_pool_metrics: None,
            dynamic_execution_metrics: None,
            remote_analysis_cache_statistics: None,
        })),
        last_message: false,
    }
}

fn cpu_time_millis(record: &buck2_data::InvocationRecord) -> i64 {
    let Some(first) = record.first_snapshot.as_ref() else {
        return 0;
    };
    let Some(last) = record.last_snapshot.as_ref() else {
        return 0;
    };
    let first_us = snapshot_cpu_time_us(first);
    let last_us = snapshot_cpu_time_us(last);
    i64::try_from(last_us.saturating_sub(first_us) / 1_000).unwrap_or(i64::MAX)
}

fn cumulative_metrics(
    record: &buck2_data::InvocationRecord,
    actions_created: u64,
    actions_executed: u64,
) -> Option<bep::build_metrics::CumulativeMetrics> {
    let num_analyses = record.analysis_count?;
    Some(bep::build_metrics::CumulativeMetrics {
        num_analyses: i32_saturating_from_u64(num_analyses),
        num_builds: i32::from(invocation_reached_execution_phase(
            record,
            actions_created,
            actions_executed,
        )),
    })
}

fn invocation_reached_execution_phase(
    record: &buck2_data::InvocationRecord,
    actions_created: u64,
    actions_executed: u64,
) -> bool {
    if actions_created > 0 || actions_executed > 0 || record.run_skipped_count > 0 {
        return true;
    }
    matches!(
        invocation_command_name(record).as_str(),
        "build" | "test" | "install" | "run"
    )
}

fn snapshot_cpu_time_us(snapshot: &buck2_data::Snapshot) -> u64 {
    snapshot
        .buck2_user_cpu_us
        .saturating_add(snapshot.buck2_system_cpu_us)
}

fn build_graph_metrics(
    declared_actions_count: u64,
    declared_artifacts_count: u64,
    post_invocation_node_count: u64,
) -> Option<bep::build_metrics::BuildGraphMetrics> {
    if declared_actions_count == 0
        && declared_artifacts_count == 0
        && post_invocation_node_count == 0
    {
        return None;
    }
    let action_count = i32_saturating_from_u64(declared_actions_count);
    Some(bep::build_metrics::BuildGraphMetrics {
        action_lookup_value_count: 0,
        action_lookup_value_count_not_including_aspects: 0,
        action_count,
        action_count_not_including_aspects: action_count,
        input_file_configured_target_count: 0,
        output_file_configured_target_count: 0,
        other_configured_target_count: 0,
        output_artifact_count: i32_saturating_from_u64(declared_artifacts_count),
        post_invocation_skyframe_node_count: i32_saturating_from_u64(post_invocation_node_count),
        dirtied_values: Vec::new(),
        changed_values: Vec::new(),
        built_values: Vec::new(),
        cleaned_values: Vec::new(),
        evaluated_values: Vec::new(),
        rule_class: Vec::new(),
        aspect: Vec::new(),
    })
}

fn post_invocation_dice_key_count(record: &buck2_data::InvocationRecord) -> u64 {
    record
        .last_snapshot
        .as_ref()
        .map(|snapshot| snapshot.dice_key_count)
        .unwrap_or_default()
}

fn network_metrics(
    record: &buck2_data::InvocationRecord,
) -> Option<bep::build_metrics::NetworkMetrics> {
    let first = record.first_snapshot.as_ref()?;
    let last = record.last_snapshot.as_ref()?;
    let mut bytes_sent = 0u64;
    let mut bytes_recv = 0u64;
    for (interface, first_stats) in &first.network_interface_stats {
        let Some(last_stats) = last.network_interface_stats.get(interface) else {
            continue;
        };
        bytes_sent = bytes_sent.saturating_add(network_counter_delta(
            first_stats.tx_bytes,
            last_stats.tx_bytes,
        ));
        bytes_recv = bytes_recv.saturating_add(network_counter_delta(
            first_stats.rx_bytes,
            last_stats.rx_bytes,
        ));
    }
    if bytes_sent == 0 || bytes_recv == 0 {
        return None;
    }
    Some(bep::build_metrics::NetworkMetrics {
        system_network_stats: Some(bep::build_metrics::network_metrics::SystemNetworkStats {
            bytes_sent,
            bytes_recv,
            packets_sent: 0,
            packets_recv: 0,
            peak_bytes_sent_per_sec: 0,
            peak_bytes_recv_per_sec: 0,
            peak_packets_sent_per_sec: 0,
            peak_packets_recv_per_sec: 0,
        }),
    })
}

fn network_counter_delta(first: u64, last: u64) -> u64 {
    if last < first { last } else { last - first }
}

fn duration_millis(duration: &prost_types::Duration) -> i64 {
    let millis = i128::from(duration.seconds) * 1_000 + i128::from(duration.nanos) / 1_000_000;
    i64::try_from(millis).unwrap_or(if millis.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    })
}

fn duration_seconds_i64(duration: &prost_types::Duration) -> i64 {
    duration.seconds
}

fn optional_duration_millis(duration: Option<&prost_types::Duration>) -> Option<i64> {
    duration.map(duration_millis)
}

fn duration_micros(duration: &prost_types::Duration) -> i64 {
    duration
        .seconds
        .saturating_mul(1_000_000)
        .saturating_add(i64::from(duration.nanos) / 1_000)
}

fn timestamp_micros(timestamp: Option<&Timestamp>) -> Option<i64> {
    let timestamp = timestamp?;
    let micros = i128::from(timestamp.seconds) * 1_000_000 + i128::from(timestamp.nanos) / 1_000;
    Some(i64::try_from(micros).unwrap_or(if micros.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    }))
}

fn timestamp_millis(timestamp: Option<&Timestamp>) -> i64 {
    let Some(timestamp) = timestamp else {
        return 0;
    };
    let millis = i128::from(timestamp.seconds) * 1_000 + i128::from(timestamp.nanos) / 1_000_000;
    i64::try_from(millis).unwrap_or(if millis.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    })
}

fn timestamp_millis_u64(timestamp: Option<&Timestamp>) -> Option<u64> {
    let timestamp = timestamp?;
    let millis = i128::from(timestamp.seconds) * 1_000 + i128::from(timestamp.nanos) / 1_000_000;
    u64::try_from(millis).ok()
}

fn target_patterns_from_invocation_record(
    patterns: &buck2_data::ParsedTargetPatterns,
) -> Vec<String> {
    patterns
        .target_patterns
        .iter()
        .map(|pattern| pattern.value.clone())
        .filter(|pattern| !pattern.is_empty())
        .collect()
}

fn invocation_outcome_name(outcome: Option<i32>) -> &'static str {
    match outcome.and_then(|outcome| buck2_data::InvocationOutcome::try_from(outcome).ok()) {
        Some(buck2_data::InvocationOutcome::Success) => "success",
        Some(buck2_data::InvocationOutcome::Failed) => "failed",
        Some(buck2_data::InvocationOutcome::Cancelled) => "cancelled",
        Some(buck2_data::InvocationOutcome::Crashed) => "crashed",
        _ => "unknown",
    }
}

fn profile_span_start_details(
    data: &buck2_data::span_start_event::Data,
) -> Option<ProfileSpanDetails> {
    match data {
        buck2_data::span_start_event::Data::Command(command) => {
            let command_name = command_name(command);
            let mut details = profile_details(
                format!("buck2 {command_name}"),
                ProfileLane::Command,
                "command",
            );
            json_arg_u64(
                &mut details.args,
                "argv_count",
                command.cli_args.len() as u64,
            );
            Some(details)
        }
        buck2_data::span_start_event::Data::CommandCritical(command) => {
            let mut details =
                profile_details("command critical section", ProfileLane::Command, "command");
            json_arg_non_empty(&mut details.args, "dice_version", &command.dice_version);
            Some(details)
        }
        buck2_data::span_start_event::Data::Analysis(analysis) => {
            let target = configured_target_from_analysis_start(analysis)
                .and_then(label_for_configured_target)
                .unwrap_or_else(|| "unknown target".to_owned());
            let mut details = profile_details(
                format!("analysis {target}"),
                ProfileLane::Analysis,
                "analysis",
            );
            json_arg_non_empty(&mut details.args, "rule", &analysis.rule);
            Some(details)
        }
        buck2_data::span_start_event::Data::AnalysisResolveQueries(analysis) => {
            let target = analysis
                .standard_target
                .as_ref()
                .and_then(label_for_configured_target)
                .unwrap_or_else(|| "unknown target".to_owned());
            let details = profile_details(
                format!("analysis queries {target}"),
                ProfileLane::Analysis,
                "analysis",
            );
            Some(details)
        }
        buck2_data::span_start_event::Data::AnalysisStage(_) => Some(profile_details(
            "evaluate rule",
            ProfileLane::Analysis,
            "analysis",
        )),
        buck2_data::span_start_event::Data::Load(load) => {
            let module = if load.module_id.is_empty() {
                load.cell.as_str()
            } else {
                load.module_id.as_str()
            };
            let mut details =
                profile_details(format!("load {module}"), ProfileLane::Loading, "loading");
            json_arg_non_empty(&mut details.args, "module_id", &load.module_id);
            json_arg_non_empty(&mut details.args, "cell", &load.cell);
            Some(details)
        }
        buck2_data::span_start_event::Data::LoadPackage(load) => {
            let mut details = profile_details(
                format!("list package {}", load.path),
                ProfileLane::Loading,
                "loading",
            );
            json_arg_non_empty(&mut details.args, "path", &load.path);
            Some(details)
        }
        buck2_data::span_start_event::Data::ActionExecution(action) => {
            Some(profile_action_start_details(action))
        }
        buck2_data::span_start_event::Data::ExecutorStage(stage) => {
            profile_executor_stage_name(stage.stage.as_ref())
                .map(|name| profile_details(name, ProfileLane::ActionStages, "action_stage"))
        }
        buck2_data::span_start_event::Data::TestDiscovery(test) => {
            let target = test
                .target_label
                .as_ref()
                .and_then(label_for_configured_target)
                .unwrap_or_else(|| "unknown target".to_owned());
            let mut details = profile_details(
                format!("discover tests {target}"),
                ProfileLane::Tests,
                "test",
            );
            json_arg_non_empty(&mut details.args, "suite", &test.suite_name);
            json_arg_non_empty(&mut details.args, "target", &target);
            Some(details)
        }
        buck2_data::span_start_event::Data::TestRun(test) => {
            let suite = test.suite.as_ref();
            let target = suite
                .and_then(|suite| suite.target_label.as_ref())
                .and_then(label_for_configured_target)
                .unwrap_or_else(|| "unknown target".to_owned());
            let mut details = profile_details(format!("test {target}"), ProfileLane::Tests, "test");
            if let Some(suite) = suite {
                json_arg_non_empty(&mut details.args, "suite", &suite.suite_name);
                json_arg_u64(
                    &mut details.args,
                    "test_count",
                    suite.test_names.len() as u64,
                );
            }
            json_arg_non_empty(&mut details.args, "target", &target);
            Some(details)
        }
        buck2_data::span_start_event::Data::FileWatcher(_) => Some(profile_details(
            "file watcher sync",
            ProfileLane::Loading,
            "loading",
        )),
        buck2_data::span_start_event::Data::FinalMaterialization(materialization) => {
            let mut details = profile_details(
                "final materialization",
                ProfileLane::Materialization,
                "materialization",
            );
            if let Some(artifact) = materialization.artifact.as_ref() {
                json_arg_non_empty(&mut details.args, "path", &artifact.path);
            }
            Some(details)
        }
        buck2_data::span_start_event::Data::Materialization(materialization) => {
            let mut details = profile_details(
                "materialization",
                ProfileLane::Materialization,
                "materialization",
            );
            if let Some(digest) = materialization.action_digest.as_ref() {
                json_arg_non_empty(&mut details.args, "action_digest", digest);
            }
            Some(details)
        }
        buck2_data::span_start_event::Data::CacheUpload(upload) => Some(
            profile_cache_upload_details(&upload.key, &upload.name, &upload.action_digest),
        ),
        buck2_data::span_start_event::Data::DepFileUpload(upload) => Some(
            profile_cache_upload_details(&upload.key, &upload.name, &upload.remote_dep_file_key),
        ),
        buck2_data::span_start_event::Data::ReUpload(_) => Some(profile_details(
            "remote upload",
            ProfileLane::Cache,
            "remote",
        )),
        buck2_data::span_start_event::Data::DeferredPreparationStage(_) => Some(profile_details(
            "deferred preparation",
            ProfileLane::Materialization,
            "materialization",
        )),
        buck2_data::span_start_event::Data::CreateOutputSymlinks(_) => Some(profile_details(
            "create output symlinks",
            ProfileLane::Materialization,
            "materialization",
        )),
        buck2_data::span_start_event::Data::LocalResources(_) => Some(profile_details(
            "setup local resources",
            ProfileLane::Other,
            "local_resources",
        )),
        buck2_data::span_start_event::Data::ReleaseLocalResources(_) => Some(profile_details(
            "release local resources",
            ProfileLane::Other,
            "local_resources",
        )),
        buck2_data::span_start_event::Data::DynamicLambda(lambda) => {
            let target = dynamic_lambda_target_label(lambda)
                .and_then(label_for_configured_target)
                .unwrap_or_else(|| "dynamic lambda".to_owned());
            let mut details = profile_details(
                format!("dynamic lambda {target}"),
                ProfileLane::Analysis,
                "analysis",
            );
            json_arg_u64(
                &mut details.args,
                "dynamic_inputs_bytes",
                lambda.dynamic_inputs_bytes,
            );
            Some(details)
        }
        buck2_data::span_start_event::Data::DiceStateUpdate(_)
        | buck2_data::span_start_event::Data::DiceCriticalSection(_)
        | buck2_data::span_start_event::Data::DiceBlockConcurrentCommand(_)
        | buck2_data::span_start_event::Data::DiceSynchronizeSection(_)
        | buck2_data::span_start_event::Data::DiceCleanup(_) => Some(profile_details(
            profile_dice_span_name(data),
            ProfileLane::Dice,
            "dice",
        )),
        buck2_data::span_start_event::Data::ExclusiveCommandWait(wait) => {
            let mut details = profile_details(
                "wait for exclusive command",
                ProfileLane::Command,
                "command",
            );
            if let Some(command_name) = wait.command_name.as_ref() {
                json_arg_non_empty(&mut details.args, "active_command", command_name);
            }
            Some(details)
        }
        buck2_data::span_start_event::Data::BxlExecution(bxl) => {
            let mut details =
                profile_details(format!("bxl {}", bxl.name), ProfileLane::Other, "bxl");
            json_arg_non_empty(&mut details.args, "name", &bxl.name);
            Some(details)
        }
        buck2_data::span_start_event::Data::BxlDiceInvocation(_) => {
            Some(profile_details("bxl dice", ProfileLane::Dice, "dice"))
        }
        buck2_data::span_start_event::Data::BxlEnsureArtifacts(_) => Some(profile_details(
            "bxl ensure artifacts",
            ProfileLane::Materialization,
            "materialization",
        )),
        buck2_data::span_start_event::Data::ActionErrorHandlerExecution(_) => Some(
            profile_details("action error handler", ProfileLane::Actions, "action"),
        ),
        buck2_data::span_start_event::Data::CqueryUniverseBuild(_) => Some(profile_details(
            "cquery universe build",
            ProfileLane::Analysis,
            "analysis",
        )),
        buck2_data::span_start_event::Data::ComputeDetailedAggregatedMetrics(_) => Some(
            profile_details("compute detailed metrics", ProfileLane::Other, "metrics"),
        ),
        buck2_data::span_start_event::Data::InstallEventInfo(install) => {
            let mut details = profile_details(
                format!("install {}", install.artifact_name),
                ProfileLane::Other,
                "install",
            );
            json_arg_non_empty(&mut details.args, "artifact_name", &install.artifact_name);
            json_arg_non_empty(&mut details.args, "file_path", &install.file_path);
            Some(details)
        }
        buck2_data::span_start_event::Data::ConnectToInstaller(_) => Some(profile_details(
            "connect to installer",
            ProfileLane::Other,
            "install",
        )),
        buck2_data::span_start_event::Data::Fake(_) => None,
        _ => None,
    }
}

fn profile_span_end_details(data: &buck2_data::span_end_event::Data) -> Option<ProfileSpanDetails> {
    match data {
        buck2_data::span_end_event::Data::Command(command) => Some(profile_details(
            format!("buck2 {}", command_end_name(command)),
            ProfileLane::Command,
            "command",
        )),
        buck2_data::span_end_event::Data::Analysis(analysis) => {
            let target = configured_target_from_analysis_end(analysis)
                .and_then(label_for_configured_target)
                .unwrap_or_else(|| "unknown target".to_owned());
            let mut details = profile_details(
                format!("analysis {target}"),
                ProfileLane::Analysis,
                "analysis",
            );
            json_arg_non_empty(&mut details.args, "rule", &analysis.rule);
            Some(details)
        }
        buck2_data::span_end_event::Data::ActionExecution(action) => {
            Some(profile_action_end_details(action))
        }
        buck2_data::span_end_event::Data::Load(load) => {
            let module = if load.module_id.is_empty() {
                load.cell.as_str()
            } else {
                load.module_id.as_str()
            };
            let mut details =
                profile_details(format!("load {module}"), ProfileLane::Loading, "loading");
            json_arg_non_empty(&mut details.args, "module_id", &load.module_id);
            json_arg_non_empty(&mut details.args, "cell", &load.cell);
            Some(details)
        }
        buck2_data::span_end_event::Data::LoadPackage(load) => {
            let mut details = profile_details(
                format!("list package {}", load.path),
                ProfileLane::Loading,
                "loading",
            );
            json_arg_non_empty(&mut details.args, "path", &load.path);
            Some(details)
        }
        buck2_data::span_end_event::Data::ExecutorStage(stage) => {
            let _ = stage;
            Some(profile_details(
                "executor stage",
                ProfileLane::ActionStages,
                "action_stage",
            ))
        }
        buck2_data::span_end_event::Data::TestDiscovery(test) => {
            let target = test
                .target_label
                .as_ref()
                .and_then(label_for_configured_target)
                .unwrap_or_else(|| "unknown target".to_owned());
            let mut details = profile_details(
                format!("discover tests {target}"),
                ProfileLane::Tests,
                "test",
            );
            json_arg_non_empty(&mut details.args, "suite", &test.suite_name);
            json_arg_non_empty(&mut details.args, "target", &target);
            Some(details)
        }
        buck2_data::span_end_event::Data::TestRun(test) => {
            let suite = test.suite.as_ref();
            let target = suite
                .and_then(|suite| suite.target_label.as_ref())
                .and_then(label_for_configured_target)
                .unwrap_or_else(|| "unknown target".to_owned());
            let mut details = profile_details(format!("test {target}"), ProfileLane::Tests, "test");
            if let Some(suite) = suite {
                json_arg_non_empty(&mut details.args, "suite", &suite.suite_name);
                json_arg_u64(
                    &mut details.args,
                    "test_count",
                    suite.test_names.len() as u64,
                );
            }
            json_arg_non_empty(&mut details.args, "target", &target);
            Some(details)
        }
        buck2_data::span_end_event::Data::CacheUpload(upload) => Some(
            profile_cache_upload_details(&upload.key, &upload.name, &upload.action_digest),
        ),
        buck2_data::span_end_event::Data::DepFileUpload(upload) => Some(
            profile_cache_upload_details(&upload.key, &upload.name, &upload.remote_dep_file_key),
        ),
        buck2_data::span_end_event::Data::Materialization(materialization) => {
            let mut details = profile_details(
                "materialization",
                ProfileLane::Materialization,
                "materialization",
            );
            if let Some(digest) = materialization.action_digest.as_ref() {
                json_arg_non_empty(&mut details.args, "action_digest", digest);
            }
            Some(details)
        }
        buck2_data::span_end_event::Data::FinalMaterialization(materialization) => {
            let mut details = profile_details(
                "final materialization",
                ProfileLane::Materialization,
                "materialization",
            );
            if let Some(artifact) = materialization.artifact.as_ref() {
                json_arg_non_empty(&mut details.args, "path", &artifact.path);
            }
            Some(details)
        }
        buck2_data::span_end_event::Data::CommandCritical(_) => Some(profile_details(
            "command critical section",
            ProfileLane::Command,
            "command",
        )),
        buck2_data::span_end_event::Data::SpanCancelled(_) => Some(profile_details(
            "cancelled span",
            ProfileLane::Other,
            "cancelled",
        )),
        _ => None,
    }
}

fn profile_span_end_args(
    data: &buck2_data::span_end_event::Data,
    tests_failed: bool,
) -> serde_json::Map<String, serde_json::Value> {
    let mut args = serde_json::Map::new();
    match data {
        buck2_data::span_end_event::Data::Command(command) => {
            let outcome = command_outcome(command, tests_failed);
            json_arg_bool(&mut args, "success", outcome.is_success());
            json_arg_i64(&mut args, "exit_code", i64::from(outcome.exit_code));
        }
        buck2_data::span_end_event::Data::ActionExecution(action) => {
            json_arg_bool(&mut args, "failed", action.failed);
            json_arg_u64(&mut args, "output_size", action.output_size);
            json_arg_u64(&mut args, "output_count", action.outputs.len() as u64);
            if let Ok(kind) = buck2_data::ActionExecutionKind::try_from(action.execution_kind) {
                json_arg_non_empty(&mut args, "execution_kind", &format!("{kind:?}"));
            }
            if let Some(command) = action.commands.last() {
                json_arg_non_empty(
                    &mut args,
                    "strategy",
                    action_execution_strategy_for_profile(command),
                );
                add_command_profile_args(&mut args, command);
            }
        }
        buck2_data::span_end_event::Data::Analysis(analysis) => {
            if let Some(declared_actions) = analysis.declared_actions {
                json_arg_u64(&mut args, "declared_actions", declared_actions);
            }
            if let Some(declared_artifacts) = analysis.declared_artifacts {
                json_arg_u64(&mut args, "declared_artifacts", declared_artifacts);
            }
            if let Some(profile) = analysis.profile.as_ref() {
                json_arg_u64(
                    &mut args,
                    "starlark_allocated_bytes",
                    profile.starlark_allocated_bytes,
                );
                json_arg_u64(
                    &mut args,
                    "starlark_available_bytes",
                    profile.starlark_available_bytes,
                );
            }
        }
        buck2_data::span_end_event::Data::Load(load) => {
            if let Some(error) = load.error.as_ref() {
                json_arg_non_empty(&mut args, "error", error);
            }
            if let Some(target_count) = load.target_count {
                json_arg_u64(&mut args, "target_count", target_count);
            }
            if let Some(bytes) = load.starlark_peak_allocated_bytes {
                json_arg_u64(&mut args, "starlark_peak_allocated_bytes", bytes);
            }
            if let Some(ticks) = load.starlark_tick_count {
                json_arg_u64(&mut args, "starlark_tick_count", ticks);
            }
            if let Some(instructions) = load.cpu_instruction_count {
                json_arg_u64(&mut args, "cpu_instruction_count", instructions);
            }
        }
        buck2_data::span_end_event::Data::TestDiscovery(test) => {
            json_arg_bool(&mut args, "re_cache_enabled", test.re_cache_enabled);
            if let Some(command) = test.command_report.as_ref() {
                add_command_profile_args(&mut args, command);
            }
        }
        buck2_data::span_end_event::Data::TestRun(test) => {
            if let Some(command) = test.command_report.as_ref() {
                add_command_profile_args(&mut args, command);
            }
        }
        buck2_data::span_end_event::Data::CacheUpload(upload) => {
            json_arg_bool(&mut args, "success", upload.success);
            json_arg_non_empty(&mut args, "error", &upload.error);
            json_arg_u64(&mut args, "file_count", upload.file_digests.len() as u64);
            json_arg_u64(&mut args, "tree_count", upload.tree_digests.len() as u64);
            if let Some(bytes) = upload.output_bytes {
                json_arg_u64(&mut args, "output_bytes", bytes);
            }
            if let Some(code) = upload.re_error_code.as_ref() {
                json_arg_non_empty(&mut args, "re_error_code", code);
            }
        }
        buck2_data::span_end_event::Data::DepFileUpload(upload) => {
            json_arg_bool(&mut args, "success", upload.success);
            json_arg_non_empty(&mut args, "error", &upload.error);
            if let Some(code) = upload.re_error_code.as_ref() {
                json_arg_non_empty(&mut args, "re_error_code", code);
            }
        }
        buck2_data::span_end_event::Data::Materialization(materialization) => {
            json_arg_u64(&mut args, "file_count", materialization.file_count);
            json_arg_u64(&mut args, "total_bytes", materialization.total_bytes);
            json_arg_non_empty(&mut args, "path", &materialization.path);
            json_arg_bool(&mut args, "success", materialization.success);
            if let Some(error) = materialization.error.as_ref() {
                json_arg_non_empty(&mut args, "error", error);
            }
            if let Some(method) = materialization
                .method
                .and_then(|method| buck2_data::MaterializationMethod::try_from(method).ok())
            {
                json_arg_non_empty(&mut args, "method", &format!("{method:?}"));
            }
        }
        buck2_data::span_end_event::Data::CreateOutputSymlinks(symlinks) => {
            json_arg_u64(&mut args, "created", symlinks.symlinks.len() as u64);
        }
        buck2_data::span_end_event::Data::SpanCancelled(_) => {
            json_arg_bool(&mut args, "cancelled", true);
        }
        _ => {}
    }
    args
}

fn profile_details(
    name: impl Into<String>,
    lane: ProfileLane,
    category: &'static str,
) -> ProfileSpanDetails {
    ProfileSpanDetails {
        name: name.into(),
        lane,
        category,
        args: serde_json::Map::new(),
    }
}

fn profile_action_start_details(action: &buck2_data::ActionExecutionStart) -> ProfileSpanDetails {
    let mut details = profile_details(
        profile_action_name(action.key.as_ref(), action.name.as_ref(), action.kind),
        ProfileLane::Actions,
        "action",
    );
    add_action_identity_args(
        &mut details.args,
        action.key.as_ref(),
        action.name.as_ref(),
        action.kind,
    );
    details
}

fn profile_action_end_details(action: &buck2_data::ActionExecutionEnd) -> ProfileSpanDetails {
    let mut details = profile_details(
        profile_action_name(action.key.as_ref(), action.name.as_ref(), action.kind),
        ProfileLane::Actions,
        "action",
    );
    add_action_identity_args(
        &mut details.args,
        action.key.as_ref(),
        action.name.as_ref(),
        action.kind,
    );
    details
}

fn profile_cache_upload_details(
    key: &Option<buck2_data::ActionKey>,
    name: &Option<buck2_data::ActionName>,
    digest: &str,
) -> ProfileSpanDetails {
    let mut details = profile_details(
        format!(
            "upload {}",
            profile_action_name(key.as_ref(), name.as_ref(), 0)
        ),
        ProfileLane::Cache,
        "cache",
    );
    add_action_identity_args(&mut details.args, key.as_ref(), name.as_ref(), 0);
    json_arg_non_empty(&mut details.args, "digest", digest);
    details
}

fn profile_action_name(
    key: Option<&buck2_data::ActionKey>,
    name: Option<&buck2_data::ActionName>,
    kind: i32,
) -> String {
    let mnemonic = profile_action_mnemonic(name, kind);
    let label = key
        .and_then(action_owner)
        .and_then(label_for_configured_target);
    let identifier = name
        .map(|name| name.identifier.as_str())
        .filter(|identifier| !identifier.is_empty());
    match (label, identifier) {
        (Some(label), Some(identifier)) => format!("{mnemonic}:{identifier} {label}"),
        (Some(label), None) => format!("{mnemonic} {label}"),
        (None, Some(identifier)) => format!("{mnemonic}:{identifier}"),
        (None, None) => mnemonic,
    }
}

fn profile_action_mnemonic(name: Option<&buck2_data::ActionName>, kind: i32) -> String {
    if let Some(name) = name
        && !name.category.is_empty()
    {
        return name.category.clone();
    }
    buck2_data::ActionKind::try_from(kind)
        .ok()
        .map(|kind| format!("{kind:?}"))
        .unwrap_or_else(|| "Action".to_owned())
}

fn add_action_identity_args(
    args: &mut serde_json::Map<String, serde_json::Value>,
    key: Option<&buck2_data::ActionKey>,
    name: Option<&buck2_data::ActionName>,
    kind: i32,
) {
    if let Some(label) = key
        .and_then(action_owner)
        .and_then(label_for_configured_target)
    {
        json_arg_non_empty(args, "target", &label);
    }
    if let Some(key) = key {
        json_arg_non_empty(args, "action_key", &key.key);
    }
    if let Some(name) = name {
        json_arg_non_empty(args, "category", &name.category);
        json_arg_non_empty(args, "identifier", &name.identifier);
    }
    if let Ok(kind) = buck2_data::ActionKind::try_from(kind) {
        json_arg_non_empty(args, "action_kind", &format!("{kind:?}"));
    }
}

fn profile_executor_stage_name(
    stage: Option<&buck2_data::executor_stage_start::Stage>,
) -> Option<String> {
    Some(match stage? {
        buck2_data::executor_stage_start::Stage::Re(re) => profile_re_stage_name(re.stage.as_ref()),
        buck2_data::executor_stage_start::Stage::Local(local) => {
            profile_local_stage_name(local.stage.as_ref())
        }
        buck2_data::executor_stage_start::Stage::CacheQuery(query) => match query.cache_type() {
            buck2_data::CacheType::ActionCache => "action cache query".to_owned(),
            buck2_data::CacheType::RemoteDepFileCache => "dep-file cache query".to_owned(),
        },
        buck2_data::executor_stage_start::Stage::CacheHit(hit) => match hit.cache_type() {
            buck2_data::CacheType::ActionCache => "action cache hit".to_owned(),
            buck2_data::CacheType::RemoteDepFileCache => "dep-file cache hit".to_owned(),
        },
        buck2_data::executor_stage_start::Stage::Prepare(_) => "prepare action".to_owned(),
        buck2_data::executor_stage_start::Stage::CasArtifact(_) => "cas artifact".to_owned(),
    })
}

fn profile_re_stage_name(stage: Option<&buck2_data::re_stage::Stage>) -> String {
    match stage {
        Some(buck2_data::re_stage::Stage::Execute(_)) => "remote execute",
        Some(buck2_data::re_stage::Stage::Download(_)) => "remote download",
        Some(buck2_data::re_stage::Stage::Queue(_)) => "remote queue",
        Some(buck2_data::re_stage::Stage::WorkerDownload(_)) => "remote worker download",
        Some(buck2_data::re_stage::Stage::WorkerUpload(_)) => "remote worker upload",
        Some(buck2_data::re_stage::Stage::Unknown(_)) => "remote unknown",
        Some(buck2_data::re_stage::Stage::MaterializeFailedInputs(_)) => {
            "remote materialize failed inputs"
        }
        Some(buck2_data::re_stage::Stage::BeforeActionExecution(_)) => "remote before action",
        Some(buck2_data::re_stage::Stage::AfterActionExecution(_)) => "remote after action",
        Some(buck2_data::re_stage::Stage::QueueOverQuota(_)) => "remote queue over quota",
        Some(buck2_data::re_stage::Stage::QueueNoWorkerAvailable(_)) => "remote queue no worker",
        Some(buck2_data::re_stage::Stage::QueueAcquiringDependencies(_)) => {
            "remote queue dependencies"
        }
        Some(buck2_data::re_stage::Stage::QueueCancelled(_)) => "remote queue cancelled",
        None => "remote stage",
    }
    .to_owned()
}

fn profile_local_stage_name(stage: Option<&buck2_data::local_stage::Stage>) -> String {
    match stage {
        Some(buck2_data::local_stage::Stage::Queued(_)) => "local queued",
        Some(buck2_data::local_stage::Stage::Execute(_)) => "local execute",
        Some(buck2_data::local_stage::Stage::MaterializeInputs(_)) => "local materialize inputs",
        Some(buck2_data::local_stage::Stage::PrepareOutputs(_)) => "local prepare outputs",
        Some(buck2_data::local_stage::Stage::FinalizeOutputs(_)) => "local finalize outputs",
        Some(buck2_data::local_stage::Stage::AcquireLocalResource(_)) => "local acquire resource",
        Some(buck2_data::local_stage::Stage::WorkerInit(_)) => "worker init",
        Some(buck2_data::local_stage::Stage::WorkerExecute(_)) => "worker execute",
        Some(buck2_data::local_stage::Stage::WorkerQueued(_)) => "worker queued",
        Some(buck2_data::local_stage::Stage::WorkerWait(_)) => "worker wait",
        None => "local stage",
    }
    .to_owned()
}

fn dynamic_lambda_target_label(
    lambda: &buck2_data::DynamicLambdaStart,
) -> Option<&buck2_data::ConfiguredTargetLabel> {
    match lambda.owner.as_ref()? {
        buck2_data::dynamic_lambda_start::Owner::TargetLabel(target) => Some(target),
        buck2_data::dynamic_lambda_start::Owner::BxlKey(_)
        | buck2_data::dynamic_lambda_start::Owner::AnonTarget(_) => None,
    }
}

fn profile_dice_span_name(data: &buck2_data::span_start_event::Data) -> &'static str {
    match data {
        buck2_data::span_start_event::Data::DiceStateUpdate(_) => "dice state update",
        buck2_data::span_start_event::Data::DiceCriticalSection(_) => "dice critical section",
        buck2_data::span_start_event::Data::DiceBlockConcurrentCommand(_) => {
            "dice block concurrent command"
        }
        buck2_data::span_start_event::Data::DiceSynchronizeSection(_) => "dice synchronize",
        buck2_data::span_start_event::Data::DiceCleanup(_) => "dice cleanup",
        _ => "dice",
    }
}

fn action_execution_strategy_for_profile(command: &buck2_data::CommandExecution) -> &'static str {
    command
        .details
        .as_ref()
        .and_then(|details| details.command_kind.as_ref())
        .and_then(|kind| kind.command.as_ref())
        .map(|command| action_execution_strategy(Some(command)))
        .unwrap_or("unknown")
}

fn add_command_profile_args(
    args: &mut serde_json::Map<String, serde_json::Value>,
    command: &buck2_data::CommandExecution,
) {
    let Some(details) = command.details.as_ref() else {
        return;
    };
    if let Some(exit_code) = details.signed_exit_code {
        json_arg_i64(args, "exit_code", i64::from(exit_code));
    }
    json_arg_duration_ms(
        args,
        "wall_time_ms",
        details.metadata.as_ref().and_then(|m| m.wall_time.as_ref()),
    );
    json_arg_duration_ms(
        args,
        "execution_time_ms",
        details
            .metadata
            .as_ref()
            .and_then(|m| m.execution_time.as_ref()),
    );
    json_arg_duration_ms(
        args,
        "queue_duration_ms",
        details
            .metadata
            .as_ref()
            .and_then(|m| m.queue_duration.as_ref()),
    );
}

fn json_arg_non_empty(
    args: &mut serde_json::Map<String, serde_json::Value>,
    name: &str,
    value: &str,
) {
    if !value.is_empty() {
        args.insert(name.to_owned(), serde_json::json!(value));
    }
}

fn json_arg_bool(args: &mut serde_json::Map<String, serde_json::Value>, name: &str, value: bool) {
    args.insert(name.to_owned(), serde_json::json!(value));
}

fn json_arg_i64(args: &mut serde_json::Map<String, serde_json::Value>, name: &str, value: i64) {
    args.insert(name.to_owned(), serde_json::json!(value));
}

fn json_arg_u64(args: &mut serde_json::Map<String, serde_json::Value>, name: &str, value: u64) {
    args.insert(name.to_owned(), serde_json::json!(value));
}

fn json_arg_duration_ms(
    args: &mut serde_json::Map<String, serde_json::Value>,
    name: &str,
    duration: Option<&prost_types::Duration>,
) {
    if let Some(duration) = duration {
        json_arg_i64(args, name, duration_millis(duration));
    }
}

fn extend_json_map(
    args: &mut serde_json::Map<String, serde_json::Value>,
    more: serde_json::Map<String, serde_json::Value>,
) {
    for (key, value) in more {
        args.insert(key, value);
    }
}

fn serialize_trace_events(events: &[serde_json::Value]) -> serde_json::Result<String> {
    let mut profile = "{\"traceEvents\":[\n".to_owned();
    for (i, event) in events.iter().enumerate() {
        if i != 0 {
            profile.push_str(",\n");
        }
        profile.push_str(&serde_json::to_string(event)?);
    }
    profile.push_str("\n]}\n");
    Ok(profile)
}

fn gzip_profile_json(profile: String) -> Option<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(profile.as_bytes()).ok()?;
    encoder.finish().ok()
}

fn build_tool_logs_event_from_command_end(
    event: &buck2_data::BuckEvent,
    span_end: &buck2_data::SpanEndEvent,
    command: &buck2_data::CommandEnd,
    outcome: CommandOutcome,
    profile: &CommandProfileBuilder,
) -> bep::BuildEvent {
    let command_name = command_end_name(command);
    let duration_ms = optional_duration_millis(span_end.duration.as_ref());
    let summary = serde_json::json!({
        "tool": BUILD_TOOL_VERSION,
        "command": command_name,
        "outcome": outcome.outcome_name(),
        "exit_code": outcome.exit_code,
        "timings_ms": {
            "command": duration_ms,
        },
    });
    let summary = serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "{}".to_owned());
    let mut logs = vec![file_with_contents("buck2-invocation.json", summary)];
    if let Some(profile) = profile.command_end_profile_gzip(event, span_end, command, outcome) {
        logs.push(file_with_bytes(COMMAND_PROFILE_NAME, profile));
    }
    build_tool_logs_event(logs)
}

fn build_tool_logs_event_from_invocation(
    record: &buck2_data::InvocationRecord,
    profile: &CommandProfileBuilder,
) -> bep::BuildEvent {
    let target_patterns = target_patterns_from_invocation(record);
    let command_name = invocation_command_name(record);
    let summary = serde_json::json!({
        "tool": BUILD_TOOL_VERSION,
        "command": command_name,
        "outcome": invocation_outcome_name(record.outcome),
        "exit_code": record.exit_code,
        "exit_result_name": record.exit_result_name.as_deref(),
        "target_patterns": target_patterns,
        "tags": &record.tags,
        "re_session_id": record.re_session_id.as_str(),
        "filesystem": record.filesystem.as_str(),
        "cache_hit_rate": record.cache_hit_rate,
        "actions": {
            "local": record.run_local_count,
            "remote": record.run_remote_count,
            "remote_cache": record.run_action_cache_count,
            "remote_dep_file_cache": record.run_remote_dep_file_cache_count,
            "skipped": record.run_skipped_count,
            "fallback": record.run_fallback_count,
            "uploads": record.cache_upload_count,
            "upload_attempts": record.cache_upload_attempt_count,
        },
        "timings_ms": {
            "command": optional_duration_millis(record.command_duration.as_ref()),
            "client_walltime": optional_duration_millis(record.client_walltime.as_ref()),
            "critical_path": optional_duration_millis(record.critical_path_duration.as_ref()),
            "first_action_execution": record.time_to_first_action_execution_ms,
            "last_action_execution_end": record.time_to_last_action_execution_end_ms,
        },
        "remote_execution": {
            "upload_bytes": record.re_upload_bytes,
            "download_bytes": record.re_download_bytes,
            "max_upload_speed": record.re_max_upload_speed,
            "max_download_speed": record.re_max_download_speed,
            "avg_upload_speed": record.re_avg_upload_speed,
            "avg_download_speed": record.re_avg_download_speed,
        },
        "sink": {
            "success_count": record.sink_success_count,
            "failure_count": record.sink_failure_count,
            "dropped_count": record.sink_dropped_count,
            "bytes_written": record.sink_bytes_written,
            "max_buffer_depth": record.sink_max_buffer_depth,
        },
        "event_log": {
            "compressed_size_bytes": record.compressed_event_log_size_bytes,
            "event_count": record.event_count,
            "has_end_of_stream": record.has_end_of_stream,
        },
    });
    let summary = serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "{}".to_owned());
    let mut logs = vec![file_with_contents("buck2-invocation.json", summary)];
    if let Some(profile) = profile
        .invocation_profile_gzip(record)
        .or_else(|| command_profile_gzip(record))
    {
        logs.push(file_with_bytes(COMMAND_PROFILE_NAME, profile));
    }
    build_tool_logs_event(logs)
}

fn build_tool_logs_event(logs: Vec<bep::File>) -> bep::BuildEvent {
    bep::BuildEvent {
        id: Some(build_tool_logs_id()),
        children: Vec::new(),
        payload: Some(build_event::Payload::BuildToolLogs(bep::BuildToolLogs {
            log: logs,
        })),
        last_message: true,
    }
}

fn command_profile_gzip(record: &buck2_data::InvocationRecord) -> Option<Vec<u8>> {
    gzip_profile_json(command_profile_json(record).ok()?)
}

fn command_profile_json(record: &buck2_data::InvocationRecord) -> serde_json::Result<String> {
    let command_duration_us = record
        .command_duration
        .as_ref()
        .or(record.client_walltime.as_ref())
        .map(duration_micros)
        .unwrap_or_default()
        .max(1);
    let command = invocation_command_name(record);
    let outcome = invocation_outcome_name(record.outcome);
    let total_actions = record
        .run_local_count
        .saturating_add(record.run_remote_count)
        .saturating_add(record.run_action_cache_count)
        .saturating_add(record.run_remote_dep_file_cache_count)
        .saturating_add(record.run_skipped_count);

    let mut trace_events = vec![
        serde_json::json!({
            "name": "thread_name",
            "ph": "M",
            "pid": 1,
            "tid": 1,
            "args": { "name": "Buck2 command" },
        }),
        serde_json::json!({
            "name": format!("buck2 {command}"),
            "cat": "command",
            "ph": "X",
            "ts": 0,
            "dur": command_duration_us,
            "pid": 1,
            "tid": 1,
            "args": {
                "outcome": outcome,
                "exit_code": record.exit_code,
                "exit_result_name": record.exit_result_name.as_deref(),
                "cache_hit_rate": record.cache_hit_rate,
                "re_session_id": record.re_session_id.as_str(),
            },
        }),
        serde_json::json!({
            "name": "action count",
            "cat": "buck2",
            "ph": "C",
            "ts": command_duration_us,
            "pid": 1,
            "tid": 1,
            "args": {
                "action": total_actions,
                "local action": record.run_local_count,
                "remote action": record.run_remote_count,
                "local action cache": record.run_action_cache_count,
                "remote dep file cache": record.run_remote_dep_file_cache_count,
                "skipped action": record.run_skipped_count,
            },
        }),
    ];

    if let (Some(first), Some(last)) = (
        record.time_to_first_action_execution_ms,
        record.time_to_last_action_execution_end_ms,
    ) && last >= first
    {
        trace_events.push(serde_json::json!({
            "name": "thread_name",
            "ph": "M",
            "pid": 1,
            "tid": 2,
            "args": { "name": "Buck2 actions" },
        }));
        trace_events.push(serde_json::json!({
            "name": "action execution",
            "cat": "action",
            "ph": "X",
            "ts": first.saturating_mul(1000),
            "dur": last.saturating_sub(first).max(1).saturating_mul(1000),
            "pid": 1,
            "tid": 2,
            "args": {
                "local": record.run_local_count,
                "remote": record.run_remote_count,
                "remote_cache": record.run_action_cache_count,
                "skipped": record.run_skipped_count,
            },
        }));
    }

    if let Some(critical_path) = record.critical_path_duration.as_ref() {
        let critical_path_us = duration_micros(critical_path).max(1);
        trace_events.push(serde_json::json!({
            "name": "thread_name",
            "ph": "M",
            "pid": 1,
            "tid": 3,
            "args": { "name": "Buck2 critical path" },
        }));
        trace_events.push(serde_json::json!({
            "name": "critical path",
            "cat": "critical_path",
            "ph": "X",
            "ts": command_duration_us.saturating_sub(critical_path_us),
            "dur": critical_path_us,
            "pid": 1,
            "tid": 3,
            "args": {},
        }));
    }

    serialize_trace_events(&trace_events)
}

fn configured_target_children_from_build_targets(
    targets: &[buck2_data::BuildTarget],
) -> Vec<bep::BuildEventId> {
    configured_target_labels_from_build_targets(targets)
        .into_iter()
        .map(target_configured_id)
        .collect()
}

fn configured_target_labels_from_build_targets(
    targets: &[buck2_data::BuildTarget],
) -> BTreeSet<String> {
    targets
        .iter()
        .filter_map(|target| normalize_buck_label(&target.target))
        .collect()
}

fn configured_target_children_from_events(events: &[bep::BuildEvent]) -> Vec<bep::BuildEventId> {
    let labels = configured_target_labels_from_events(events);
    configured_target_children_from_labels(&labels)
}

fn configured_target_labels_from_events(events: &[bep::BuildEvent]) -> BTreeSet<String> {
    events
        .iter()
        .filter_map(|event| match event.id.as_ref()?.id.as_ref()? {
            build_event_id::Id::TargetConfigured(id) => Some(id.label.clone()),
            _ => None,
        })
        .collect()
}

fn configured_target_children_from_labels(labels: &BTreeSet<String>) -> Vec<bep::BuildEventId> {
    labels.iter().cloned().map(target_configured_id).collect()
}

fn configured_target_children_matching_exact_patterns(
    patterns: &[String],
    labels: &BTreeSet<String>,
) -> Vec<bep::BuildEventId> {
    let Some(pattern_labels) = exact_target_labels_from_patterns(patterns) else {
        return Vec::new();
    };
    labels
        .iter()
        .filter(|label| {
            pattern_labels
                .iter()
                .any(|pattern| configured_label_matches_exact_pattern(label, pattern))
        })
        .cloned()
        .map(target_configured_id)
        .collect()
}

fn exact_target_labels_from_patterns(patterns: &[String]) -> Option<BTreeSet<String>> {
    if patterns.is_empty() {
        return None;
    }
    patterns
        .iter()
        .map(|pattern| exact_target_label_from_pattern(pattern))
        .collect()
}

fn configured_label_matches_exact_pattern(label: &str, pattern: &str) -> bool {
    if label == pattern {
        return true;
    }
    pattern.starts_with("//")
        && label
            .strip_prefix('@')
            .and_then(|label| label.find("//").map(|index| &label[index..]))
            .is_some_and(|unqualified| unqualified == pattern)
}

fn configured_target_children_from_exact_patterns(patterns: &[String]) -> Vec<bep::BuildEventId> {
    patterns
        .iter()
        .filter_map(|pattern| exact_target_label_from_pattern(pattern))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(target_configured_id)
        .collect()
}

fn configured_target_children_from_exact_patterns_for_root_cell(
    patterns: &[String],
    root_cell: &str,
) -> Vec<bep::BuildEventId> {
    let root_cell = root_cell.trim().trim_start_matches('@');
    if root_cell.is_empty() {
        return Vec::new();
    }
    patterns
        .iter()
        .filter_map(|pattern| exact_target_label_from_pattern(pattern))
        .filter_map(|label| {
            label
                .strip_prefix("//")
                .map(|target| format!("@{root_cell}//{target}"))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(target_configured_id)
        .collect()
}

fn exact_target_label_from_pattern(pattern: &str) -> Option<String> {
    let pattern = pattern.trim();
    if pattern.contains("...") {
        return None;
    }
    let (_, name) = pattern.rsplit_once(':')?;
    if name.is_empty() {
        return None;
    }
    normalize_buck_label(pattern)
}

/// A configured target BuildBuddy counts as a test: a known test size, or a rule kind ending
/// in "test" (target_tracker.go `isTest` at v2.310.0).
fn is_test_configured_event(event: &bep::BuildEvent) -> bool {
    let Some(build_event::Payload::Configured(configured)) = event.payload.as_ref() else {
        return false;
    };
    configured.test_size != bep::TestSize::Unknown as i32
        || configured
            .target_kind
            .to_lowercase()
            .trim_end_matches(" rule")
            .ends_with("test")
}

fn target_key_for(target: &buck2_data::ConfiguredTargetLabel) -> Option<TargetKey> {
    Some(TargetKey {
        label: label_for_configured_target(target)?,
        configuration: configuration_id_for_target(target),
    })
}

fn test_run_key(test_end: &buck2_data::TestRunEnd) -> Option<TargetKey> {
    target_key_for(test_end.suite.as_ref()?.target_label.as_ref()?)
}

fn pattern_expanded_event(
    patterns: Vec<String>,
    children: Vec<bep::BuildEventId>,
) -> bep::BuildEvent {
    bep::BuildEvent {
        id: Some(pattern_expanded_id(patterns.clone())),
        children,
        payload: Some(build_event::Payload::Expanded(bep::PatternExpanded {
            test_suite_expansions: Vec::new(),
        })),
        last_message: false,
    }
}

fn pattern_expanded_id(patterns: Vec<String>) -> bep::BuildEventId {
    bep::BuildEventId {
        id: Some(build_event_id::Id::Pattern(
            build_event_id::PatternExpandedId { pattern: patterns },
        )),
    }
}

fn configured_event_from_analysis_start(
    analysis: &buck2_data::AnalysisStart,
) -> Option<bep::BuildEvent> {
    let target = configured_target_from_analysis_start(analysis)?;
    let label = label_for_configured_target(target)?;
    let mut target_kind = analysis.rule.trim().to_owned();
    if target_kind.is_empty() {
        target_kind = GENERIC_TARGET_KIND.to_owned();
    } else if !target_kind.ends_with(" rule") {
        target_kind.push_str(" rule");
    }
    Some(configured_event(
        label,
        configuration_id_for_target(target),
        target_kind,
    ))
}

fn completed_event_from_analysis_end(
    analysis: &buck2_data::AnalysisEnd,
) -> Option<bep::BuildEvent> {
    let target = configured_target_from_analysis_end(analysis)?;
    let label = label_for_configured_target(target)?;
    let configuration = configuration_id_for_target(target);
    Some(completed_event(label, configuration, true))
}

fn configured_event_from_test_label(
    label: Option<&buck2_data::ConfiguredTargetLabel>,
    target_kind: &str,
    tags: &[String],
) -> Option<bep::BuildEvent> {
    Some(configured_event_with_tags(
        label_for_configured_target(label?)?,
        configuration_id_for_target(label?),
        target_kind.to_owned(),
        tags.to_vec(),
    ))
}

fn configured_event_from_test_suite(suite: &buck2_data::TestSuite) -> Option<bep::BuildEvent> {
    configured_event_from_test_label(suite.target_label.as_ref(), TEST_TARGET_KIND, &suite.labels)
}

fn configured_event_from_build_target(target: &buck2_data::BuildTarget) -> Option<bep::BuildEvent> {
    let label = normalize_buck_label(&target.target)?;
    Some(configured_event(
        label,
        configuration_id_for_build_target(target),
        GENERIC_TARGET_KIND.to_owned(),
    ))
}

fn completed_event_from_build_target(target: &buck2_data::BuildTarget) -> Option<bep::BuildEvent> {
    let label = normalize_buck_label(&target.target)?;
    Some(completed_event(
        label,
        configuration_id_for_build_target(target),
        true,
    ))
}

fn configuration_id_for_build_target(target: &buck2_data::BuildTarget) -> String {
    if target.configuration.is_empty() {
        DEFAULT_CONFIGURATION_ID.to_owned()
    } else {
        target.configuration.clone()
    }
}

fn configured_event(label: String, configuration: String, target_kind: String) -> bep::BuildEvent {
    configured_event_with_tags(label, configuration, target_kind, Vec::new())
}

fn configured_event_with_tags(
    label: String,
    configuration: String,
    target_kind: String,
    tag: Vec<String>,
) -> bep::BuildEvent {
    let test_size = test_size_for_target_kind_and_tags(&target_kind, &tag);
    bep::BuildEvent {
        id: Some(target_configured_id(label.clone())),
        children: vec![target_completed_id(label, configuration)],
        payload: Some(build_event::Payload::Configured(bep::TargetConfigured {
            target_kind,
            test_size,
            tag,
        })),
        last_message: false,
    }
}

fn test_size_for_target_kind_and_tags(target_kind: &str, tags: &[String]) -> i32 {
    let target_kind = target_kind
        .trim()
        .trim_end_matches(" rule")
        .to_ascii_lowercase();
    if !target_kind.ends_with("test") {
        return bep::TestSize::Unknown as i32;
    }
    tags.iter()
        .find_map(|tag| match tag.as_str() {
            "small" => Some(bep::TestSize::Small as i32),
            "medium" => Some(bep::TestSize::Medium as i32),
            "large" => Some(bep::TestSize::Large as i32),
            "enormous" => Some(bep::TestSize::Enormous as i32),
            _ => None,
        })
        .unwrap_or(bep::TestSize::Medium as i32)
}

fn completed_event(label: String, configuration: String, success: bool) -> bep::BuildEvent {
    completed_event_with_children(label, configuration, success, Vec::new())
}

fn completed_event_with_children(
    label: String,
    configuration: String,
    success: bool,
    children: Vec<bep::BuildEventId>,
) -> bep::BuildEvent {
    completed_event_with_children_and_outputs(
        label,
        configuration,
        success,
        children,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
    )
}

fn completed_event_with_children_and_outputs(
    label: String,
    configuration: String,
    success: bool,
    children: Vec<bep::BuildEventId>,
    output_group: Vec<bep::OutputGroup>,
    important_output: Vec<bep::File>,
    directory_output: Vec<bep::File>,
    tag: Vec<String>,
    test_timeout: Option<prost_types::Duration>,
) -> bep::BuildEvent {
    let test_timeout_seconds = test_timeout.as_ref().map(duration_seconds_i64).unwrap_or(0);
    bep::BuildEvent {
        id: Some(target_completed_id(label.clone(), configuration)),
        children,
        payload: Some(build_event::Payload::Completed(bep::TargetComplete {
            success,
            target_kind: String::new(),
            test_size: bep::TestSize::Unknown as i32,
            output_group,
            important_output,
            tag,
            test_timeout_seconds,
            directory_output,
            failure_detail: (!success)
                .then(|| execution_failure_detail(format!("Target failed: {label}"))),
            test_timeout,
        })),
        last_message: false,
    }
}

fn action_event(
    event: &buck2_data::BuckEvent,
    span_end: &buck2_data::SpanEndEvent,
    action: &buck2_data::ActionExecutionEnd,
) -> Option<bep::BuildEvent> {
    let key = action.key.as_ref()?;
    let target = action_owner(key)?;
    let label = label_for_configured_target(target)?;
    let configuration = configuration_id_for_target(target);
    let last_command = action.commands.last();
    let primary_output = action
        .outputs
        .first()
        .map(|o| o.tiny_digest.clone())
        .filter(|v| !v.is_empty())
        .or_else(|| (!key.key.is_empty()).then(|| key.key.clone()))
        .unwrap_or_else(|| label.clone());
    let stdout = last_command
        .and_then(|command| command.details.as_ref())
        .map(|details| details.cmd_stdout.clone())
        .unwrap_or_default();
    let stderr = last_command
        .and_then(|command| command.details.as_ref())
        .map(|details| details.cmd_stderr.clone())
        .unwrap_or_default();
    let exit_code = last_command
        .and_then(|command| command.details.as_ref())
        .and_then(|details| details.signed_exit_code)
        .unwrap_or(if action.failed { 1 } else { 0 });
    let end_time = event.timestamp.clone();
    let start_time = start_time_from_span_end(span_end, end_time.as_ref());
    let command_line = last_command
        .map(command_line_from_execution)
        .unwrap_or_default();
    let mnemonic = action_mnemonic(action);
    let strategy_details =
        action_strategy_details(action, last_command, &label, &mnemonic, exit_code);
    let failure_detail = action.failed.then(|| {
        execution_failure_detail(action_failure_detail_message(
            action, &label, &stderr, exit_code,
        ))
    });
    let primary_output_file = (!action.failed)
        .then(|| action.outputs.first().and_then(bep_file_from_action_output))
        .flatten();

    Some(bep::BuildEvent {
        id: Some(bep::BuildEventId {
            id: Some(build_event_id::Id::ActionCompleted(
                build_event_id::ActionCompletedId {
                    primary_output: primary_output.clone(),
                    label: label.clone(),
                    configuration: Some(configuration_id(configuration.clone())),
                },
            )),
        }),
        children: Vec::new(),
        payload: Some(build_event::Payload::Action(bep::ActionExecuted {
            success: !action.failed,
            exit_code,
            stdout: Some(file_with_contents("stdout", stdout)),
            stderr: Some(file_with_contents("stderr", stderr)),
            label,
            primary_output: primary_output_file,
            configuration: Some(configuration_id(configuration)),
            r#type: mnemonic,
            command_line,
            action_metadata_logs: Vec::new(),
            failure_detail,
            start_time,
            end_time,
            strategy_details,
        })),
        last_message: false,
    })
}

fn bep_file_from_action_output(output: &buck2_data::ActionOutput) -> Option<bep::File> {
    let name = if output.path.is_empty() {
        output.tiny_digest.clone()
    } else {
        output.path.clone()
    };
    if name.is_empty() {
        return None;
    }
    Some(bep::File {
        name,
        path_prefix: Vec::new(),
        file: action_output_uri(output).map(bep::file::File::Uri),
        digest: output.digest.clone(),
        length: i64::try_from(output.size).unwrap_or(i64::MAX),
    })
}

fn action_output_uri(output: &buck2_data::ActionOutput) -> Option<String> {
    // Buck2 does not yet know the BES bytestream authority here, so keep this
    // empty for now. The full digest and size are still useful for consumers
    // that can map Buck2 output digests to their own CAS.
    let _ = output;
    None
}

fn finished_event_from_invocation_record(
    event: &buck2_data::BuckEvent,
    record: &buck2_data::InvocationRecord,
) -> bep::BuildEvent {
    let mut exit_code = 1;
    let mut exit_name = "FAILED".to_owned();

    match buck2_data::InvocationOutcome::try_from(record.outcome.unwrap_or_default()) {
        Ok(buck2_data::InvocationOutcome::Success) => {
            exit_code = 0;
            exit_name = "SUCCESS".to_owned();
        }
        Ok(buck2_data::InvocationOutcome::Cancelled) => {
            exit_code = INTERRUPTED_EXIT_CODE;
            exit_name = "INTERRUPTED".to_owned();
        }
        Ok(buck2_data::InvocationOutcome::Crashed) => {
            exit_name = "CRASHED".to_owned();
        }
        Ok(buck2_data::InvocationOutcome::Failed)
        | Ok(buck2_data::InvocationOutcome::Unknown)
        | Err(_) => {}
    }

    if let Some(record_exit_code) = record.exit_code {
        exit_code = i32::try_from(record_exit_code).unwrap_or(i32::MAX);
        if record.exit_result_name.is_none() {
            exit_name = if exit_code == 0 { "SUCCESS" } else { "FAILED" }.to_owned();
        }
    }
    if let Some(record_exit_name) = record.exit_result_name.as_ref()
        && !record_exit_name.is_empty()
    {
        exit_name = record_exit_name.clone();
    }

    finished_event(
        event.timestamp.clone(),
        exit_code,
        exit_name,
        vec![build_tool_logs_id(), build_metrics_id()],
    )
}

/// How a command ended, in Bazel's exit codes, so a BES server reads it as it reads Bazel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CommandOutcome {
    exit_code: i32,
    exit_name: &'static str,
}

impl CommandOutcome {
    fn is_success(self) -> bool {
        self.exit_code == 0
    }

    fn outcome_name(self) -> &'static str {
        if self.is_success() {
            "success"
        } else {
            "failed"
        }
    }
}

/// `is_success` is only whether the command returned `Ok`, and a build or test with failed
/// targets returns `Ok` with the failures in its response, so it says nothing about them on its
/// own. `build_result.build_completed` is false when the response carried errors, and the test
/// result comes from the test events, because TestCommandEnd does not carry it.
fn command_outcome(command: &buck2_data::CommandEnd, tests_failed: bool) -> CommandOutcome {
    let (exit_code, exit_name) = if !command.is_success {
        (1, "FAILED")
    } else if command
        .build_result
        .as_ref()
        .is_some_and(|result| !result.build_completed)
    {
        (BUILD_FAILURE_EXIT_CODE, "BUILD_FAILURE")
    } else if tests_failed && matches!(command.data, Some(buck2_data::command_end::Data::Test(_))) {
        (TESTS_FAILED_EXIT_CODE, "TESTS_FAILED")
    } else {
        (0, "SUCCESS")
    };
    CommandOutcome {
        exit_code,
        exit_name,
    }
}

fn finished_event_from_command_end(
    event: &buck2_data::BuckEvent,
    outcome: CommandOutcome,
) -> bep::BuildEvent {
    finished_event(
        event.timestamp,
        outcome.exit_code,
        outcome.exit_name,
        vec![build_tool_logs_id()],
    )
}

/// The final event for a build the daemon abandons: shutdown reached the stream before
/// CommandEnd did. Bazel's exit code for an interrupted build, and `last_message`, so the
/// server closes the invocation instead of showing it running forever.
pub fn interrupted_finish_event(timestamp: Option<Timestamp>) -> bep::BuildEvent {
    let mut event = finished_event(timestamp, 8, "INTERRUPTED", Vec::new());
    event.last_message = true;
    event
}

fn finished_event(
    timestamp: Option<Timestamp>,
    exit_code: i32,
    exit_name: impl Into<String>,
    children: Vec<bep::BuildEventId>,
) -> bep::BuildEvent {
    let exit_name = exit_name.into();
    let finish_time_millis = timestamp_millis(timestamp.as_ref());
    bep::BuildEvent {
        id: Some(build_finished_id()),
        children,
        payload: Some(build_event::Payload::Finished(bep::BuildFinished {
            overall_success: exit_code == 0,
            finish_time_millis,
            anomaly_report: None,
            exit_code: Some(bep::build_finished::ExitCode {
                name: exit_name.clone(),
                code: exit_code,
            }),
            finish_time: timestamp,
            failure_detail: (exit_code != 0).then(|| {
                execution_failure_detail(format!(
                    "Buck2 invocation failed with {exit_name} ({exit_code})"
                ))
            }),
        })),
        last_message: false,
    }
}

fn is_build_finished_event(event: &bep::BuildEvent) -> bool {
    matches!(event.payload, Some(build_event::Payload::Finished(_)))
}

fn is_target_lifecycle_event(event: &bep::BuildEvent) -> bool {
    matches!(
        event.payload,
        Some(
            build_event::Payload::Action(_)
                | build_event::Payload::Completed(_)
                | build_event::Payload::TestResult(_)
                | build_event::Payload::TestSummary(_)
                | build_event::Payload::Aborted(_)
        )
    )
}

fn finished_event_signature(event: &bep::BuildEvent) -> Option<FinishedEventSignature> {
    let Some(build_event::Payload::Finished(finished)) = event.payload.as_ref() else {
        return None;
    };
    let exit_code = finished.exit_code.as_ref();
    Some(FinishedEventSignature {
        children: event.children.iter().map(event_id_key).collect(),
        exit_code: exit_code.map(|exit| exit.code).unwrap_or_default(),
        exit_name: exit_code.map(|exit| exit.name.clone()).unwrap_or_default(),
        overall_success: finished.overall_success,
    })
}

fn abort_reason_from_finished_events(events: &[bep::BuildEvent]) -> i32 {
    events
        .iter()
        .find_map(|event| match event.payload.as_ref() {
            Some(build_event::Payload::Finished(finished)) => {
                Some(abort_reason_from_finished(finished))
            }
            _ => None,
        })
        .unwrap_or(bep::aborted::AbortReason::Unknown as i32)
}

fn abort_reason_from_finished(finished: &bep::BuildFinished) -> i32 {
    let exit_code = finished.exit_code.as_ref();
    let exit_name = exit_code.map(|exit| exit.name.as_str()).unwrap_or_default();
    let exit_code = exit_code.map(|exit| exit.code).unwrap_or_default();

    if exit_name == "INTERRUPTED" || exit_code == INTERRUPTED_EXIT_CODE {
        bep::aborted::AbortReason::UserInterrupted as i32
    } else if exit_name == "CRASHED" {
        bep::aborted::AbortReason::Internal as i32
    } else if finished.overall_success {
        bep::aborted::AbortReason::Unknown as i32
    } else {
        bep::aborted::AbortReason::Incomplete as i32
    }
}

fn aborted_event(id: bep::BuildEventId, reason: i32) -> bep::BuildEvent {
    bep::BuildEvent {
        id: Some(id),
        children: Vec::new(),
        payload: Some(build_event::Payload::Aborted(bep::Aborted {
            reason,
            description:
                "Event was announced by Buck2 but did not occur before the invocation finished."
                    .to_owned(),
        })),
        last_message: false,
    }
}

fn execution_failure_detail(message: impl Into<String>) -> failure::FailureDetail {
    failure::FailureDetail {
        message: message.into(),
        category: Some(failure::failure_detail::Category::Execution(
            failure::Execution {
                code: failure::execution::Code::NonActionExecutionFailure as i32,
            },
        )),
    }
}

fn action_failure_detail_message(
    action: &buck2_data::ActionExecutionEnd,
    label: &str,
    stderr: &str,
    exit_code: i32,
) -> String {
    if let Some(message) = action.error.as_ref().and_then(action_error_detail_message) {
        return format!("Action failed for {label}: {message}");
    }
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return format!("Action failed for {label}: {}", first_line(stderr));
    }
    format!("Action failed for {label} with exit code {exit_code}")
}

fn action_error_detail_message(error: &buck2_data::action_execution_end::Error) -> Option<String> {
    match error {
        buck2_data::action_execution_end::Error::Unknown(message) => {
            (!message.is_empty()).then(|| message.clone())
        }
        buck2_data::action_execution_end::Error::MissingOutputs(_) => {
            Some("missing outputs".to_owned())
        }
        buck2_data::action_execution_end::Error::CommandExecutionError(_) => None,
    }
}

fn first_line(value: &str) -> &str {
    value.lines().next().unwrap_or(value)
}

fn action_target_key(action: &buck2_data::ActionExecutionEnd) -> Option<TargetKey> {
    let target = action_owner(action.key.as_ref()?)?;
    Some(TargetKey {
        label: label_for_configured_target(target)?,
        configuration: configuration_id_for_target(target),
    })
}

fn action_owner(key: &buck2_data::ActionKey) -> Option<&buck2_data::ConfiguredTargetLabel> {
    match key.owner.as_ref()? {
        buck2_data::action_key::Owner::TargetLabel(label)
        | buck2_data::action_key::Owner::TestTargetLabel(label)
        | buck2_data::action_key::Owner::LocalResourceSetup(label) => Some(label),
        buck2_data::action_key::Owner::BxlKey(_) | buck2_data::action_key::Owner::AnonTarget(_) => {
            None
        }
    }
}

fn configured_target_from_analysis_start(
    analysis: &buck2_data::AnalysisStart,
) -> Option<&buck2_data::ConfiguredTargetLabel> {
    match analysis.target.as_ref()? {
        buck2_data::analysis_start::Target::StandardTarget(target) => Some(target),
        buck2_data::analysis_start::Target::DynamicLambda(owner) => {
            configured_target_from_dynamic_owner(owner)
        }
        buck2_data::analysis_start::Target::AnonTarget(_) => None,
    }
}

fn configured_target_from_analysis_end(
    analysis: &buck2_data::AnalysisEnd,
) -> Option<&buck2_data::ConfiguredTargetLabel> {
    match analysis.target.as_ref()? {
        buck2_data::analysis_end::Target::StandardTarget(target) => Some(target),
        buck2_data::analysis_end::Target::DynamicLambda(owner) => {
            configured_target_from_dynamic_owner(owner)
        }
        buck2_data::analysis_end::Target::AnonTarget(_) => None,
    }
}

fn configured_target_from_dynamic_owner(
    owner: &buck2_data::DynamicLambdaOwner,
) -> Option<&buck2_data::ConfiguredTargetLabel> {
    match owner.owner.as_ref()? {
        buck2_data::dynamic_lambda_owner::Owner::TargetLabel(target) => Some(target),
        buck2_data::dynamic_lambda_owner::Owner::BxlKey(_)
        | buck2_data::dynamic_lambda_owner::Owner::AnonTarget(_) => None,
    }
}

fn label_for_configured_target(target: &buck2_data::ConfiguredTargetLabel) -> Option<String> {
    label_for_target(target.label.as_ref()?)
}

fn label_for_target(label: &buck2_data::TargetLabel) -> Option<String> {
    label_for_package_and_name(&label.package, &label.name)
}

fn label_for_package_and_name(package: &str, name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let label = if package.is_empty() {
        format!(":{}", name)
    } else {
        format!("{package}:{name}")
    };
    normalize_buck_label(&label)
}

fn normalize_buck_label(label: &str) -> Option<String> {
    let label = label.trim();
    if label.is_empty() {
        return None;
    }
    if label.starts_with('@') {
        return Some(label.to_owned());
    }

    let label = label.strip_prefix("//").unwrap_or(label);
    if let Some((cell, rest)) = label.split_once("//") {
        let rest = rest.trim_start_matches('/');
        if cell == "root" {
            Some(format!("//{rest}"))
        } else {
            Some(format!("@{cell}//{rest}"))
        }
    } else if label.starts_with(':') {
        Some(format!("//{label}"))
    } else {
        Some(format!("//{label}"))
    }
}

fn configuration_id_for_target(target: &buck2_data::ConfiguredTargetLabel) -> String {
    target
        .configuration
        .as_ref()
        .map(|configuration| configuration.full_name.clone())
        .filter(|configuration| !configuration.is_empty())
        .unwrap_or_else(|| DEFAULT_CONFIGURATION_ID.to_owned())
}

fn command_line_from_execution(command: &buck2_data::CommandExecution) -> Vec<String> {
    let Some(details) = command.details.as_ref() else {
        return Vec::new();
    };
    let Some(kind) = details.command_kind.as_ref() else {
        return Vec::new();
    };
    let Some(command) = kind.command.as_ref() else {
        return Vec::new();
    };
    match command {
        buck2_data::command_execution_kind::Command::LocalCommand(command) => command.argv.clone(),
        buck2_data::command_execution_kind::Command::WorkerCommand(command) => command.argv.clone(),
        buck2_data::command_execution_kind::Command::WorkerInitCommand(command) => {
            command.argv.clone()
        }
        buck2_data::command_execution_kind::Command::RemoteCommand(_)
        | buck2_data::command_execution_kind::Command::OmittedLocalCommand(_) => Vec::new(),
    }
}

fn action_strategy_details(
    action: &buck2_data::ActionExecutionEnd,
    command: Option<&buck2_data::CommandExecution>,
    label: &str,
    mnemonic: &str,
    exit_code: i32,
) -> Vec<Any> {
    let mut fields = BTreeMap::new();
    insert_string_field(&mut fields, "tool", "buck2");
    insert_string_field(&mut fields, "label", label);
    insert_string_field(&mut fields, "mnemonic", mnemonic);
    insert_bool_field(&mut fields, "success", !action.failed);
    insert_number_field(&mut fields, "exit_code", f64::from(exit_code));
    insert_number_field(&mut fields, "output_count", action.outputs.len() as f64);
    insert_number_field(&mut fields, "output_size_bytes", action.output_size as f64);
    insert_bool_field(&mut fields, "prefers_local", action.prefers_local);
    insert_bool_field(&mut fields, "requires_local", action.requires_local);
    insert_bool_field(
        &mut fields,
        "allows_cache_upload",
        action.allows_cache_upload,
    );

    let command_kind = command
        .and_then(|command| command.details.as_ref())
        .and_then(|details| details.command_kind.as_ref())
        .and_then(|kind| kind.command.as_ref());

    insert_string_field(
        &mut fields,
        "strategy",
        action_execution_strategy(command_kind),
    );

    if let Some(command) = command {
        insert_command_metadata_fields(&mut fields, command);
    }

    if let Some(command_kind) = command_kind {
        insert_command_kind_fields(&mut fields, command_kind);
    }

    vec![struct_any(fields)]
}

fn insert_command_metadata_fields(
    fields: &mut BTreeMap<String, Value>,
    command: &buck2_data::CommandExecution,
) {
    let Some(metadata) = command
        .details
        .as_ref()
        .and_then(|details| details.metadata.as_ref())
    else {
        return;
    };

    insert_optional_duration_field(fields, "wall_time_ms", metadata.wall_time.as_ref());
    insert_optional_duration_field(
        fields,
        "execution_time_ms",
        metadata.execution_time.as_ref(),
    );
    insert_optional_duration_field(
        fields,
        "input_materialization_ms",
        metadata.input_materialization_duration.as_ref(),
    );
    insert_optional_duration_field(
        fields,
        "queue_duration_ms",
        metadata.queue_duration.as_ref(),
    );
}

fn insert_command_kind_fields(
    fields: &mut BTreeMap<String, Value>,
    command: &buck2_data::command_execution_kind::Command,
) {
    match command {
        buck2_data::command_execution_kind::Command::LocalCommand(command) => {
            insert_string_field(fields, "runner", "local");
            insert_argv_fields(fields, &command.argv);
            insert_non_empty_string_field(fields, "action_digest", &command.action_digest);
        }
        buck2_data::command_execution_kind::Command::WorkerCommand(command) => {
            insert_string_field(fields, "runner", "worker");
            insert_argv_fields(fields, &command.argv);
            insert_non_empty_string_field(fields, "action_digest", &command.action_digest);
            insert_number_field(
                fields,
                "fallback_arg_count",
                command.fallback_exe.len() as f64,
            );
        }
        buck2_data::command_execution_kind::Command::WorkerInitCommand(command) => {
            insert_string_field(fields, "runner", "worker-init");
            insert_argv_fields(fields, &command.argv);
        }
        buck2_data::command_execution_kind::Command::RemoteCommand(command) => {
            insert_string_field(fields, "runner", "remote");
            insert_non_empty_string_field(fields, "action_digest", &command.action_digest);
            insert_bool_field(fields, "cache_hit", command.cache_hit);
            if let Ok(cache_hit_type) =
                buck2_data::RemoteCommandOrigin::try_from(command.cache_hit_type)
            {
                insert_string_field(fields, "cache_hit_type", cache_hit_type.as_str_name());
            }
            insert_optional_duration_field(fields, "queue_time_ms", command.queue_time.as_ref());
            if let Some(remote_dep_file_key) = command.remote_dep_file_key.as_ref() {
                insert_non_empty_string_field(fields, "remote_dep_file_key", remote_dep_file_key);
            }
            insert_number_field(
                fields,
                "materialized_failed_input_count",
                command.materialized_inputs_for_failed.len() as f64,
            );
            insert_number_field(
                fields,
                "materialized_failed_output_count",
                command.materialized_outputs_for_failed_actions.len() as f64,
            );
            if let Some(details) = command.details.as_ref() {
                if let Some(session_id) = details.session_id.as_ref() {
                    insert_non_empty_string_field(fields, "remote_session_id", session_id);
                }
                insert_non_empty_string_field(fields, "remote_use_case", &details.use_case);
                insert_bool_field(
                    fields,
                    "remote_persistent_worker",
                    details.persistent_worker,
                );
                insert_platform_field(fields, details.platform.as_ref());
            }
        }
        buck2_data::command_execution_kind::Command::OmittedLocalCommand(command) => {
            insert_string_field(fields, "runner", "local-omitted");
            insert_non_empty_string_field(fields, "action_digest", &command.action_digest);
        }
    }
}

fn action_execution_strategy(
    command: Option<&buck2_data::command_execution_kind::Command>,
) -> &'static str {
    let Some(command) = command else {
        return "unknown";
    };
    match command {
        buck2_data::command_execution_kind::Command::LocalCommand(_)
        | buck2_data::command_execution_kind::Command::OmittedLocalCommand(_) => "local",
        buck2_data::command_execution_kind::Command::WorkerCommand(_)
        | buck2_data::command_execution_kind::Command::WorkerInitCommand(_) => "worker",
        buck2_data::command_execution_kind::Command::RemoteCommand(command) => {
            match buck2_data::RemoteCommandOrigin::try_from(command.cache_hit_type) {
                Ok(
                    buck2_data::RemoteCommandOrigin::ActionCache
                    | buck2_data::RemoteCommandOrigin::ReServedFromCache,
                ) => "remote-cache",
                Ok(buck2_data::RemoteCommandOrigin::RemoteDepFileCache) => "remote-dep-file-cache",
                Ok(
                    buck2_data::RemoteCommandOrigin::Executed
                    | buck2_data::RemoteCommandOrigin::ReDeduplicated,
                )
                | Err(_) => "remote",
            }
        }
    }
}

fn insert_platform_field(
    fields: &mut BTreeMap<String, Value>,
    platform: Option<&buck2_data::RePlatform>,
) {
    let Some(platform) = platform else {
        return;
    };
    let platform = platform
        .properties
        .iter()
        .filter(|property| !property.name.is_empty())
        .map(|property| format!("{}={}", property.name, property.value))
        .collect::<Vec<_>>();
    if platform.is_empty() {
        return;
    }
    insert_string_field(fields, "remote_platform", platform.join(","));
}

fn insert_argv_fields(fields: &mut BTreeMap<String, Value>, argv: &[String]) {
    if let Some(argv0) = argv.first() {
        insert_non_empty_string_field(fields, "argv0", argv0);
    }
    insert_number_field(fields, "argv_count", argv.len() as f64);
}

fn insert_optional_duration_field(
    fields: &mut BTreeMap<String, Value>,
    name: &str,
    duration: Option<&prost_types::Duration>,
) {
    if let Some(duration) = duration {
        insert_number_field(fields, name, duration_millis(duration) as f64);
    }
}

fn insert_non_empty_string_field(fields: &mut BTreeMap<String, Value>, name: &str, value: &str) {
    if !value.is_empty() {
        insert_string_field(fields, name, value);
    }
}

fn insert_string_field(fields: &mut BTreeMap<String, Value>, name: &str, value: impl Into<String>) {
    fields.insert(
        name.to_owned(),
        Value {
            kind: Some(value::Kind::StringValue(value.into())),
        },
    );
}

fn insert_bool_field(fields: &mut BTreeMap<String, Value>, name: &str, value: bool) {
    fields.insert(
        name.to_owned(),
        Value {
            kind: Some(value::Kind::BoolValue(value)),
        },
    );
}

fn insert_number_field(fields: &mut BTreeMap<String, Value>, name: &str, value: f64) {
    fields.insert(
        name.to_owned(),
        Value {
            kind: Some(value::Kind::NumberValue(value)),
        },
    );
}

fn struct_any(fields: BTreeMap<String, Value>) -> Any {
    Any {
        type_url: STRUCT_TYPE_URL.to_owned(),
        value: Struct { fields }.encode_to_vec(),
    }
}

fn action_mnemonic(action: &buck2_data::ActionExecutionEnd) -> String {
    if let Some(name) = action.name.as_ref()
        && !name.category.is_empty()
    {
        return name.category.clone();
    }
    buck2_data::ActionKind::try_from(action.kind)
        .ok()
        .map(|kind| format!("{kind:?}"))
        .unwrap_or_else(|| "Action".to_owned())
}

fn action_counts_as_executed(action: &buck2_data::ActionExecutionEnd) -> bool {
    !matches!(
        buck2_data::ActionExecutionKind::try_from(action.execution_kind),
        Ok(buck2_data::ActionExecutionKind::LocalActionCache)
            | Ok(buck2_data::ActionExecutionKind::LocalDepFile)
    )
}

fn last_command_execution_time_ms(action: &buck2_data::ActionExecutionEnd) -> u64 {
    action
        .commands
        .last()
        .and_then(|command| command.details.as_ref())
        .and_then(|details| details.metadata.as_ref())
        .and_then(|metadata| metadata.execution_time.as_ref())
        .map(|duration| {
            u64::try_from(duration.seconds)
                .unwrap_or_default()
                .saturating_mul(1_000)
                .saturating_add(u64::try_from(duration.nanos).unwrap_or_default() / 1_000_000)
        })
        .unwrap_or_default()
}

fn action_outputs_from_local_action_cache(action: &buck2_data::ActionExecutionEnd) -> bool {
    matches!(
        buck2_data::ActionExecutionKind::try_from(action.execution_kind),
        Ok(buck2_data::ActionExecutionKind::LocalActionCache)
    )
}

fn start_time_from_span_end(
    span_end: &buck2_data::SpanEndEvent,
    end_time: Option<&Timestamp>,
) -> Option<Timestamp> {
    let duration = span_end.duration.as_ref()?;
    let end_time = end_time?;
    let nanos = i128::from(end_time.seconds) * 1_000_000_000 + i128::from(end_time.nanos);
    let duration_nanos = i128::from(duration.seconds) * 1_000_000_000 + i128::from(duration.nanos);
    let start_nanos = nanos.checked_sub(duration_nanos)?;
    Some(Timestamp {
        seconds: (start_nanos / 1_000_000_000) as i64,
        nanos: (start_nanos % 1_000_000_000) as i32,
    })
}

fn file_with_contents(name: &str, contents: String) -> bep::File {
    let mut contents = contents.into_bytes();
    if contents.len() > MAX_INLINE_FILE_BYTES {
        contents.truncate(MAX_INLINE_FILE_BYTES);
        contents.extend_from_slice(b"\n<<truncated>>");
    }
    let length = contents.len() as i64;
    bep::File {
        name: name.to_owned(),
        path_prefix: Vec::new(),
        file: Some(bep::file::File::Contents(contents)),
        digest: String::new(),
        length,
    }
}

fn file_with_bytes(name: &str, contents: Vec<u8>) -> bep::File {
    let length = contents.len() as i64;
    bep::File {
        name: name.to_owned(),
        path_prefix: Vec::new(),
        file: Some(bep::file::File::Contents(contents)),
        digest: String::new(),
        length,
    }
}

fn test_status(status: i32) -> i32 {
    match buck2_data::TestStatus::try_from(status) {
        Ok(buck2_data::TestStatus::Pass) | Ok(buck2_data::TestStatus::ListingSuccess) => {
            bep::TestStatus::Passed as i32
        }
        Ok(buck2_data::TestStatus::Fail) | Ok(buck2_data::TestStatus::Fatal) => {
            bep::TestStatus::Failed as i32
        }
        Ok(buck2_data::TestStatus::Timeout) => bep::TestStatus::Timeout as i32,
        Ok(buck2_data::TestStatus::InfraFailure) => bep::TestStatus::RemoteFailure as i32,
        Ok(buck2_data::TestStatus::ListingFailed) => bep::TestStatus::FailedToBuild as i32,
        Ok(buck2_data::TestStatus::Rerun) => bep::TestStatus::Flaky as i32,
        Ok(buck2_data::TestStatus::Skip) | Ok(buck2_data::TestStatus::Omitted) => {
            bep::TestStatus::Incomplete as i32
        }
        Ok(buck2_data::TestStatus::Unknown)
        | Ok(buck2_data::TestStatus::NotSetTestStatus)
        | Err(_) => bep::TestStatus::NoStatus as i32,
    }
}

fn is_failing_test_status(status: i32) -> bool {
    status == bep::TestStatus::Failed as i32
        || status == bep::TestStatus::Timeout as i32
        || status == bep::TestStatus::RemoteFailure as i32
        || status == bep::TestStatus::FailedToBuild as i32
}

fn test_status_from_command_report(command: Option<&buck2_data::CommandExecution>) -> i32 {
    match command.and_then(|command| command.status.as_ref()) {
        Some(buck2_data::command_execution::Status::Success(_)) => bep::TestStatus::Passed as i32,
        Some(buck2_data::command_execution::Status::Timeout(_)) => bep::TestStatus::Timeout as i32,
        Some(buck2_data::command_execution::Status::Failure(_))
        | Some(buck2_data::command_execution::Status::WorkerFailure(_)) => {
            bep::TestStatus::Failed as i32
        }
        Some(buck2_data::command_execution::Status::Error(_)) => {
            bep::TestStatus::RemoteFailure as i32
        }
        Some(buck2_data::command_execution::Status::Cancelled(_)) => {
            bep::TestStatus::Incomplete as i32
        }
        None => bep::TestStatus::NoStatus as i32,
    }
}

fn aggregate_test_status(
    command: Option<&buck2_data::CommandExecution>,
    cases: &[TestCaseState],
) -> i32 {
    let command_status = test_status_from_command_report(command);
    if command_status != bep::TestStatus::NoStatus as i32 {
        return command_status;
    }
    if cases.is_empty() {
        return bep::TestStatus::NoStatus as i32;
    }
    if cases
        .iter()
        .any(|case| is_failing_test_status(test_status(case.status)))
    {
        return bep::TestStatus::Failed as i32;
    }
    if cases
        .iter()
        .all(|case| test_status(case.status) == bep::TestStatus::Passed as i32)
    {
        return bep::TestStatus::Passed as i32;
    }
    bep::TestStatus::NoStatus as i32
}

fn test_action_outputs(
    cases: &[TestCaseState],
    command: Option<&buck2_data::CommandExecution>,
    status: i32,
) -> Vec<bep::File> {
    let mut outputs = Vec::new();
    let log = test_log_contents(cases, command);
    if !log.is_empty() {
        outputs.push(file_with_contents("test.log", log));
    }
    let xml = test_xml_contents(cases, status);
    if !xml.is_empty() {
        outputs.push(file_with_contents("test.xml", xml));
    }
    outputs
}

fn test_summary_files(
    status: i32,
    cases: &[TestCaseState],
    command: Option<&buck2_data::CommandExecution>,
    passed: bool,
) -> Vec<bep::File> {
    let is_passed =
        status == bep::TestStatus::Passed as i32 || status == bep::TestStatus::Flaky as i32;
    if is_passed != passed {
        return Vec::new();
    }
    let log = test_log_contents(cases, command);
    if log.is_empty() {
        Vec::new()
    } else {
        vec![file_with_contents("test.log", log)]
    }
}

fn test_log_contents(
    cases: &[TestCaseState],
    command: Option<&buck2_data::CommandExecution>,
) -> String {
    let mut log = String::new();
    if let Some(details) = command.and_then(|command| command.details.as_ref()) {
        if !details.cmd_stdout.is_empty() {
            log.push_str("stdout:\n");
            log.push_str(&details.cmd_stdout);
            if !details.cmd_stdout.ends_with('\n') {
                log.push('\n');
            }
        }
        if !details.cmd_stderr.is_empty() {
            log.push_str("stderr:\n");
            log.push_str(&details.cmd_stderr);
            if !details.cmd_stderr.ends_with('\n') {
                log.push('\n');
            }
        }
    }
    for case in cases {
        let details = case
            .message
            .as_deref()
            .filter(|message| !message.is_empty())
            .or_else(|| (!case.details.is_empty()).then_some(case.details.as_str()));
        if let Some(details) = details {
            log.push_str(&case.name);
            log.push_str(": ");
            log.push_str(details);
            if !details.ends_with('\n') {
                log.push('\n');
            }
        }
    }
    log
}

fn test_xml_contents(cases: &[TestCaseState], status: i32) -> String {
    if cases.is_empty() {
        return String::new();
    }
    let failures = cases
        .iter()
        .filter(|case| test_status(case.status) != bep::TestStatus::Passed as i32)
        .count();
    let mut xml = format!(
        r#"<testsuite tests="{}" failures="{}" errors="0" skipped="{}">"#,
        cases.len(),
        failures,
        if status == bep::TestStatus::Incomplete as i32 {
            failures
        } else {
            0
        }
    );
    for case in cases {
        xml.push_str(&format!(
            r#"<testcase name="{}" time="{}">"#,
            xml_escape(&case.name),
            duration_seconds(case.duration.as_ref())
        ));
        if test_status(case.status) != bep::TestStatus::Passed as i32 {
            let message = case.message.as_deref().unwrap_or(&case.details);
            xml.push_str(&format!(
                r#"<failure message="{}">{}</failure>"#,
                xml_escape(message),
                xml_escape(&case.details)
            ));
        }
        xml.push_str("</testcase>");
    }
    xml.push_str("</testsuite>");
    xml
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
}

fn duration_seconds(duration: Option<&prost_types::Duration>) -> String {
    let Some(duration) = duration else {
        return "0".to_owned();
    };
    let seconds = duration.seconds as f64 + f64::from(duration.nanos) / 1_000_000_000.0;
    format!("{seconds:.3}")
}

fn test_status_details(
    cases: &[TestCaseState],
    command: Option<&buck2_data::CommandExecution>,
) -> String {
    let failed_cases = cases
        .iter()
        .filter(|case| test_status(case.status) != bep::TestStatus::Passed as i32)
        .map(|case| case.name.clone())
        .collect::<Vec<_>>();
    if !failed_cases.is_empty() {
        return format!("failed tests: {}", failed_cases.join(", "));
    }
    command
        .and_then(|command| command.details.as_ref())
        .map(|details| details.cmd_stderr.clone())
        .filter(|stderr| !stderr.is_empty())
        .unwrap_or_default()
}

fn test_duration(command: Option<&buck2_data::CommandExecution>) -> Option<prost_types::Duration> {
    command
        .and_then(|command| command.details.as_ref())
        .and_then(|details| details.metadata.as_ref())
        .and_then(|metadata| metadata.wall_time.clone())
}

fn test_execution_info(
    command: Option<&buck2_data::CommandExecution>,
) -> Option<bep::test_result::ExecutionInfo> {
    let details = command.and_then(|command| command.details.as_ref())?;
    Some(bep::test_result::ExecutionInfo {
        timeout_seconds: 0,
        strategy: details
            .command_kind
            .as_ref()
            .and_then(|kind| kind.command.as_ref())
            .map(test_strategy)
            .unwrap_or_default(),
        cached_remotely: test_cached_remotely(command),
        exit_code: details.signed_exit_code.unwrap_or_default(),
        hostname: String::new(),
        timing_breakdown: test_timing_breakdown(command),
        resource_usage: test_resource_usage(command),
    })
}

fn test_resource_usage(
    command: Option<&buck2_data::CommandExecution>,
) -> Vec<bep::test_result::execution_info::ResourceUsage> {
    let Some(stats) = command
        .and_then(|command| command.details.as_ref())
        .and_then(|details| details.metadata.as_ref())
        .and_then(|metadata| metadata.execution_stats.as_ref())
    else {
        return Vec::new();
    };

    let mut usage = Vec::new();
    push_test_resource_usage(&mut usage, "memory_peak_bytes", stats.memory_peak);
    push_test_resource_usage(
        &mut usage,
        "cpu_instructions_user",
        stats.cpu_instructions_user,
    );
    push_test_resource_usage(
        &mut usage,
        "cpu_instructions_kernel",
        stats.cpu_instructions_kernel,
    );
    usage
}

fn push_test_resource_usage(
    usage: &mut Vec<bep::test_result::execution_info::ResourceUsage>,
    name: &str,
    value: Option<u64>,
) {
    let Some(value) = value.and_then(|value| i64::try_from(value).ok()) else {
        return;
    };
    usage.push(bep::test_result::execution_info::ResourceUsage {
        name: name.to_owned(),
        value,
    });
}

fn test_timing_breakdown(
    command: Option<&buck2_data::CommandExecution>,
) -> Option<bep::test_result::execution_info::TimingBreakdown> {
    let metadata = command
        .and_then(|command| command.details.as_ref())
        .and_then(|details| details.metadata.as_ref())?;
    let total = metadata
        .wall_time
        .as_ref()
        .or(metadata.execution_time.as_ref())?;
    let mut child = Vec::new();
    push_test_timing_child_or_zero(&mut child, "parseTime", None);
    push_test_timing_child_or_zero(&mut child, "fetchTime", None);
    push_test_timing_child_or_zero(&mut child, "queueTime", metadata.queue_duration.as_ref());
    push_test_timing_child_or_zero(&mut child, "uploadTime", None);
    push_test_timing_child_or_zero(&mut child, "setupTime", None);
    push_test_timing_child_or_zero(
        &mut child,
        "executionWallTime",
        metadata.execution_time.as_ref(),
    );
    push_test_timing_child_or_zero(&mut child, "processOutputsTime", None);
    push_test_timing_child_or_zero(&mut child, "networkTime", None);
    push_test_timing_child(
        &mut child,
        "inputMaterializationTime",
        metadata.input_materialization_duration.as_ref(),
    );
    push_test_timing_child(
        &mut child,
        "hashingTime",
        metadata.hashing_duration.as_ref(),
    );
    push_test_timing_child(
        &mut child,
        "suspendTime",
        metadata.suspend_duration.as_ref(),
    );
    Some(test_timing_node("totalTime", total, child))
}

fn push_test_timing_child_or_zero(
    child: &mut Vec<bep::test_result::execution_info::TimingBreakdown>,
    name: &str,
    duration: Option<&prost_types::Duration>,
) {
    let duration = duration.cloned().unwrap_or_default();
    child.push(test_timing_node(name, &duration, Vec::new()));
}

fn push_test_timing_child(
    child: &mut Vec<bep::test_result::execution_info::TimingBreakdown>,
    name: &str,
    duration: Option<&prost_types::Duration>,
) {
    if let Some(duration) = duration {
        child.push(test_timing_node(name, duration, Vec::new()));
    }
}

fn test_timing_node(
    name: &str,
    duration: &prost_types::Duration,
    child: Vec<bep::test_result::execution_info::TimingBreakdown>,
) -> bep::test_result::execution_info::TimingBreakdown {
    bep::test_result::execution_info::TimingBreakdown {
        child,
        name: name.to_owned(),
        time_millis: duration_millis(duration),
        time: Some(duration.clone()),
    }
}

fn test_strategy(command: &buck2_data::command_execution_kind::Command) -> String {
    match command {
        buck2_data::command_execution_kind::Command::LocalCommand(_) => "local",
        buck2_data::command_execution_kind::Command::WorkerCommand(_)
        | buck2_data::command_execution_kind::Command::WorkerInitCommand(_) => "worker",
        buck2_data::command_execution_kind::Command::RemoteCommand(_) => "remote",
        buck2_data::command_execution_kind::Command::OmittedLocalCommand(_) => "local",
    }
    .to_owned()
}

fn test_cached_remotely(command: Option<&buck2_data::CommandExecution>) -> bool {
    matches!(
        command
            .and_then(|command| command.details.as_ref())
            .and_then(|details| details.command_kind.as_ref())
            .and_then(|kind| kind.command.as_ref()),
        Some(buck2_data::command_execution_kind::Command::RemoteCommand(remote))
            if remote.cache_hit
    )
}

fn action_error_message(action_error: &buck2_data::ActionError) -> String {
    let label = action_error
        .key
        .as_ref()
        .and_then(action_owner)
        .and_then(label_for_configured_target)
        .unwrap_or_else(|| "unknown target".to_owned());
    let category = action_error
        .name
        .as_ref()
        .map(|name| name.category.as_str())
        .filter(|category| !category.is_empty())
        .unwrap_or("Action");
    format!("Action failed: {category} for {label}")
}

fn first_metadata(metadata: &HashMap<String, String>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        metadata
            .get(*key)
            .filter(|value| !value.is_empty())
            .cloned()
    })
}

#[cfg(test)]
mod tests {

    #[test]
    fn interrupted_finish_event_is_a_final_interrupted_build_finished() {
        let event = interrupted_finish_event(None);
        assert!(event.last_message);
        assert!(matches!(
            event.id.as_ref().and_then(|id| id.id.as_ref()),
            Some(build_event_id::Id::BuildFinished(_))
        ));
        match event.payload {
            Some(build_event::Payload::Finished(finished)) => {
                assert!(!finished.overall_success);
                let exit = finished.exit_code.expect("exit code");
                assert_eq!((exit.name.as_str(), exit.code), ("INTERRUPTED", 8));
            }
            other => panic!("unexpected payload {other:?}"),
        }
    }
    use std::io::Read;

    use flate2::read::GzDecoder;
    use prost::Message as _;

    use super::*;

    fn trace_event(data: buck2_data::buck_event::Data) -> buck2_data::BuckEvent {
        buck2_data::BuckEvent {
            timestamp: Some(Timestamp {
                seconds: 1,
                nanos: 0,
            }),
            trace_id: "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa".to_owned(),
            span_id: 1,
            parent_id: 0,
            data: Some(data),
        }
    }

    fn trace_event_at(
        span_id: u64,
        parent_id: u64,
        micros: i64,
        data: buck2_data::buck_event::Data,
    ) -> buck2_data::BuckEvent {
        buck2_data::BuckEvent {
            timestamp: Some(Timestamp {
                seconds: 1 + micros / 1_000_000,
                nanos: ((micros % 1_000_000) * 1_000) as i32,
            }),
            trace_id: "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa".to_owned(),
            span_id,
            parent_id,
            data: Some(data),
        }
    }

    fn decode_profile_from_logs(logs: &bep::BuildToolLogs) -> serde_json::Value {
        let profile_contents = logs
            .log
            .iter()
            .find(|log| log.name == COMMAND_PROFILE_NAME)
            .and_then(|log| log.file.as_ref())
            .unwrap_or_else(|| panic!("expected {COMMAND_PROFILE_NAME}"));
        let profile_contents = match profile_contents {
            bep::file::File::Contents(contents) => contents,
            other => panic!("expected inline command profile, got {other:?}"),
        };
        let mut decoder = GzDecoder::new(profile_contents.as_slice());
        let mut profile = String::new();
        decoder
            .read_to_string(&mut profile)
            .expect("valid gzip profile");
        assert!(profile.contains("\"traceEvents\":[\n"));
        serde_json::from_str(&profile).expect("valid profile json")
    }

    fn progress_payload(events: &[bep::BuildEvent]) -> &bep::Progress {
        events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::Progress(progress)) => Some(progress),
                _ => None,
            })
            .expect("progress event")
    }

    fn progress_payloads(events: &[bep::BuildEvent]) -> Vec<&bep::Progress> {
        events
            .iter()
            .filter_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::Progress(progress)) => Some(progress),
                _ => None,
            })
            .collect()
    }

    fn configured_target() -> buck2_data::ConfiguredTargetLabel {
        configured_target_with_package("pkg", "main", "cfg")
    }

    fn configured_target_with_package(
        package: &str,
        name: &str,
        configuration: &str,
    ) -> buck2_data::ConfiguredTargetLabel {
        buck2_data::ConfiguredTargetLabel {
            label: Some(buck2_data::TargetLabel {
                package: package.to_owned(),
                name: name.to_owned(),
            }),
            configuration: Some(buck2_data::Configuration {
                full_name: configuration.to_owned(),
            }),
            execution_configuration: None,
        }
    }

    fn struct_string_field<'a>(details: &'a Struct, field: &str) -> Option<&'a str> {
        match details.fields.get(field)?.kind.as_ref()? {
            value::Kind::StringValue(value) => Some(value.as_str()),
            _ => None,
        }
    }

    fn struct_number_field(details: &Struct, field: &str) -> Option<f64> {
        match details.fields.get(field)?.kind.as_ref()? {
            value::Kind::NumberValue(value) => Some(*value),
            _ => None,
        }
    }

    fn command_line_from_event(event: &bep::BuildEvent) -> &cl::CommandLine {
        match event.payload.as_ref() {
            Some(build_event::Payload::StructuredCommandLine(command_line)) => command_line,
            _ => panic!("expected structured command line"),
        }
    }

    fn chunk_values(command_line: &cl::CommandLine, section_label: &str) -> Vec<String> {
        command_line
            .sections
            .iter()
            .find_map(|section| {
                if section.section_label != section_label {
                    return None;
                }
                match section.section_type.as_ref() {
                    Some(cl::command_line_section::SectionType::ChunkList(chunks)) => {
                        Some(chunks.chunk.clone())
                    }
                    _ => None,
                }
            })
            .unwrap_or_default()
    }

    fn option_values(command_line: &cl::CommandLine, section_label: &str) -> Vec<String> {
        command_line
            .sections
            .iter()
            .find_map(|section| {
                if section.section_label != section_label {
                    return None;
                }
                match section.section_type.as_ref() {
                    Some(cl::command_line_section::SectionType::OptionList(options)) => Some(
                        options
                            .option
                            .iter()
                            .map(|option| option.combined_form.clone())
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                }
            })
            .unwrap_or_default()
    }

    #[test]
    fn command_start_emits_started_and_metadata() {
        let mut converter = BazelEventConverter::default();
        let event = trace_event(buck2_data::buck_event::Data::SpanStart(
            buck2_data::SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::Command(
                    buck2_data::CommandStart {
                        cli_args: vec![
                            "buck2".to_owned(),
                            "build".to_owned(),
                            "--remote-cache=grpc://cache.example.com".to_owned(),
                            "//:main".to_owned(),
                        ],
                        metadata: HashMap::from([
                            ("username".to_owned(), "alice".to_owned()),
                            ("hostname".to_owned(), "workstation".to_owned()),
                        ]),
                        data: Some(buck2_data::command_start::Data::Build(
                            buck2_data::BuildCommandStart {},
                        )),
                        ..Default::default()
                    },
                )),
            },
        ));

        let events = converter.convert(1, &event);
        let Some(build_event::Payload::Started(started)) = events[0].payload.as_ref() else {
            panic!("expected build started event");
        };
        assert_eq!(started.start_time_millis, 1_000);
        assert_eq!(started.server_pid, i64::from(std::process::id()));
        assert_eq!(started.host, "workstation");
        assert_eq!(started.user, "alice");
        assert!(
            events[0]
                .children
                .iter()
                .any(|child| child == &progress_id(0))
        );
        assert!(
            events[0]
                .children
                .iter()
                .any(|child| { child == &pattern_expanded_id(vec!["//:main".to_owned()]) })
        );
        assert!(
            events[0]
                .children
                .iter()
                .any(|child| { child == &structured_command_line_id(TOOL_COMMAND_LINE_LABEL) })
        );
        assert!(
            events[0]
                .children
                .iter()
                .any(|child| { child == &convenience_symlinks_identified_id() })
        );
        assert!(events.iter().any(|event| matches!(
            event.payload,
            Some(build_event::Payload::StructuredCommandLine(_))
        )));
        let tool = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::StructuredCommandLine(command_line))
                    if event.id.as_ref()
                        == Some(&structured_command_line_id(TOOL_COMMAND_LINE_LABEL)) =>
                {
                    Some(command_line)
                }
                _ => None,
            })
            .expect("tool structured command line");
        assert!(tool.command_line_label.is_empty());
        assert!(tool.sections.is_empty());
        let canonical = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::StructuredCommandLine(command_line))
                    if command_line.command_line_label == CANONICAL_COMMAND_LINE_LABEL =>
                {
                    Some(command_line)
                }
                _ => None,
            })
            .expect("canonical structured command line");
        let options = canonical
            .sections
            .iter()
            .flat_map(|section| match section.section_type.as_ref() {
                Some(cl::command_line_section::SectionType::OptionList(options)) => {
                    options.option.as_slice()
                }
                _ => &[],
            })
            .collect::<Vec<_>>();
        assert!(options.iter().any(|option| {
            option.option_name == "remote_cache"
                && option.option_value == "grpc://cache.example.com"
        }));
        assert!(options.iter().any(|option| {
            option.option_name == "client_env" && option.option_value == "USER=alice"
        }));
        assert!(
            events
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::OptionsParsed(_))))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::Configuration(_))))
        );
        let build_metadata = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildMetadata(metadata)) => Some(metadata),
                _ => None,
            })
            .expect("build metadata");
        assert_eq!(
            build_metadata
                .metadata
                .get(BUILDBUDDY_VISIBILITY_KEY)
                .map(String::as_str),
            Some(BUILDBUDDY_PUBLIC_VISIBILITY)
        );
        assert_eq!(
            build_metadata.metadata.get("USER").map(String::as_str),
            Some("alice")
        );
        assert_eq!(
            build_metadata.metadata.get("HOST").map(String::as_str),
            Some("workstation")
        );
        let workspace_status = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::WorkspaceStatus(status)) => Some(status),
                _ => None,
            })
            .expect("workspace status");
        let workspace_item = |key: &str| {
            workspace_status
                .item
                .iter()
                .find(|item| item.key == key)
                .map(|item| item.value.as_str())
        };
        assert_eq!(workspace_item("BUILD_TIMESTAMP"), Some("1"));
        assert_eq!(
            workspace_item("FORMATTED_DATE"),
            Some("1970 Jan 01 00 00 01 Thu")
        );

        let any = encode_bep_event(&events[0]);
        assert_eq!(any.type_url, BEP_EVENT_TYPE_URL);
        let decoded = bep::BuildEvent::decode(any.value.as_slice()).expect("valid BEP event");
        assert!(matches!(
            decoded.payload,
            Some(build_event::Payload::Started(_))
        ));
    }

    #[test]
    fn pre_command_startup_events_wait_for_command_context() {
        let mut converter = BazelEventConverter::default();
        let startup_events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::StructuredError(
                        buck2_data::StructuredError {
                            payload: "early soft error".to_owned(),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );
        assert!(startup_events.is_empty());

        let command_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "build".to_owned(),
                                "//:main".to_owned(),
                            ],
                            data: Some(buck2_data::command_start::Data::Build(
                                buck2_data::BuildCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );

        let started = command_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::Started(started)) => Some(started),
                _ => None,
            })
            .expect("started event");
        assert_eq!(started.command, "build");
        assert!(command_events.iter().any(|event| matches!(
            event.payload.as_ref(),
            Some(build_event::Payload::Progress(progress))
                if progress.stderr == "early soft error"
        )));
        let original = command_events
            .iter()
            .find(|event| {
                event.id.as_ref() == Some(&structured_command_line_id(ORIGINAL_COMMAND_LINE_LABEL))
            })
            .map(command_line_from_event)
            .expect("original command line");
        assert_eq!(chunk_values(original, "command"), vec!["build"]);
    }

    #[test]
    fn console_message_progress_uses_stderr() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::ConsoleMessage(
                        buck2_data::ConsoleMessage {
                            message: "stderr chunk".to_owned(),
                        },
                    )),
                },
            )),
        );

        let progress = progress_payload(&events);
        assert_eq!(progress.stdout, "");
        assert_eq!(progress.stderr, "stderr chunk");
    }

    #[test]
    fn streaming_output_progress_uses_stdout() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::StreamingOutput(
                        buck2_data::StdoutStreamingOutput {
                            message: "stdout chunk".to_owned(),
                        },
                    )),
                },
            )),
        );

        let progress = progress_payload(&events);
        assert_eq!(progress.stdout, "stdout chunk");
        assert_eq!(progress.stderr, "");
    }

    #[test]
    fn normal_build_progress_uses_stderr() {
        let mut converter = BazelEventConverter::default();
        let mut events = Vec::new();

        events.extend(converter.convert(
            1,
            &trace_event_at(
                1,
                0,
                0,
                buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "build".to_owned(),
                                "//:main".to_owned(),
                            ],
                            data: Some(buck2_data::command_start::Data::Build(
                                buck2_data::BuildCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                }),
            ),
        ));
        events.extend(converter.convert(
            2,
            &trace_event_at(
                2,
                1,
                1_000,
                buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Load(
                        buck2_data::LoadBuildFileStart {
                            module_id: "root//pkg:BUCK".to_owned(),
                            cell: "root".to_owned(),
                        },
                    )),
                }),
            ),
        ));
        events.extend(converter.convert(
            3,
            &trace_event_at(
                2,
                1,
                2_000,
                buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Load(
                        buck2_data::LoadBuildFileEnd {
                            module_id: "root//pkg:BUCK".to_owned(),
                            cell: "root".to_owned(),
                            target_count: Some(3),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                }),
            ),
        ));
        events.extend(converter.convert(
            4,
            &trace_event_at(
                3,
                1,
                3_000,
                buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Analysis(
                        buck2_data::AnalysisStart {
                            target: Some(buck2_data::analysis_start::Target::StandardTarget(
                                configured_target(),
                            )),
                            rule: "genrule".to_owned(),
                        },
                    )),
                }),
            ),
        ));
        events.extend(converter.convert(
            5,
            &trace_event_at(
                3,
                1,
                4_000,
                buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Analysis(
                        buck2_data::AnalysisEnd {
                            target: Some(buck2_data::analysis_end::Target::StandardTarget(
                                configured_target(),
                            )),
                            rule: "genrule".to_owned(),
                            declared_actions: Some(2),
                            declared_artifacts: Some(4),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                }),
            ),
        ));
        events.extend(converter.convert(
            6,
            &trace_event_at(
                4,
                1,
                5_000,
                buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::ActionExecution(
                        buck2_data::ActionExecutionStart {
                            kind: buck2_data::ActionKind::Run as i32,
                            name: Some(buck2_data::ActionName {
                                category: "genrule".to_owned(),
                                identifier: "main".to_owned(),
                            }),
                            ..Default::default()
                        },
                    )),
                }),
            ),
        ));
        events.extend(converter.convert(
            7,
            &trace_event_at(
                4,
                1,
                6_000,
                buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                        buck2_data::ActionExecutionEnd {
                            kind: buck2_data::ActionKind::Run as i32,
                            name: Some(buck2_data::ActionName {
                                category: "genrule".to_owned(),
                                identifier: "main".to_owned(),
                            }),
                            execution_kind: buck2_data::ActionExecutionKind::Local as i32,
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                }),
            ),
        ));

        let progress = progress_payloads(&events);
        assert!(
            progress
                .iter()
                .any(|progress| progress.stderr.contains("Command: build."))
        );
        assert!(progress.iter().any(|progress| {
            progress
                .stderr
                .contains("Build ID:  aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa")
        }));
        assert!(
            progress
                .iter()
                .any(|progress| progress.stderr.contains("Time elapsed: 0.0s"))
        );
        assert!(progress.iter().any(|progress| {
            progress
                .stderr
                .contains("Loading targets.     Remaining     1/1")
        }));
        assert!(progress.iter().any(|progress| {
            progress
                .stderr
                .contains("Analyzing targets.   Remaining     1/1")
        }));
        assert!(progress.iter().any(|progress| {
            progress
                .stderr
                .contains("Executing actions.   Remaining     1/1")
        }));
        assert!(
            progress
                .iter()
                .any(|progress| { progress.stderr.contains("2 actions, 4 artifacts declared") })
        );
        assert!(
            progress
                .iter()
                .any(|progress| progress.stderr.contains("Finished 1 local"))
        );
        assert!(progress.iter().all(|progress| progress.stdout.is_empty()));
        assert!(progress.iter().skip(1).any(|progress| {
            progress.stderr.starts_with("[")
                && progress.stderr.contains(BES_PROGRESS_CLEAR_FROM_CURSOR)
        }));
    }

    #[test]
    fn progress_includes_network_session_and_snapshot_bytes() {
        let mut converter = BazelEventConverter::default();
        let mut events = Vec::new();

        events.extend(converter.convert(
            1,
            &trace_event_at(
                1,
                0,
                0,
                buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            data: Some(buck2_data::command_start::Data::Build(
                                buck2_data::BuildCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                }),
            ),
        ));
        events.extend(converter.convert(
            2,
            &trace_event_at(
                1,
                0,
                1_000,
                buck2_data::buck_event::Data::Instant(buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::ReSession(
                        buck2_data::RemoteExecutionSessionCreated {
                            session_id: "reSessionID-123".to_owned(),
                            ..Default::default()
                        },
                    )),
                }),
            ),
        ));
        events.extend(converter.convert(
            3,
            &trace_event_at(
                1,
                0,
                1_000_000,
                buck2_data::buck_event::Data::Instant(buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::Snapshot(Box::new(
                        buck2_data::Snapshot {
                            re_upload_bytes: 1_024,
                            re_download_bytes: 2_048,
                            ..Default::default()
                        },
                    ))),
                }),
            ),
        ));
        events.extend(converter.convert(
            4,
            &trace_event_at(
                1,
                0,
                2_100_000,
                buck2_data::buck_event::Data::Instant(buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::Snapshot(Box::new(
                        buck2_data::Snapshot {
                            re_upload_bytes: 1_024 + 18 * 1024 * 1024,
                            re_download_bytes: 2_048 + 77 * 1024 * 1024,
                            ..Default::default()
                        },
                    ))),
                }),
            ),
        ));

        let rendered = progress_payloads(&events)
            .iter()
            .map(|progress| progress.stderr.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Network: Up:"));
        assert!(rendered.contains("18MiB"));
        assert!(rendered.contains("Down:"));
        assert!(rendered.contains("77MiB"));
        assert!(rendered.contains("(reSessionID-123)"));
    }

    /// A build of two targets, one whose action succeeds and is then tested and one whose
    /// action fails, converted with the given `[bes] upload_successful_action_events`.
    fn convert_build_with_successful_and_failed_actions(
        upload_successful_action_events: bool,
    ) -> Vec<bep::BuildEvent> {
        let mut converter = BazelEventConverter::new_with_options(
            std::iter::empty::<(String, String)>(),
            upload_successful_action_events,
        );
        let passing = configured_target_with_package("pkg", "passing", "cfg");
        let failing = configured_target_with_package("pkg", "failing", "cfg");
        let mut events = Vec::new();

        events.extend(converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            data: Some(buck2_data::command_start::Data::Test(
                                buck2_data::TestCommandStart::default(),
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        ));
        let mut sequence = 2;
        for target in [&passing, &failing] {
            events.extend(converter.convert(
                sequence,
                &trace_event(buck2_data::buck_event::Data::SpanStart(
                    buck2_data::SpanStartEvent {
                        data: Some(buck2_data::span_start_event::Data::Analysis(
                            buck2_data::AnalysisStart {
                                rule: "cxx_binary".to_owned(),
                                target: Some(buck2_data::analysis_start::Target::StandardTarget(
                                    target.clone(),
                                )),
                            },
                        )),
                    },
                )),
            ));
            events.extend(converter.convert(
                sequence + 1,
                &trace_event(buck2_data::buck_event::Data::SpanEnd(
                    buck2_data::SpanEndEvent {
                        data: Some(buck2_data::span_end_event::Data::Analysis(
                            buck2_data::AnalysisEnd {
                                rule: "cxx_binary".to_owned(),
                                target: Some(buck2_data::analysis_end::Target::StandardTarget(
                                    target.clone(),
                                )),
                                ..Default::default()
                            },
                        )),
                        ..Default::default()
                    },
                )),
            ));
            sequence += 2;
        }
        for (target, failed, stdout, stderr, exit_code) in [
            (&passing, false, "linker stdout", "linker stderr", 0),
            (&failing, true, "compiler stdout", "compile failed", 1),
        ] {
            events.extend(converter.convert(
                sequence,
                &trace_event(buck2_data::buck_event::Data::SpanEnd(
                    buck2_data::SpanEndEvent {
                        data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                            buck2_data::ActionExecutionEnd {
                                key: Some(buck2_data::ActionKey {
                                    key: "action-key".to_owned(),
                                    owner: Some(buck2_data::action_key::Owner::TargetLabel(
                                        target.clone(),
                                    )),
                                    ..Default::default()
                                }),
                                kind: buck2_data::ActionKind::Run as i32,
                                name: Some(buck2_data::ActionName {
                                    category: "cxx_link".to_owned(),
                                    identifier: "main".to_owned(),
                                }),
                                execution_kind: buck2_data::ActionExecutionKind::Remote as i32,
                                failed,
                                outputs: vec![buck2_data::ActionOutput {
                                    tiny_digest: "buck-out/main".to_owned(),
                                    path: "buck-out/main".to_owned(),
                                    digest: "abcd:10".to_owned(),
                                    size: 10,
                                    is_directory: false,
                                }],
                                commands: vec![buck2_data::CommandExecution {
                                    details: Some(buck2_data::CommandExecutionDetails {
                                        signed_exit_code: Some(exit_code),
                                        cmd_stdout: stdout.to_owned(),
                                        cmd_stderr: stderr.to_owned(),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                }],
                                ..Default::default()
                            },
                        ))),
                        ..Default::default()
                    },
                )),
            ));
            sequence += 1;
        }
        events.extend(converter.convert(
            sequence,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::TestResult(
                        buck2_data::TestResult {
                            name: "test_passes".to_owned(),
                            status: buck2_data::TestStatus::Pass as i32,
                            target_label: Some(passing.clone()),
                            ..Default::default()
                        },
                    )),
                },
            )),
        ));
        events.extend(convert_through_end_of_results(
            &mut converter,
            sequence + 1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::TestRun(Box::new(
                        buck2_data::TestRunEnd {
                            suite: Some(buck2_data::TestSuite {
                                suite_name: "suite".to_owned(),
                                test_names: vec!["test_passes".to_owned()],
                                target_label: Some(passing),
                                labels: Vec::new(),
                            }),
                            command_report: Some(buck2_data::CommandExecution {
                                details: Some(buck2_data::CommandExecutionDetails {
                                    signed_exit_code: Some(0),
                                    ..Default::default()
                                }),
                                status: Some(buck2_data::command_execution::Status::Success(
                                    buck2_data::command_execution::Success {},
                                )),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                },
            )),
        ));
        events.extend(converter.convert(
            sequence + 3,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Failed as i32),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        ));
        events
    }

    fn action_payloads(events: &[bep::BuildEvent]) -> Vec<&bep::ActionExecuted> {
        events
            .iter()
            .filter_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::Action(action)) => Some(action),
                _ => None,
            })
            .collect()
    }

    fn inline_contents(file: Option<&bep::File>) -> &str {
        match file.and_then(|file| file.file.as_ref()) {
            Some(bep::file::File::Contents(contents)) => {
                std::str::from_utf8(contents).expect("utf8 contents")
            }
            other => panic!("expected inline contents, got {other:?}"),
        }
    }

    #[test]
    fn every_action_is_published_by_default() {
        let events = convert_build_with_successful_and_failed_actions(true);
        let actions = action_payloads(&events);
        assert_eq!(actions.len(), 2);
        assert!(actions.iter().any(|action| action.success));
        assert!(actions.iter().any(|action| !action.success));
    }

    #[test]
    fn only_failed_actions_are_published_without_successful_action_events() {
        let all = convert_build_with_successful_and_failed_actions(true);
        let failed_only = convert_build_with_successful_and_failed_actions(false);

        let actions = action_payloads(&failed_only);
        assert_eq!(actions.len(), 1);
        let failed = actions[0];
        assert!(!failed.success);
        assert_eq!(failed.label, "//pkg:failing");
        assert_eq!(failed.exit_code, 1);
        assert_eq!(inline_contents(failed.stdout.as_ref()), "compiler stdout");
        assert_eq!(inline_contents(failed.stderr.as_ref()), "compile failed");
        assert!(failed.failure_detail.is_some());

        // Every other event is the one a full stream carries, less the successful action's
        // ActionExecuted and the TargetComplete children that name it.
        let successful_action_ids = all
            .iter()
            .filter(|event| {
                matches!(
                    event.payload.as_ref(),
                    Some(build_event::Payload::Action(action)) if action.success
                )
            })
            .filter_map(|event| event.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(successful_action_ids.len(), 1);
        let expected = all
            .into_iter()
            .filter(|event| {
                !successful_action_ids
                    .iter()
                    .any(|id| event.id.as_ref() == Some(id))
            })
            .map(|mut event| {
                event
                    .children
                    .retain(|child| !successful_action_ids.contains(child));
                event
            })
            .collect::<Vec<_>>();
        assert_eq!(failed_only, expected);

        for (payload, wanted) in [
            ("TargetConfigured", 2),
            ("NamedSetOfFiles", 2),
            ("TestResult", 1),
            ("TestSummary", 1),
        ] {
            let count = failed_only
                .iter()
                .filter(|event| {
                    matches!(
                        (payload, event.payload.as_ref()),
                        (
                            "TargetConfigured",
                            Some(build_event::Payload::Configured(_))
                        ) | (
                            "NamedSetOfFiles",
                            Some(build_event::Payload::NamedSetOfFiles(_))
                        ) | ("TestResult", Some(build_event::Payload::TestResult(_)))
                            | ("TestSummary", Some(build_event::Payload::TestSummary(_)))
                    )
                })
                .count();
            assert!(
                count >= wanted,
                "{payload}: {count} events, wanted {wanted}"
            );
        }
        let passing_completed = failed_only
            .iter()
            .filter_map(|event| {
                match (
                    event.id.as_ref().and_then(|id| id.id.as_ref()),
                    event.payload.as_ref(),
                ) {
                    (
                        Some(build_event_id::Id::TargetCompleted(id)),
                        Some(build_event::Payload::Completed(completed)),
                    ) if id.label == "//pkg:passing" => Some(completed),
                    _ => None,
                }
            })
            .next_back()
            .expect("TargetComplete for the target whose action succeeded");
        assert!(passing_completed.success);
        assert_eq!(passing_completed.important_output.len(), 1);
        assert_eq!(passing_completed.important_output[0].name, "buck-out/main");
    }

    #[test]
    fn successful_action_upload_filter_keeps_progress() {
        let mut converter =
            BazelEventConverter::new_with_options(std::iter::empty::<(String, String)>(), false);
        let mut events = Vec::new();
        let target = configured_target();
        let key = buck2_data::ActionKey {
            owner: Some(buck2_data::action_key::Owner::TargetLabel(target)),
            key: "action-key".to_owned(),
            ..Default::default()
        };

        events.extend(converter.convert(
            1,
            &trace_event_at(
                1,
                0,
                0,
                buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            data: Some(buck2_data::command_start::Data::Build(
                                buck2_data::BuildCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                }),
            ),
        ));
        events.extend(converter.convert(
            2,
            &trace_event_at(
                2,
                1,
                1_000,
                buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::ActionExecution(
                        buck2_data::ActionExecutionStart {
                            key: Some(key.clone()),
                            kind: buck2_data::ActionKind::Run as i32,
                            name: Some(buck2_data::ActionName {
                                category: "genrule".to_owned(),
                                identifier: "main".to_owned(),
                            }),
                        },
                    )),
                }),
            ),
        ));
        events.extend(converter.convert(
            3,
            &trace_event_at(
                2,
                1,
                2_000,
                buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                        buck2_data::ActionExecutionEnd {
                            key: Some(key),
                            kind: buck2_data::ActionKind::Run as i32,
                            name: Some(buck2_data::ActionName {
                                category: "genrule".to_owned(),
                                identifier: "main".to_owned(),
                            }),
                            execution_kind: buck2_data::ActionExecutionKind::Remote as i32,
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                }),
            ),
        ));

        assert!(
            !events
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::Action(_))))
        );
        assert!(
            progress_payloads(&events)
                .iter()
                .any(|progress| progress.stderr.contains("Finished 1 remote"))
        );
    }

    #[test]
    fn active_action_progress_includes_executor_stage() {
        let mut converter = BazelEventConverter::default();
        let mut events = Vec::new();

        events.extend(converter.convert(
            1,
            &trace_event_at(
                1,
                0,
                0,
                buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            data: Some(buck2_data::command_start::Data::Build(
                                buck2_data::BuildCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                }),
            ),
        ));

        for i in 0u64..12 {
            let target = configured_target_with_package("pkg", &format!("main{i}"), "cfg");
            events.extend(converter.convert(
                (2 + i) as i64,
                &trace_event_at(
                    10 + i,
                    1,
                    1_000 + i as i64,
                    buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                        data: Some(buck2_data::span_start_event::Data::ActionExecution(
                            buck2_data::ActionExecutionStart {
                                key: Some(buck2_data::ActionKey {
                                    key: format!("action-key-{i}"),
                                    owner: Some(buck2_data::action_key::Owner::TargetLabel(target)),
                                    ..Default::default()
                                }),
                                kind: buck2_data::ActionKind::Run as i32,
                                name: Some(buck2_data::ActionName {
                                    category: "rustc".to_owned(),
                                    identifier: "metadata [pic]".to_owned(),
                                }),
                            },
                        )),
                    }),
                ),
            ));
        }

        events.extend(converter.convert(
            20,
            &trace_event_at(
                100,
                10,
                2_100_000,
                buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::ExecutorStage(
                        buck2_data::ExecutorStageStart {
                            stage: Some(buck2_data::executor_stage_start::Stage::Local(
                                buck2_data::LocalStage {
                                    stage: Some(buck2_data::local_stage::Stage::Queued(
                                        buck2_data::LocalQueued {},
                                    )),
                                },
                            )),
                        },
                    )),
                }),
            ),
        ));

        let rendered = progress_payloads(&events)
            .iter()
            .map(|progress| progress.stderr.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let command_index = rendered.find("Command: build.").expect("command line");
        let separator_index = rendered
            .find(BES_PROGRESS_SEPARATOR)
            .expect("active action separator");
        let action_index = rendered
            .find("//pkg:main0 -- action")
            .expect("active action row");
        assert!(command_index < separator_index);
        assert!(separator_index < action_index);
        assert!(rendered.contains("Time elapsed: 2.1s"));
        assert!(
            rendered.contains("//pkg:main0 -- action (rustc metadata [pic]) [local_queued] 2.1s")
        );
        assert!(rendered.contains("... and 2 more currently executing"));
    }

    #[test]
    fn command_line_events_render_as_buck2_in_buildbuddy() {
        let command = buck2_data::CommandStart {
            cli_args: vec![
                "/root/workspace/repo-root/buck-out/buildbuddy-rbe-build/art/gh_facebook_buck2/7703741d4b7244c6/app/buck2/__buck2-bin__/buck2".to_owned(),
                "--isolation-dir".to_owned(),
                "buildbuddy-rbe-selftest".to_owned(),
                "build".to_owned(),
                "--config-file".to_owned(),
                ".buckconfig.local".to_owned(),
                "--remote-only".to_owned(),
                "//:buck2".to_owned(),
            ],
            data: Some(buck2_data::command_start::Data::Build(
                buck2_data::BuildCommandStart {},
            )),
            ..Default::default()
        };

        let original_event = original_structured_command_line_event(&command);
        let original = command_line_from_event(&original_event);
        assert_eq!(chunk_values(original, "executable"), vec!["buck2"]);
        assert_eq!(chunk_values(original, "command"), vec!["build"]);
        assert_eq!(
            option_values(original, "command options"),
            vec![
                "--isolation-dir=buildbuddy-rbe-selftest",
                "--config-file=.buckconfig.local",
                "--remote-only",
            ]
        );
        assert_eq!(chunk_values(original, "residual"), vec!["//:buck2"]);

        let canonical_event = canonical_structured_command_line_event(&command);
        let canonical = command_line_from_event(&canonical_event);
        let canonical_options = option_values(canonical, "command options");
        assert!(canonical_options.contains(&"--remote-only".to_owned()));
        assert!(!canonical_options.contains(&"--remote-only=//:buck2".to_owned()));
        assert_eq!(chunk_values(canonical, "residue"), vec!["//:buck2"]);

        let options_parsed_event = options_parsed_event(&command);
        let Some(build_event::Payload::OptionsParsed(options_parsed)) =
            options_parsed_event.payload.as_ref()
        else {
            panic!("expected options parsed");
        };
        assert_eq!(
            options_parsed.explicit_cmd_line,
            vec![
                "--isolation-dir=buildbuddy-rbe-selftest",
                "--config-file=.buckconfig.local",
                "--remote-only",
            ]
        );
    }

    #[test]
    fn command_line_events_infer_command_from_argv() {
        let command = buck2_data::CommandStart {
            cli_args: vec![
                "/root/workspace/repo-root/buck-out/buildbuddy-rbe-build/art/gh_facebook_buck2/7703741d4b7244c6/app/buck2/__buck2-bin__/buck2".to_owned(),
                "--isolation-dir".to_owned(),
                "buildbuddy-bes-compare".to_owned(),
                "build".to_owned(),
                "--config-file".to_owned(),
                ".buckconfig.local".to_owned(),
                "--prefer-remote".to_owned(),
                "--".to_owned(),
                "//:main".to_owned(),
            ],
            data: None,
            ..Default::default()
        };

        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(command.clone())),
                },
            )),
        );

        let started = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::Started(started)) => Some(started),
                _ => None,
            })
            .expect("started event");
        assert_eq!(started.command, "build");
        assert_eq!(command_name(&command), "build");

        let original_event = original_structured_command_line_event(&command);
        let original = command_line_from_event(&original_event);
        assert_eq!(chunk_values(original, "executable"), vec!["buck2"]);
        assert_eq!(chunk_values(original, "command"), vec!["build"]);
        assert_eq!(
            option_values(original, "command options"),
            vec![
                "--isolation-dir=buildbuddy-bes-compare",
                "--config-file=.buckconfig.local",
                "--prefer-remote",
            ]
        );
        assert_eq!(chunk_values(original, "residual"), vec!["//:main"]);

        let options_parsed_event = options_parsed_event(&command);
        let Some(build_event::Payload::OptionsParsed(options_parsed)) =
            options_parsed_event.payload.as_ref()
        else {
            panic!("expected options parsed");
        };
        assert_eq!(
            options_parsed.explicit_cmd_line,
            vec![
                "--isolation-dir=buildbuddy-bes-compare",
                "--config-file=.buckconfig.local",
                "--prefer-remote",
            ]
        );
    }

    #[test]
    fn command_name_inference_skips_startup_option_values() {
        let args = vec![
            "buck2".to_owned(),
            "--isolation-dir".to_owned(),
            "build".to_owned(),
            "test".to_owned(),
            "//:main".to_owned(),
        ];
        assert_eq!(command_name_from_args(&args), Some("test"));

        let args_without_executable = vec!["build".to_owned(), "//:main".to_owned()];
        assert_eq!(
            command_name_from_args(&args_without_executable),
            Some("build")
        );
    }

    #[test]
    fn workspace_status_uses_source_date_epoch_metadata() {
        let event = workspace_status_event_from_metadata(
            Some(&HashMap::from([(
                "SOURCE_DATE_EPOCH".to_owned(),
                "42".to_owned(),
            )])),
            Vec::new(),
            Some(1_000),
        );
        let Some(build_event::Payload::WorkspaceStatus(status)) = event.payload.as_ref() else {
            panic!("expected workspace status");
        };
        let workspace_item = |key: &str| {
            status
                .item
                .iter()
                .find(|item| item.key == key)
                .map(|item| item.value.as_str())
        };
        assert_eq!(workspace_item("BUILD_EMBED_LABEL"), Some(""));
        assert_eq!(workspace_item("BUILD_TIMESTAMP"), Some("42"));
        assert_eq!(
            workspace_item("FORMATTED_DATE"),
            Some("1970 Jan 01 00 00 42 Thu")
        );
    }

    #[test]
    fn workspace_status_uses_embed_label_metadata() {
        let event = workspace_status_event_from_metadata(
            Some(&HashMap::from([(
                "BUILD_EMBED_LABEL".to_owned(),
                "release-2026".to_owned(),
            )])),
            Vec::new(),
            None,
        );
        let Some(build_event::Payload::WorkspaceStatus(status)) = event.payload.as_ref() else {
            panic!("expected workspace status");
        };
        let embed_label = status
            .item
            .iter()
            .find(|item| item.key == "BUILD_EMBED_LABEL")
            .map(|item| item.value.as_str());
        assert_eq!(embed_label, Some("release-2026"));
    }

    #[test]
    fn options_parsed_reports_tool_tag() {
        let event = options_parsed_event_from_args(
            &[
                "buck2".to_owned(),
                "build".to_owned(),
                "--tool-tag".to_owned(),
                "ci-runner".to_owned(),
                "//:main".to_owned(),
            ],
            "build",
        );
        let Some(build_event::Payload::OptionsParsed(options)) = event.payload.as_ref() else {
            panic!("expected options parsed");
        };

        assert_eq!(options.tool_tag, "ci-runner");
        assert_eq!(options.cmd_line, vec!["--tool-tag=ci-runner"]);
        assert_eq!(options.explicit_cmd_line, vec!["--tool-tag=ci-runner"]);
    }

    #[test]
    fn http_materialization_emits_fetch_event() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Materialization(
                        buck2_data::MaterializationEnd {
                            success: true,
                            method: Some(buck2_data::MaterializationMethod::HttpDownload as i32),
                            url: Some("https://example.test/archive.tar.zst".to_owned()),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                },
            )),
        );

        let fetch_event = events
            .iter()
            .find(|event| matches!(event.payload, Some(build_event::Payload::Fetch(_))))
            .expect("Fetch event");
        let Some(build_event_id::Id::Fetch(fetch_id)) =
            fetch_event.id.as_ref().and_then(|id| id.id.as_ref())
        else {
            panic!("expected Fetch id");
        };
        assert_eq!(fetch_id.url, "https://example.test/archive.tar.zst");
        assert_eq!(
            fetch_id.downloader,
            build_event_id::fetch_id::Downloader::Http as i32
        );
        let Some(build_event::Payload::Fetch(fetch)) = fetch_event.payload.as_ref() else {
            panic!("expected Fetch payload");
        };
        assert!(fetch.success);
    }

    #[test]
    fn cas_materialization_does_not_emit_fetch_event() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Materialization(
                        buck2_data::MaterializationEnd {
                            success: true,
                            method: Some(buck2_data::MaterializationMethod::CasDownload as i32),
                            url: Some("https://example.test/archive.tar.zst".to_owned()),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                },
            )),
        );

        assert!(
            !events
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::Fetch(_))))
        );
    }

    #[test]
    fn external_resource_fetch_emits_fetch_event() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::ExternalResourceFetch(
                        buck2_data::ExternalResourceFetch {
                            url: "https://github.com/example/lib.git".to_owned(),
                            downloader: buck2_data::ExternalResourceDownloader::UnknownDownloader
                                as i32,
                            success: false,
                        },
                    )),
                },
            )),
        );

        let fetch_event = events
            .iter()
            .find(|event| matches!(event.payload, Some(build_event::Payload::Fetch(_))))
            .expect("Fetch event");
        let Some(build_event_id::Id::Fetch(fetch_id)) =
            fetch_event.id.as_ref().and_then(|id| id.id.as_ref())
        else {
            panic!("expected Fetch id");
        };
        assert_eq!(fetch_id.url, "https://github.com/example/lib.git");
        assert_eq!(
            fetch_id.downloader,
            build_event_id::fetch_id::Downloader::Unknown as i32
        );
        let Some(build_event::Payload::Fetch(fetch)) = fetch_event.payload.as_ref() else {
            panic!("expected Fetch payload");
        };
        assert!(!fetch.success);
    }

    #[test]
    fn re_log_stream_emits_test_progress_for_tests() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::ReLogStreamAvailable(
                        buck2_data::ReLogStreamAvailable {
                            action_digest: "action-digest".to_owned(),
                            stdout_stream_name: "uploads/test/stdout".to_owned(),
                            stderr_stream_name: "uploads/test/stderr".to_owned(),
                            action_key: Some("//pkg:main test test_case".to_owned()),
                            use_case: "buck2_test".to_owned(),
                            key: Some(buck2_data::ActionKey {
                                owner: Some(buck2_data::action_key::Owner::TestTargetLabel(
                                    target.clone(),
                                )),
                                ..Default::default()
                            }),
                        },
                    )),
                },
            )),
        );

        let progress = events
            .iter()
            .find(|event| matches!(event.payload, Some(build_event::Payload::TestProgress(_))))
            .expect("TestProgress event");
        let Some(build_event_id::Id::TestProgress(progress_id)) =
            progress.id.as_ref().and_then(|id| id.id.as_ref())
        else {
            panic!("expected TestProgress id");
        };
        assert_eq!(progress_id.label, "//pkg:main");
        assert_eq!(progress_id.configuration.as_ref().unwrap().id, "cfg");
        assert_eq!(progress_id.run, 1);
        assert_eq!(progress_id.shard, 1);
        assert_eq!(progress_id.attempt, 1);
        assert_eq!(progress_id.opaque_count, 1);
        let Some(build_event::Payload::TestProgress(progress)) = progress.payload.as_ref() else {
            panic!("expected TestProgress payload");
        };
        assert_eq!(progress.uri, "uploads/test/stdout");
    }

    #[test]
    fn re_log_stream_ignores_non_test_actions() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::ReLogStreamAvailable(
                        buck2_data::ReLogStreamAvailable {
                            action_digest: "action-digest".to_owned(),
                            stdout_stream_name: "uploads/action/stdout".to_owned(),
                            stderr_stream_name: String::new(),
                            action_key: Some("//pkg:main compile".to_owned()),
                            use_case: "buck2_build".to_owned(),
                            key: Some(buck2_data::ActionKey {
                                owner: Some(buck2_data::action_key::Owner::TargetLabel(target)),
                                ..Default::default()
                            }),
                        },
                    )),
                },
            )),
        );

        assert!(
            !events
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::TestProgress(_))))
        );
    }

    #[test]
    fn run_exec_request_emits_exec_request_constructed() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::RunExecRequest(
                        buck2_data::RunExecRequest {
                            working_directory: "/repo".to_owned(),
                            argv: vec!["buck-out/bin/app".to_owned(), "--flag".to_owned()],
                            env: vec![buck2_data::EnvironmentEntry {
                                key: "BUCK_RUN_BUILD_ID".to_owned(),
                                value: "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa".to_owned(),
                            }],
                            env_to_clear: vec!["BUCK2_WRAPPER".to_owned()],
                            should_exec: true,
                        },
                    )),
                },
            )),
        );

        let exec_event = events
            .iter()
            .find(|event| matches!(event.payload, Some(build_event::Payload::ExecRequest(_))))
            .expect("ExecRequestConstructed event");
        assert!(matches!(
            exec_event.id.as_ref().and_then(|id| id.id.as_ref()),
            Some(build_event_id::Id::ExecRequest(_))
        ));
        let Some(build_event::Payload::ExecRequest(exec_request)) = exec_event.payload.as_ref()
        else {
            panic!("expected ExecRequestConstructed payload");
        };
        assert_eq!(exec_request.working_directory, b"/repo");
        assert_eq!(
            exec_request.argv,
            vec![b"buck-out/bin/app".to_vec(), b"--flag".to_vec()]
        );
        assert_eq!(exec_request.environment_variable.len(), 1);
        assert_eq!(
            exec_request.environment_variable[0].name,
            b"BUCK_RUN_BUILD_ID"
        );
        assert_eq!(
            exec_request.environment_variable[0].value,
            b"aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa"
        );
        assert_eq!(
            exec_request.environment_variable_to_clear,
            vec![b"BUCK2_WRAPPER".to_vec()]
        );
        assert!(exec_request.should_exec);
    }

    #[test]
    fn create_output_symlinks_emits_convenience_symlinks() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::CreateOutputSymlinks(
                        buck2_data::CreateOutputSymlinksEnd {
                            symlinks: vec![buck2_data::OutputSymlink {
                                path: "buck-out/unhashed/pkg/app".to_owned(),
                                target: "gen/root/pkg/app".to_owned(),
                            }],
                        },
                    )),
                    ..Default::default()
                },
            )),
        );

        let event = events
            .iter()
            .find(|event| {
                matches!(
                    event.payload,
                    Some(build_event::Payload::ConvenienceSymlinksIdentified(_))
                )
            })
            .expect("convenience symlinks event");
        assert!(matches!(
            event.id.as_ref().and_then(|id| id.id.as_ref()),
            Some(build_event_id::Id::ConvenienceSymlinksIdentified(_))
        ));
        let Some(build_event::Payload::ConvenienceSymlinksIdentified(symlinks)) =
            event.payload.as_ref()
        else {
            panic!("expected ConvenienceSymlinksIdentified payload");
        };
        assert_eq!(symlinks.convenience_symlinks.len(), 1);
        let symlink = &symlinks.convenience_symlinks[0];
        assert_eq!(symlink.path, "buck-out/unhashed/pkg/app");
        assert_eq!(
            symlink.action,
            bep::convenience_symlink::Action::Create as i32
        );
        assert_eq!(symlink.target, "gen/root/pkg/app");

        let final_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Command(
                        buck2_data::CommandEnd {
                            data: Some(buck2_data::command_end::Data::Build(
                                buck2_data::BuildCommandEnd {
                                    ..Default::default()
                                },
                            )),
                            is_success: true,
                            build_result: Some(buck2_data::BuildResult {
                                build_completed: true,
                            }),
                        },
                    )),
                    ..Default::default()
                },
            )),
        );
        assert!(!final_events.iter().any(|event| matches!(
            event.payload,
            Some(build_event::Payload::ConvenienceSymlinksIdentified(_))
        )));
    }

    #[test]
    fn invocation_record_emits_command_context_without_command_start() {
        let mut converter = BazelEventConverter::default();
        let first_events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::ConsoleMessage(
                        buck2_data::ConsoleMessage {
                            message: "starting".to_owned(),
                        },
                    )),
                },
            )),
        );
        assert!(matches!(
            first_events[0].payload,
            Some(build_event::Payload::Started(_))
        ));

        let final_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            cli_args: vec![
                                "/root/workspace/repo-root/buck-out/buildbuddy-rbe-build/art/gh_facebook_buck2/7703741d4b7244c6/app/buck2/__buck2-bin__/buck2".to_owned(),
                                "--isolation-dir".to_owned(),
                                "buildbuddy-rbe-selftest".to_owned(),
                                "build".to_owned(),
                                "--config-file".to_owned(),
                                ".buckconfig.local".to_owned(),
                                "--remote-only".to_owned(),
                                "//:buck2".to_owned(),
                            ],
                            command_name: None,
                            metadata: Some(buck2_data::TypedMetadata {
                                strings: HashMap::from([
                                    ("username".to_owned(), "alice".to_owned()),
                                    ("hostname".to_owned(), "workstation".to_owned()),
                                    ("CWD".to_owned(), "/repo".to_owned()),
                                    ("REPO_ROOT".to_owned(), "/repo".to_owned()),
                                    ("GIT_BRANCH".to_owned(), "main".to_owned()),
                                    ("GIT_COMMIT".to_owned(), "abc123".to_owned()),
                                ]),
                                ..Default::default()
                            }),
                            parsed_target_patterns: Some(buck2_data::ParsedTargetPatterns {
                                target_patterns: vec![buck2_data::TargetPattern {
                                    value: "//:buck2".to_owned(),
                                }],
                            }),
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            exit_code: Some(0),
                            exit_result_name: Some("SUCCESS".to_owned()),
                            wrapper_start_time: Some(2_000),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        assert!(
            !final_events
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::Aborted(_))))
        );

        let unstructured = final_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::UnstructuredCommandLine(command_line)) => {
                    Some(command_line)
                }
                _ => None,
            })
            .expect("unstructured command line");
        assert_eq!(
            unstructured.args.last().map(String::as_str),
            Some("//:buck2")
        );

        let original = final_events
            .iter()
            .find(|event| {
                event.id.as_ref() == Some(&structured_command_line_id(ORIGINAL_COMMAND_LINE_LABEL))
            })
            .map(command_line_from_event)
            .expect("original command line");
        assert_eq!(chunk_values(original, "executable"), vec!["buck2"]);
        assert_eq!(chunk_values(original, "command"), vec!["build"]);
        assert_eq!(
            option_values(original, "command options"),
            vec![
                "--isolation-dir=buildbuddy-rbe-selftest",
                "--config-file=.buckconfig.local",
                "--remote-only",
            ]
        );
        assert_eq!(chunk_values(original, "residual"), vec!["//:buck2"]);

        let canonical = final_events
            .iter()
            .find(|event| {
                event.id.as_ref() == Some(&structured_command_line_id(CANONICAL_COMMAND_LINE_LABEL))
            })
            .map(command_line_from_event)
            .expect("canonical command line");
        assert_eq!(chunk_values(canonical, "command"), vec!["build"]);
        assert_eq!(chunk_values(canonical, "residue"), vec!["//:buck2"]);
        assert!(
            option_values(canonical, "command options")
                .contains(&"--client_env=USER=alice".to_owned())
        );

        let options_parsed = final_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::OptionsParsed(options)) => Some(options),
                _ => None,
            })
            .expect("options parsed");
        assert_eq!(
            options_parsed.explicit_cmd_line,
            vec![
                "--isolation-dir=buildbuddy-rbe-selftest",
                "--config-file=.buckconfig.local",
                "--remote-only",
            ]
        );

        let workspace_status = final_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::WorkspaceStatus(status)) => Some(status),
                _ => None,
            })
            .expect("workspace status");
        let workspace_item = |key: &str| {
            workspace_status
                .item
                .iter()
                .find(|item| item.key == key)
                .map(|item| item.value.as_str())
        };
        assert_eq!(workspace_item("BUILD_USER"), Some("alice"));
        assert_eq!(workspace_item("BUILD_HOST"), Some("workstation"));
        assert_eq!(workspace_item("BUILD_TIMESTAMP"), Some("2"));
        assert_eq!(
            workspace_item("FORMATTED_DATE"),
            Some("1970 Jan 01 00 00 02 Thu")
        );
        assert_eq!(workspace_item("BUILD_WORKING_DIRECTORY"), Some("/repo"));
        assert_eq!(workspace_item("PATTERN"), Some("//:buck2"));

        let build_metadata = final_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildMetadata(metadata)) => Some(metadata),
                _ => None,
            })
            .expect("build metadata");
        assert_eq!(
            build_metadata.metadata.get("USER").map(String::as_str),
            Some("alice")
        );
        assert_eq!(
            build_metadata
                .metadata
                .get("COMMIT_SHA")
                .map(String::as_str),
            Some("abc123")
        );
        assert_eq!(
            build_metadata
                .metadata
                .get("BUCK2_COMMAND")
                .map(String::as_str),
            Some("build")
        );
        assert_eq!(
            build_metadata
                .metadata
                .get(BUILDBUDDY_VISIBILITY_KEY)
                .map(String::as_str),
            Some(BUILDBUDDY_PUBLIC_VISIBILITY)
        );
    }

    #[test]
    fn invocation_record_first_event_populates_started_context_and_pattern() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "build".to_owned(),
                                "--config-file".to_owned(),
                                ".buckconfig.local".to_owned(),
                                "//:buck2".to_owned(),
                            ],
                            command_name: None,
                            metadata: Some(buck2_data::TypedMetadata {
                                strings: HashMap::from([
                                    ("username".to_owned(), "alice".to_owned()),
                                    ("hostname".to_owned(), "workstation".to_owned()),
                                    ("CWD".to_owned(), "/repo/subdir".to_owned()),
                                ]),
                                ..Default::default()
                            }),
                            repo_path: Some("/repo".to_owned()),
                            parsed_target_patterns: Some(buck2_data::ParsedTargetPatterns {
                                target_patterns: vec![buck2_data::TargetPattern {
                                    value: "//:buck2".to_owned(),
                                }],
                            }),
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            exit_code: Some(0),
                            exit_result_name: Some("SUCCESS".to_owned()),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let started = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::Started(started)) => Some((event, started)),
                _ => None,
            })
            .expect("started event");
        assert_eq!(started.1.command, "build");
        assert_eq!(
            started.1.options_description,
            "buck2 build --config-file .buckconfig.local //:buck2"
        );
        assert_eq!(started.1.working_directory, "/repo/subdir");
        assert_eq!(started.1.workspace_directory, "/repo");
        assert_eq!(started.1.host, "workstation");
        assert_eq!(started.1.user, "alice");
        assert!(
            started
                .0
                .children
                .iter()
                .any(|child| child == &pattern_expanded_id(vec!["//:buck2".to_owned()]))
        );
        assert!(events.iter().any(|event| {
            event.id.as_ref() == Some(&pattern_expanded_id(vec!["//:buck2".to_owned()]))
                && matches!(
                    event.payload.as_ref(),
                    Some(build_event::Payload::Expanded(_))
                )
        }));
        assert!(!events.iter().any(|event| matches!(
            event.payload.as_ref(),
            Some(build_event::Payload::Aborted(_))
        )));
    }

    #[test]
    fn invocation_record_does_not_duplicate_command_start_context() {
        let mut converter = BazelEventConverter::default();
        let start_events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "build".to_owned(),
                                "//:main".to_owned(),
                            ],
                            data: Some(buck2_data::command_start::Data::Build(
                                buck2_data::BuildCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );
        assert!(start_events.iter().any(|event| {
            matches!(
                event.payload,
                Some(build_event::Payload::UnstructuredCommandLine(_))
            )
        }));

        let final_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "build".to_owned(),
                                "//:main".to_owned(),
                            ],
                            command_name: Some("build".to_owned()),
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            exit_code: Some(0),
                            exit_result_name: Some("SUCCESS".to_owned()),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        assert!(!final_events.iter().any(|event| matches!(
            event.payload,
            Some(build_event::Payload::UnstructuredCommandLine(_))
                | Some(build_event::Payload::StructuredCommandLine(_))
                | Some(build_event::Payload::OptionsParsed(_))
                | Some(build_event::Payload::WorkspaceStatus(_))
                | Some(build_event::Payload::Configuration(_))
                | Some(build_event::Payload::WorkspaceInfo(_))
        )));
    }

    #[test]
    fn command_start_preserves_explicit_buildbuddy_visibility() {
        let event = build_metadata_event(&build_metadata_from_command(
            &buck2_data::CommandStart {
                metadata: HashMap::from([(
                    BUILDBUDDY_VISIBILITY_KEY.to_owned(),
                    "PRIVATE".to_owned(),
                )]),
                ..Default::default()
            },
            &BTreeMap::new(),
        ));

        let build_metadata = match event.payload.as_ref() {
            Some(build_event::Payload::BuildMetadata(metadata)) => metadata,
            other => panic!("expected build metadata, got {other:?}"),
        };
        assert_eq!(
            build_metadata
                .metadata
                .get(BUILDBUDDY_VISIBILITY_KEY)
                .map(String::as_str),
            Some("PRIVATE")
        );
    }

    #[test]
    fn command_start_includes_configured_build_metadata() {
        let mut converter = BazelEventConverter::new_with_options(
            [
                ("PARENT_RUN_ID".to_owned(), "workflow-run".to_owned()),
                (BUILDBUDDY_VISIBILITY_KEY.to_owned(), "PRIVATE".to_owned()),
            ],
            true,
        );
        let event = trace_event(buck2_data::buck_event::Data::SpanStart(
            buck2_data::SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::Command(
                    buck2_data::CommandStart {
                        data: Some(buck2_data::command_start::Data::Build(
                            buck2_data::BuildCommandStart {},
                        )),
                        ..Default::default()
                    },
                )),
            },
        ));

        let events = converter.convert(1, &event);
        let started_index = events
            .iter()
            .position(|event| matches!(event.payload, Some(build_event::Payload::Started(_))))
            .expect("started event");
        let build_metadata_index = events
            .iter()
            .position(|event| matches!(event.payload, Some(build_event::Payload::BuildMetadata(_))))
            .expect("build metadata event");
        assert!(started_index < build_metadata_index);
        assert!(
            events[started_index]
                .children
                .iter()
                .any(|child| child == &build_metadata_id())
        );
        let build_metadata = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildMetadata(metadata)) => Some(metadata),
                _ => None,
            })
            .expect("build metadata");
        assert_eq!(
            build_metadata
                .metadata
                .get("PARENT_RUN_ID")
                .map(String::as_str),
            Some("workflow-run")
        );
        assert_eq!(
            build_metadata
                .metadata
                .get(BUILDBUDDY_VISIBILITY_KEY)
                .map(String::as_str),
            Some("PRIVATE")
        );
    }

    #[test]
    fn pattern_expanded_links_resolved_targets() {
        let mut converter = BazelEventConverter::default();
        let patterns = trace_event(buck2_data::buck_event::Data::Instant(
            buck2_data::InstantEvent {
                data: Some(buck2_data::instant_event::Data::TargetPatterns(
                    buck2_data::ParsedTargetPatterns {
                        target_patterns: vec![buck2_data::TargetPattern {
                            value: "//:main".to_owned(),
                        }],
                    },
                )),
            },
        ));

        let events = converter.convert(1, &patterns);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::Expanded(_))))
        );

        let build_graph = trace_event(buck2_data::buck_event::Data::Record(
            buck2_data::RecordEvent {
                data: Some(buck2_data::record_event::Data::BuildGraphStats(
                    buck2_data::BuildGraphStats {
                        build_targets: vec![buck2_data::BuildTarget {
                            target: "//:main".to_owned(),
                            configuration: "cfg".to_owned(),
                            configured_graph_size: None,
                        }],
                    },
                )),
            },
        ));
        let events = converter.convert(2, &build_graph);
        let expanded = events
            .iter()
            .find(|event| matches!(event.payload, Some(build_event::Payload::Expanded(_))))
            .expect("PatternExpanded event");

        let Some(build_event_id::Id::Pattern(pattern_id)) =
            expanded.id.as_ref().and_then(|id| id.id.as_ref())
        else {
            panic!("expected PatternExpanded id");
        };
        assert_eq!(pattern_id.pattern, vec!["//:main"]);
        assert!(
            expanded
                .children
                .iter()
                .any(|child| child == &target_configured_id("//:main".to_owned()))
        );
    }

    #[test]
    fn parsed_target_patterns_keep_announced_cli_pattern_id() {
        let mut converter = BazelEventConverter::default();
        let raw_pattern = pattern_expanded_id(vec!["//app/...".to_owned()]);
        let qualified_pattern = pattern_expanded_id(vec!["gh_facebook_buck2//app/...".to_owned()]);

        let start_events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "build".to_owned(),
                                "//app/...".to_owned(),
                            ],
                            data: Some(buck2_data::command_start::Data::Build(
                                buck2_data::BuildCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );
        assert!(
            start_events[0]
                .children
                .iter()
                .any(|child| child == &raw_pattern)
        );

        converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::TargetPatterns(
                        buck2_data::ParsedTargetPatterns {
                            target_patterns: vec![buck2_data::TargetPattern {
                                value: "gh_facebook_buck2//app/...".to_owned(),
                            }],
                        },
                    )),
                },
            )),
        );

        let graph_events = converter.convert(
            3,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::BuildGraphStats(
                        buck2_data::BuildGraphStats {
                            build_targets: vec![buck2_data::BuildTarget {
                                target: "gh_facebook_buck2//app:lib".to_owned(),
                                configuration: "cfg".to_owned(),
                                configured_graph_size: None,
                            }],
                        },
                    )),
                },
            )),
        );

        let expanded = graph_events
            .iter()
            .find(|event| matches!(event.payload, Some(build_event::Payload::Expanded(_))))
            .expect("PatternExpanded event");
        assert_eq!(expanded.id.as_ref(), Some(&raw_pattern));
        assert!(expanded.children.iter().any(|child| {
            child == &target_configured_id("@gh_facebook_buck2//app:lib".to_owned())
        }));

        let final_events = converter.convert(
            4,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            parsed_target_patterns: Some(buck2_data::ParsedTargetPatterns {
                                target_patterns: vec![buck2_data::TargetPattern {
                                    value: "gh_facebook_buck2//app/...".to_owned(),
                                }],
                            }),
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            exit_code: Some(0),
                            exit_result_name: Some("SUCCESS".to_owned()),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        assert!(
            graph_events
                .iter()
                .chain(final_events.iter())
                .all(|event| event.id.as_ref() != Some(&qualified_pattern))
        );
        assert!(!final_events.iter().any(|event| {
            event.id.as_ref() == Some(&raw_pattern)
                && matches!(event.payload, Some(build_event::Payload::Aborted(_)))
        }));
    }

    #[test]
    fn command_start_patterns_expand_at_invocation_end() {
        let mut converter = BazelEventConverter::default();

        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "build".to_owned(),
                                "//:main".to_owned(),
                            ],
                            data: Some(buck2_data::command_start::Data::Build(
                                buck2_data::BuildCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );
        let final_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let expanded = final_events
            .iter()
            .find(|event| matches!(event.payload, Some(build_event::Payload::Expanded(_))))
            .expect("PatternExpanded event");
        assert_eq!(
            expanded.id.as_ref(),
            Some(&pattern_expanded_id(vec!["//:main".to_owned()]))
        );
    }

    #[test]
    fn command_start_patterns_expand_at_command_end() {
        let mut converter = BazelEventConverter::default();

        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "test".to_owned(),
                                "//:main".to_owned(),
                            ],
                            data: Some(buck2_data::command_start::Data::Test(
                                buck2_data::TestCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );

        let final_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Command(
                        buck2_data::CommandEnd {
                            data: Some(buck2_data::command_end::Data::Test(
                                buck2_data::TestCommandEnd::default(),
                            )),
                            is_success: true,
                            build_result: Some(buck2_data::BuildResult {
                                build_completed: true,
                            }),
                        },
                    )),
                    ..Default::default()
                },
            )),
        );

        let expanded_index = final_events
            .iter()
            .position(|event| {
                event.id.as_ref() == Some(&pattern_expanded_id(vec!["//:main".to_owned()]))
                    && matches!(
                        event.payload.as_ref(),
                        Some(build_event::Payload::Expanded(_))
                    )
            })
            .expect("PatternExpanded event");
        let finished_index = final_events
            .iter()
            .position(is_build_finished_event)
            .expect("BuildFinished event");

        assert!(expanded_index < finished_index);
        assert!(!final_events.iter().any(|event| {
            event.id.as_ref() == Some(&pattern_expanded_id(vec!["//:main".to_owned()]))
                && matches!(
                    event.payload.as_ref(),
                    Some(build_event::Payload::Aborted(_))
                )
        }));
    }

    #[test]
    fn test_command_patterns_expand_before_test_results() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();
        let pattern_id = pattern_expanded_id(vec!["//pkg:main".to_owned()]);
        let target_id = target_configured_id("//pkg:main".to_owned());

        let start_events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "test".to_owned(),
                                "//pkg:main".to_owned(),
                            ],
                            data: Some(buck2_data::command_start::Data::Test(
                                buck2_data::TestCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );
        assert!(!start_events.iter().any(|event| matches!(
            event.payload,
            Some(build_event::Payload::WorkspaceStatus(_))
        )));

        let build_graph_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::BuildGraphStats(
                        buck2_data::BuildGraphStats {
                            build_targets: Vec::new(),
                        },
                    )),
                },
            )),
        );
        assert!(
            !build_graph_events
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::Expanded(_))))
        );

        let discovery_events = converter.convert(
            3,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::TestDiscovery(
                        buck2_data::TestDiscoveryStart {
                            suite_name: "suite".to_owned(),
                            target_label: Some(target.clone()),
                            labels: vec!["ci".to_owned()],
                        },
                    )),
                },
            )),
        );
        assert!(
            !discovery_events
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::Configured(_))))
        );

        let test_events = convert_through_end_of_results(&mut converter,
            4,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::TestRun(
                        Box::new(buck2_data::TestRunEnd {
                            suite: Some(buck2_data::TestSuite {
                                suite_name: "suite".to_owned(),
                                test_names: vec!["test_passes".to_owned()],
                                target_label: Some(target),
                                labels: vec!["ci".to_owned()],
                            }),
                            command_report: Some(buck2_data::CommandExecution {
                                details: Some(buck2_data::CommandExecutionDetails {
                                    signed_exit_code: Some(0),
                                    cmd_stdout: "test stdout".to_owned(),
                                    command_kind: Some(buck2_data::CommandExecutionKind {
                                        command: Some(
                                            buck2_data::command_execution_kind::Command::LocalCommand(
                                                buck2_data::LocalCommand {
                                                    argv: vec!["test-binary".to_owned()],
                                                    ..Default::default()
                                                },
                                            ),
                                        ),
                                    }),
                                    ..Default::default()
                                }),
                                status: Some(buck2_data::command_execution::Status::Success(
                                    buck2_data::command_execution::Success {},
                                )),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                    )),
                    ..Default::default()
                },
            )),
        );

        let expanded_index = test_events
            .iter()
            .position(|event| event.id.as_ref() == Some(&pattern_id))
            .expect("PatternExpanded event");
        assert!(test_events[expanded_index].children.contains(&target_id));
        let configured_index = test_events
            .iter()
            .position(|event| matches!(event.payload, Some(build_event::Payload::Configured(_))))
            .expect("TargetConfigured event");
        let workspace_status_index = test_events
            .iter()
            .position(|event| {
                matches!(
                    event.payload,
                    Some(build_event::Payload::WorkspaceStatus(_))
                )
            })
            .expect("WorkspaceStatus event");
        let test_result_index = test_events
            .iter()
            .position(|event| matches!(event.payload, Some(build_event::Payload::TestResult(_))))
            .expect("TestResult event");
        let test_summary_index = test_events
            .iter()
            .position(|event| matches!(event.payload, Some(build_event::Payload::TestSummary(_))))
            .expect("TestSummary event");

        assert!(expanded_index < configured_index);
        assert!(configured_index < workspace_status_index);
        assert!(workspace_status_index < test_result_index);
        assert!(test_result_index < test_summary_index);
    }

    #[test]
    fn test_command_build_graph_targets_expand_with_children() {
        let mut converter = BazelEventConverter::default();
        let pattern_id = pattern_expanded_id(vec!["//pkg:main".to_owned()]);
        let target_id = target_configured_id("//pkg:main".to_owned());

        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "test".to_owned(),
                                "//pkg:main".to_owned(),
                            ],
                            data: Some(buck2_data::command_start::Data::Test(
                                buck2_data::TestCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );

        let build_graph_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::BuildGraphStats(
                        buck2_data::BuildGraphStats {
                            build_targets: vec![buck2_data::BuildTarget {
                                target: "//pkg:main".to_owned(),
                                configuration: "cfg".to_owned(),
                                configured_graph_size: None,
                            }],
                        },
                    )),
                },
            )),
        );

        let expanded_index = build_graph_events
            .iter()
            .position(|event| event.id.as_ref() == Some(&pattern_id))
            .expect("PatternExpanded event");
        assert!(
            build_graph_events[expanded_index]
                .children
                .contains(&target_id)
        );
        let configured_index = build_graph_events
            .iter()
            .position(|event| matches!(event.payload, Some(build_event::Payload::Configured(_))))
            .expect("TargetConfigured event");
        let workspace_status_index = build_graph_events
            .iter()
            .position(|event| {
                matches!(
                    event.payload,
                    Some(build_event::Payload::WorkspaceStatus(_))
                )
            })
            .expect("WorkspaceStatus event");

        assert!(expanded_index < configured_index);
        assert!(configured_index < workspace_status_index);
    }

    #[test]
    fn test_command_build_graph_targets_expand_to_qualified_root_cell_children() {
        let mut converter = BazelEventConverter::default();
        let pattern_id = pattern_expanded_id(vec!["//pkg:main".to_owned()]);
        let qualified_target_id = target_configured_id("@gh_facebook_buck2//pkg:main".to_owned());
        let unqualified_target_id = target_configured_id("//pkg:main".to_owned());

        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "test".to_owned(),
                                "//pkg:main".to_owned(),
                            ],
                            data: Some(buck2_data::command_start::Data::Test(
                                buck2_data::TestCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );

        let build_graph_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::BuildGraphStats(
                        buck2_data::BuildGraphStats {
                            build_targets: vec![
                                buck2_data::BuildTarget {
                                    target: "@prelude//toolchains:rust".to_owned(),
                                    configuration: "cfg".to_owned(),
                                    configured_graph_size: None,
                                },
                                buck2_data::BuildTarget {
                                    target: "@gh_facebook_buck2//pkg:main".to_owned(),
                                    configuration: "cfg".to_owned(),
                                    configured_graph_size: None,
                                },
                            ],
                        },
                    )),
                },
            )),
        );

        let expanded = build_graph_events
            .iter()
            .find(|event| event.id.as_ref() == Some(&pattern_id))
            .expect("PatternExpanded event");
        assert!(expanded.children.contains(&qualified_target_id));
        assert!(!expanded.children.contains(&unqualified_target_id));
    }

    #[test]
    fn test_command_exact_patterns_expand_with_root_cell_metadata() {
        let mut converter = BazelEventConverter::new_with_options(
            [("ROOT_CELL".to_owned(), "gh_facebook_buck2".to_owned())],
            false,
        );
        let pattern_id = pattern_expanded_id(vec!["//pkg:main".to_owned()]);
        let qualified_target_id = target_configured_id("@gh_facebook_buck2//pkg:main".to_owned());
        let unqualified_target_id = target_configured_id("//pkg:main".to_owned());

        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "test".to_owned(),
                                "//pkg:main".to_owned(),
                            ],
                            data: Some(buck2_data::command_start::Data::Test(
                                buck2_data::TestCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );

        let end_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Command(
                        buck2_data::CommandEnd {
                            data: Some(buck2_data::command_end::Data::Test(
                                buck2_data::TestCommandEnd::default(),
                            )),
                            is_success: true,
                            build_result: Some(buck2_data::BuildResult {
                                build_completed: true,
                            }),
                        },
                    )),
                    ..Default::default()
                },
            )),
        );

        let expanded = end_events
            .iter()
            .find(|event| event.id.as_ref() == Some(&pattern_id))
            .expect("PatternExpanded event");
        assert!(expanded.children.contains(&qualified_target_id));
        assert!(!expanded.children.contains(&unqualified_target_id));
    }

    #[test]
    fn test_command_root_cell_patterns_prefer_requested_targets() {
        let mut converter = BazelEventConverter::new_with_options(
            [("ROOT_CELL".to_owned(), "gh_facebook_buck2".to_owned())],
            false,
        );
        let pattern_id = pattern_expanded_id(vec!["//app/buck2_core:soft_error".to_owned()]);
        let requested_target_id =
            target_configured_id("@gh_facebook_buck2//app/buck2_core:soft_error".to_owned());
        let unrelated_target = buck2_data::ConfiguredTargetLabel {
            label: Some(buck2_data::TargetLabel {
                package: "@buildbuddy_toolchains//".to_owned(),
                name: "empty_dex".to_owned(),
            }),
            configuration: Some(buck2_data::Configuration {
                full_name: "cfg".to_owned(),
            }),
            execution_configuration: None,
        };
        let unrelated_target_id =
            target_configured_id("@buildbuddy_toolchains//:empty_dex".to_owned());

        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "test".to_owned(),
                                "//app/buck2_core:soft_error".to_owned(),
                            ],
                            data: Some(buck2_data::command_start::Data::Test(
                                buck2_data::TestCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );

        converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Analysis(
                        buck2_data::AnalysisStart {
                            target: Some(buck2_data::analysis_start::Target::StandardTarget(
                                unrelated_target,
                            )),
                            rule: "genrule".to_owned(),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );

        let end_events = converter.convert(
            3,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Command(
                        buck2_data::CommandEnd {
                            data: Some(buck2_data::command_end::Data::Test(
                                buck2_data::TestCommandEnd::default(),
                            )),
                            is_success: true,
                            build_result: Some(buck2_data::BuildResult {
                                build_completed: true,
                            }),
                        },
                    )),
                    ..Default::default()
                },
            )),
        );

        let expanded = end_events
            .iter()
            .find(|event| event.id.as_ref() == Some(&pattern_id))
            .expect("PatternExpanded event");
        assert!(expanded.children.contains(&requested_target_id));
        assert!(!expanded.children.contains(&unrelated_target_id));
    }

    #[test]
    fn test_command_late_patterns_expand_before_test_results() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();
        let pattern_id = pattern_expanded_id(vec!["//pkg:main".to_owned()]);
        let target_id = target_configured_id("//pkg:main".to_owned());

        let start_events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec!["buck2".to_owned(), "test".to_owned()],
                            data: Some(buck2_data::command_start::Data::Test(
                                buck2_data::TestCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );
        assert!(!start_events.iter().any(|event| matches!(
            event.payload,
            Some(build_event::Payload::WorkspaceStatus(_))
        )));

        let pattern_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::TargetPatterns(
                        buck2_data::ParsedTargetPatterns {
                            target_patterns: vec![buck2_data::TargetPattern {
                                value: "//pkg:main".to_owned(),
                            }],
                        },
                    )),
                },
            )),
        );
        assert!(pattern_events.is_empty());

        let build_graph_events = converter.convert(
            3,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::BuildGraphStats(
                        buck2_data::BuildGraphStats {
                            build_targets: Vec::new(),
                        },
                    )),
                },
            )),
        );
        assert!(
            !build_graph_events
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::Expanded(_))))
        );

        converter.convert(
            4,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::TestDiscovery(
                        buck2_data::TestDiscoveryStart {
                            suite_name: "suite".to_owned(),
                            target_label: Some(target.clone()),
                            labels: vec!["ci".to_owned()],
                        },
                    )),
                },
            )),
        );

        let test_events = convert_through_end_of_results(&mut converter,
            5,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::TestRun(
                        Box::new(buck2_data::TestRunEnd {
                            suite: Some(buck2_data::TestSuite {
                                suite_name: "suite".to_owned(),
                                test_names: vec!["test_passes".to_owned()],
                                target_label: Some(target),
                                labels: vec!["ci".to_owned()],
                            }),
                            command_report: Some(buck2_data::CommandExecution {
                                details: Some(buck2_data::CommandExecutionDetails {
                                    signed_exit_code: Some(0),
                                    command_kind: Some(buck2_data::CommandExecutionKind {
                                        command: Some(
                                            buck2_data::command_execution_kind::Command::LocalCommand(
                                                buck2_data::LocalCommand {
                                                    argv: vec!["test-binary".to_owned()],
                                                    ..Default::default()
                                                },
                                            ),
                                        ),
                                    }),
                                    ..Default::default()
                                }),
                                status: Some(buck2_data::command_execution::Status::Success(
                                    buck2_data::command_execution::Success {},
                                )),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                    )),
                    ..Default::default()
                },
            )),
        );

        let expanded_index = test_events
            .iter()
            .position(|event| event.id.as_ref() == Some(&pattern_id))
            .expect("PatternExpanded event");
        assert!(test_events[expanded_index].children.contains(&target_id));
        let workspace_status_index = test_events
            .iter()
            .position(|event| {
                matches!(
                    event.payload,
                    Some(build_event::Payload::WorkspaceStatus(_))
                )
            })
            .expect("WorkspaceStatus event");
        let test_result_index = test_events
            .iter()
            .position(|event| matches!(event.payload, Some(build_event::Payload::TestResult(_))))
            .expect("TestResult event");

        assert!(expanded_index < workspace_status_index);
        assert!(workspace_status_index < test_result_index);
    }

    #[test]
    fn test_command_patterns_expand_from_first_test_result_without_discovery() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();
        let pattern_id = pattern_expanded_id(vec!["//pkg:main".to_owned()]);
        let target_id = target_configured_id("//pkg:main".to_owned());

        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec!["buck2".to_owned(), "test".to_owned()],
                            data: Some(buck2_data::command_start::Data::Test(
                                buck2_data::TestCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );

        converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::TargetPatterns(
                        buck2_data::ParsedTargetPatterns {
                            target_patterns: vec![buck2_data::TargetPattern {
                                value: "//pkg:main".to_owned(),
                            }],
                        },
                    )),
                },
            )),
        );

        converter.convert(
            3,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::BuildGraphStats(
                        buck2_data::BuildGraphStats {
                            build_targets: Vec::new(),
                        },
                    )),
                },
            )),
        );

        let test_events = convert_through_end_of_results(&mut converter,
            4,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::TestRun(
                        Box::new(buck2_data::TestRunEnd {
                            suite: Some(buck2_data::TestSuite {
                                suite_name: "suite".to_owned(),
                                test_names: vec!["test_passes".to_owned()],
                                target_label: Some(target),
                                labels: vec!["ci".to_owned()],
                            }),
                            command_report: Some(buck2_data::CommandExecution {
                                details: Some(buck2_data::CommandExecutionDetails {
                                    signed_exit_code: Some(0),
                                    command_kind: Some(buck2_data::CommandExecutionKind {
                                        command: Some(
                                            buck2_data::command_execution_kind::Command::LocalCommand(
                                                buck2_data::LocalCommand {
                                                    argv: vec!["test-binary".to_owned()],
                                                    ..Default::default()
                                                },
                                            ),
                                        ),
                                    }),
                                    ..Default::default()
                                }),
                                status: Some(buck2_data::command_execution::Status::Success(
                                    buck2_data::command_execution::Success {},
                                )),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                    )),
                    ..Default::default()
                },
            )),
        );

        let expanded_index = test_events
            .iter()
            .position(|event| event.id.as_ref() == Some(&pattern_id))
            .expect("PatternExpanded event");
        assert!(test_events[expanded_index].children.contains(&target_id));
        let configured_index = test_events
            .iter()
            .position(|event| matches!(event.payload, Some(build_event::Payload::Configured(_))))
            .expect("TargetConfigured event");
        let workspace_status_index = test_events
            .iter()
            .position(|event| {
                matches!(
                    event.payload,
                    Some(build_event::Payload::WorkspaceStatus(_))
                )
            })
            .expect("WorkspaceStatus event");
        let test_result_index = test_events
            .iter()
            .position(|event| matches!(event.payload, Some(build_event::Payload::TestResult(_))))
            .expect("TestResult event");

        assert!(expanded_index < configured_index);
        assert!(configured_index < workspace_status_index);
        assert!(workspace_status_index < test_result_index);
    }

    #[test]
    fn test_command_exact_patterns_expand_without_configured_events() {
        let mut converter = BazelEventConverter::default();
        let pattern_id = pattern_expanded_id(vec!["//pkg:main".to_owned()]);
        let target_id = target_configured_id("//pkg:main".to_owned());

        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec!["buck2".to_owned(), "test".to_owned()],
                            data: Some(buck2_data::command_start::Data::Test(
                                buck2_data::TestCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );

        converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::TargetPatterns(
                        buck2_data::ParsedTargetPatterns {
                            target_patterns: vec![buck2_data::TargetPattern {
                                value: "//pkg:main".to_owned(),
                            }],
                        },
                    )),
                },
            )),
        );

        let end_events = converter.convert(
            3,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Command(
                        buck2_data::CommandEnd {
                            data: Some(buck2_data::command_end::Data::Test(
                                buck2_data::TestCommandEnd::default(),
                            )),
                            is_success: true,
                            build_result: Some(buck2_data::BuildResult {
                                build_completed: true,
                            }),
                        },
                    )),
                    ..Default::default()
                },
            )),
        );

        let expanded = end_events
            .iter()
            .find(|event| event.id.as_ref() == Some(&pattern_id))
            .expect("PatternExpanded event");
        assert!(expanded.children.contains(&target_id));
    }

    #[test]
    fn invocation_record_emits_build_metrics() {
        let mut converter = BazelEventConverter::default();
        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Analysis(
                        buck2_data::AnalysisEnd {
                            target: Some(buck2_data::analysis_end::Target::StandardTarget(
                                configured_target(),
                            )),
                            declared_actions: Some(23),
                            declared_artifacts: Some(31),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                },
            )),
        );
        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                        buck2_data::ActionExecutionEnd {
                            key: Some(buck2_data::ActionKey {
                                owner: Some(buck2_data::action_key::Owner::TargetLabel(
                                    configured_target(),
                                )),
                                key: "action-key".to_owned(),
                                ..Default::default()
                            }),
                            outputs: vec![
                                buck2_data::ActionOutput {
                                    tiny_digest: "buck-out/main.o".to_owned(),
                                    path: "buck-out/main.o".to_owned(),
                                    digest: "abcd:10".to_owned(),
                                    size: 10,
                                    is_directory: false,
                                },
                                buck2_data::ActionOutput {
                                    tiny_digest: "buck-out/main.d".to_owned(),
                                    path: "buck-out/main.d".to_owned(),
                                    digest: "efgh:9".to_owned(),
                                    size: 9,
                                    is_directory: false,
                                },
                            ],
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                },
            )),
        );
        let event = trace_event(buck2_data::buck_event::Data::Record(
            buck2_data::RecordEvent {
                data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                    buck2_data::InvocationRecord {
                        outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                        run_local_count: 2,
                        run_remote_count: 3,
                        run_action_cache_count: 5,
                        run_remote_dep_file_cache_count: 7,
                        client_walltime: Some(prost_types::Duration {
                            seconds: 1,
                            nanos: 500_000_000,
                        }),
                        analysis_count: Some(7),
                        command_name: Some("build".to_owned()),
                        re_session_id: "re-session".to_owned(),
                        tags: vec!["ci".to_owned()],
                        first_snapshot: Some(buck2_data::Snapshot {
                            buck2_user_cpu_us: 10_000,
                            buck2_system_cpu_us: 20_000,
                            network_interface_stats: [(
                                "eth0".to_owned(),
                                buck2_data::NetworkInterfaceStats {
                                    tx_bytes: 1_000,
                                    rx_bytes: 2_000,
                                    network_kind: buck2_data::NetworkKind::Ethernet as i32,
                                },
                            )]
                            .into_iter()
                            .collect(),
                            ..Default::default()
                        }),
                        last_snapshot: Some(buck2_data::Snapshot {
                            buck2_user_cpu_us: 1_210_000,
                            buck2_system_cpu_us: 820_000,
                            dice_key_count: 44,
                            network_interface_stats: [(
                                "eth0".to_owned(),
                                buck2_data::NetworkInterfaceStats {
                                    tx_bytes: 7_000,
                                    rx_bytes: 11_000,
                                    network_kind: buck2_data::NetworkKind::Ethernet as i32,
                                },
                            )]
                            .into_iter()
                            .collect(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ))),
            },
        ));

        let events = converter.convert(1, &event);
        let finished_index = events
            .iter()
            .position(is_build_finished_event)
            .expect("build finished event");
        assert!(
            events[finished_index]
                .children
                .iter()
                .any(|child| child == &build_tool_logs_id())
        );
        assert!(
            events[finished_index]
                .children
                .iter()
                .any(|child| child == &build_metrics_id())
        );
        let metrics = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildMetrics(metrics)) => Some(metrics),
                _ => None,
            })
            .expect("build metrics event");
        let action_summary = metrics.action_summary.as_ref().unwrap();
        assert_eq!(action_summary.actions_created, 23);
        assert_eq!(action_summary.actions_created_not_including_aspects, 23);
        assert_eq!(action_summary.actions_executed, 17);
        assert_eq!(action_summary.remote_cache_hits, 12);
        assert!(action_summary.runner_count.iter().any(|count| {
            count.name == "total" && count.count == 17 && count.exec_kind.is_empty()
        }));
        assert!(action_summary.runner_count.iter().any(|count| {
            count.name == "remote dep file cache hit"
                && count.count == 7
                && count.exec_kind == "remote-cache"
        }));
        let action_cache_statistics = action_summary.action_cache_statistics.as_ref().unwrap();
        assert_eq!(action_cache_statistics.hits, 5);
        assert_eq!(action_cache_statistics.misses, 5);
        assert_eq!(action_cache_statistics.miss_details.len(), 1);
        assert_eq!(
            action_cache_statistics.miss_details[0].reason,
            blaze::action_cache_statistics::MissReason::NotCached as i32
        );
        assert_eq!(action_cache_statistics.miss_details[0].count, 5);
        assert_eq!(
            metrics.timing_metrics.as_ref().unwrap().wall_time_in_ms,
            1500
        );
        assert_eq!(
            metrics.timing_metrics.as_ref().unwrap().cpu_time_in_ms,
            2000
        );
        let network_stats = metrics
            .network_metrics
            .as_ref()
            .and_then(|metrics| metrics.system_network_stats.as_ref())
            .expect("system network stats");
        assert_eq!(network_stats.bytes_sent, 6_000);
        assert_eq!(network_stats.bytes_recv, 9_000);
        assert_eq!(network_stats.packets_sent, 0);
        assert_eq!(network_stats.packets_recv, 0);
        assert_eq!(
            metrics.target_metrics.as_ref().unwrap().targets_configured,
            7
        );
        let cumulative_metrics = metrics
            .cumulative_metrics
            .as_ref()
            .expect("cumulative metrics");
        assert_eq!(cumulative_metrics.num_analyses, 7);
        assert_eq!(cumulative_metrics.num_builds, 1);
        let graph_metrics = metrics
            .build_graph_metrics
            .as_ref()
            .expect("build graph metrics");
        assert_eq!(graph_metrics.action_count, 23);
        assert_eq!(graph_metrics.action_count_not_including_aspects, 23);
        assert_eq!(graph_metrics.output_artifact_count, 31);
        assert_eq!(graph_metrics.post_invocation_skyframe_node_count, 44);
        let output_artifacts_seen = metrics
            .artifact_metrics
            .as_ref()
            .and_then(|metrics| metrics.output_artifacts_seen.as_ref())
            .expect("output artifacts seen metric");
        assert_eq!(output_artifacts_seen.size_in_bytes, 19);
        assert_eq!(output_artifacts_seen.count, 2);
        let logs = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildToolLogs(logs)) => Some(logs),
                _ => None,
            })
            .expect("build tool logs event");
        assert_eq!(logs.log[0].name, "buck2-invocation.json");
        assert_eq!(logs.log[1].name, COMMAND_PROFILE_NAME);
        let contents = match logs.log[0].file.as_ref() {
            Some(bep::file::File::Contents(contents)) => contents,
            other => panic!("expected inline invocation summary, got {other:?}"),
        };
        let summary: serde_json::Value =
            serde_json::from_slice(contents).expect("valid summary json");
        assert_eq!(summary["command"], "build");
        assert_eq!(summary["outcome"], "success");
        assert_eq!(summary["re_session_id"], "re-session");
        assert_eq!(summary["actions"]["remote_cache"], 5);
        assert_eq!(summary["actions"]["remote_dep_file_cache"], 7);
        let profile_contents = match logs.log[1].file.as_ref() {
            Some(bep::file::File::Contents(contents)) => contents,
            other => panic!("expected inline command profile, got {other:?}"),
        };
        let mut decoder = GzDecoder::new(profile_contents.as_slice());
        let mut profile = String::new();
        decoder
            .read_to_string(&mut profile)
            .expect("valid gzip profile");
        assert!(profile.contains("\"traceEvents\":[\n"));
        let profile_json: serde_json::Value =
            serde_json::from_str(&profile).expect("valid profile json");
        assert!(
            profile_json["traceEvents"]
                .as_array()
                .expect("trace events array")
                .iter()
                .any(|event| event["name"] == "buck2 build")
        );
    }

    #[test]
    fn invocation_record_emits_dice_graph_size_without_actions() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            last_snapshot: Some(buck2_data::Snapshot {
                                dice_key_count: 12,
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let metrics = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildMetrics(metrics)) => Some(metrics),
                _ => None,
            })
            .expect("build metrics event");
        let graph_metrics = metrics
            .build_graph_metrics
            .as_ref()
            .expect("build graph metrics");
        assert_eq!(graph_metrics.action_count, 0);
        assert_eq!(graph_metrics.output_artifact_count, 0);
        assert_eq!(graph_metrics.post_invocation_skyframe_node_count, 12);
    }

    #[test]
    fn invocation_record_emits_package_metrics() {
        let mut converter = BazelEventConverter::default();
        for (sequence_number, path, duration_ms) in
            [(1, "pkg", 10), (2, "pkg/dep", 25), (3, "pkg", 50)]
        {
            converter.convert(
                sequence_number,
                &trace_event(buck2_data::buck_event::Data::SpanEnd(
                    buck2_data::SpanEndEvent {
                        data: Some(buck2_data::span_end_event::Data::LoadPackage(
                            buck2_data::LoadPackageEnd {
                                path: path.to_owned(),
                            },
                        )),
                        duration: Some(prost_types::Duration {
                            seconds: 0,
                            nanos: duration_ms * 1_000_000,
                        }),
                        ..Default::default()
                    },
                )),
            );
        }
        for (sequence_number, module_id, target_count, tick_count) in [
            (4, "pkg:BUCK", 3, 100),
            (5, "pkg/dep:BUCK", 5, 200),
            (6, "pkg:BUCK", 7, 500),
        ] {
            converter.convert(
                sequence_number,
                &trace_event(buck2_data::buck_event::Data::SpanEnd(
                    buck2_data::SpanEndEvent {
                        data: Some(buck2_data::span_end_event::Data::Load(
                            buck2_data::LoadBuildFileEnd {
                                module_id: module_id.to_owned(),
                                target_count: Some(target_count),
                                starlark_tick_count: Some(tick_count),
                                ..Default::default()
                            },
                        )),
                        ..Default::default()
                    },
                )),
            );
        }

        let events = converter.convert(
            7,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let metrics = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildMetrics(metrics)) => Some(metrics),
                _ => None,
            })
            .expect("build metrics event");
        let package_metrics = metrics.package_metrics.as_ref().expect("package metrics");
        assert_eq!(package_metrics.packages_loaded, 2);
        assert_eq!(package_metrics.package_load_metrics.len(), 2);
        assert_eq!(
            package_metrics.package_load_metrics[0].name.as_deref(),
            Some("pkg")
        );
        assert_eq!(
            package_metrics.package_load_metrics[0]
                .load_duration
                .as_ref()
                .map(duration_millis),
            Some(10)
        );
        assert_eq!(package_metrics.package_load_metrics[0].num_targets, Some(3));
        assert_eq!(
            package_metrics.package_load_metrics[0].computation_steps,
            Some(100)
        );
        assert_eq!(
            package_metrics.package_load_metrics[1].name.as_deref(),
            Some("pkg/dep")
        );
        assert_eq!(
            package_metrics.package_load_metrics[1]
                .load_duration
                .as_ref()
                .map(duration_millis),
            Some(25)
        );
        assert_eq!(package_metrics.package_load_metrics[1].num_targets, Some(5));
        assert_eq!(
            package_metrics.package_load_metrics[1].computation_steps,
            Some(200)
        );
    }

    #[test]
    fn invocation_record_emits_package_metrics_from_load_count() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            load_count: Some(3),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let metrics = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildMetrics(metrics)) => Some(metrics),
                _ => None,
            })
            .expect("build metrics event");
        assert_eq!(
            metrics
                .package_metrics
                .as_ref()
                .expect("package metrics")
                .packages_loaded,
            3
        );
    }

    #[test]
    fn invocation_record_emits_action_cache_artifact_metrics() {
        let mut converter = BazelEventConverter::default();
        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                        buck2_data::ActionExecutionEnd {
                            execution_kind: buck2_data::ActionExecutionKind::LocalActionCache
                                as i32,
                            outputs: vec![
                                buck2_data::ActionOutput {
                                    tiny_digest: "buck-out/local.o".to_owned(),
                                    path: "buck-out/local.o".to_owned(),
                                    digest: "abcd:10".to_owned(),
                                    size: 10,
                                    is_directory: false,
                                },
                                buck2_data::ActionOutput {
                                    tiny_digest: "buck-out/local.d".to_owned(),
                                    path: "buck-out/local.d".to_owned(),
                                    digest: "efgh:9".to_owned(),
                                    size: 9,
                                    is_directory: false,
                                },
                            ],
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                },
            )),
        );
        converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                        buck2_data::ActionExecutionEnd {
                            execution_kind: buck2_data::ActionExecutionKind::ActionCache as i32,
                            outputs: vec![buck2_data::ActionOutput {
                                tiny_digest: "buck-out/remote.o".to_owned(),
                                path: "buck-out/remote.o".to_owned(),
                                digest: "ijkl:4".to_owned(),
                                size: 4,
                                is_directory: false,
                            }],
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                },
            )),
        );

        let events = converter.convert(
            3,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let metrics = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildMetrics(metrics)) => Some(metrics),
                _ => None,
            })
            .expect("build metrics event");
        let artifact_metrics = metrics.artifact_metrics.as_ref().unwrap();
        let output_artifacts_seen = artifact_metrics.output_artifacts_seen.as_ref().unwrap();
        assert_eq!(output_artifacts_seen.size_in_bytes, 23);
        assert_eq!(output_artifacts_seen.count, 3);
        let output_artifacts_from_action_cache = artifact_metrics
            .output_artifacts_from_action_cache
            .as_ref()
            .expect("output artifacts from action cache");
        assert_eq!(output_artifacts_from_action_cache.size_in_bytes, 19);
        assert_eq!(output_artifacts_from_action_cache.count, 2);
    }

    #[test]
    fn invocation_record_emits_materialization_file_count() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            materialization_output_size: Some(4096),
                            materialization_files: Some(7),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let output_artifacts_seen = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildMetrics(metrics)) => metrics
                    .artifact_metrics
                    .as_ref()
                    .and_then(|metrics| metrics.output_artifacts_seen.as_ref()),
                _ => None,
            })
            .expect("output artifacts seen");
        assert_eq!(output_artifacts_seen.size_in_bytes, 4096);
        assert_eq!(output_artifacts_seen.count, 7);
    }

    #[test]
    fn invocation_record_emits_top_level_artifact_metrics() {
        let mut converter = BazelEventConverter::default();
        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::BuildGraphStats(
                        buck2_data::BuildGraphStats {
                            build_targets: vec![
                                buck2_data::BuildTarget {
                                    target: "//:main".to_owned(),
                                    configuration: "cfg".to_owned(),
                                    configured_graph_size: None,
                                },
                                buck2_data::BuildTarget {
                                    target: "//:other".to_owned(),
                                    configuration: "cfg".to_owned(),
                                    configured_graph_size: None,
                                },
                            ],
                        },
                    )),
                },
            )),
        );

        let main = configured_target_with_package("", "main", "cfg");
        let other = configured_target_with_package("", "other", "cfg");
        let dep = configured_target_with_package("", "dep", "cfg");
        for (sequence_number, target, action_key, output_path, size) in [
            (2, main, "main-action", "buck-out/shared.o", 10),
            (3, other, "other-action", "buck-out/shared.o", 10),
            (4, dep, "dep-action", "buck-out/dep.o", 5),
        ] {
            converter.convert(
                sequence_number,
                &trace_event(buck2_data::buck_event::Data::SpanEnd(
                    buck2_data::SpanEndEvent {
                        data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                            buck2_data::ActionExecutionEnd {
                                key: Some(buck2_data::ActionKey {
                                    key: action_key.to_owned(),
                                    owner: Some(buck2_data::action_key::Owner::TargetLabel(target)),
                                    ..Default::default()
                                }),
                                outputs: vec![buck2_data::ActionOutput {
                                    tiny_digest: output_path.to_owned(),
                                    path: output_path.to_owned(),
                                    digest: format!("{action_key}:{size}"),
                                    size,
                                    is_directory: false,
                                }],
                                ..Default::default()
                            },
                        ))),
                        ..Default::default()
                    },
                )),
            );
        }

        let events = converter.convert(
            5,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let metrics = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildMetrics(metrics)) => Some(metrics),
                _ => None,
            })
            .expect("build metrics event");
        let artifact_metrics = metrics.artifact_metrics.as_ref().unwrap();
        let output_artifacts_seen = artifact_metrics.output_artifacts_seen.as_ref().unwrap();
        assert_eq!(output_artifacts_seen.size_in_bytes, 25);
        assert_eq!(output_artifacts_seen.count, 3);
        let top_level_artifacts = artifact_metrics
            .top_level_artifacts
            .as_ref()
            .expect("top level artifacts");
        assert_eq!(top_level_artifacts.size_in_bytes, 20);
        assert_eq!(top_level_artifacts.count, 2);
    }

    #[test]
    fn invocation_record_emits_action_data() {
        let mut converter = BazelEventConverter::default();

        for (sequence_number, category, execution_kind, end_micros, duration_ms) in [
            (
                1,
                "cxx_compile",
                buck2_data::ActionExecutionKind::Local,
                20_000,
                10,
            ),
            (
                2,
                "genrule",
                buck2_data::ActionExecutionKind::Remote,
                40_000,
                5,
            ),
            (
                3,
                "cxx_compile",
                buck2_data::ActionExecutionKind::ActionCache,
                50_000,
                15,
            ),
            (
                4,
                "local_cache",
                buck2_data::ActionExecutionKind::LocalActionCache,
                60_000,
                5,
            ),
        ] {
            converter.convert(
                sequence_number,
                &trace_event_at(
                    sequence_number as u64,
                    1,
                    end_micros,
                    buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
                        data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                            buck2_data::ActionExecutionEnd {
                                name: Some(buck2_data::ActionName {
                                    category: category.to_owned(),
                                    identifier: "main".to_owned(),
                                }),
                                execution_kind: execution_kind as i32,
                                ..Default::default()
                            },
                        ))),
                        duration: Some(prost_types::Duration {
                            seconds: 0,
                            nanos: duration_ms * 1_000_000,
                        }),
                        ..Default::default()
                    }),
                ),
            );
        }

        let events = converter.convert(
            5,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let metrics = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildMetrics(metrics)) => Some(metrics),
                _ => None,
            })
            .expect("build metrics event");
        let action_data = &metrics.action_summary.as_ref().unwrap().action_data;
        assert_eq!(action_data.len(), 2);
        assert_eq!(action_data[0].mnemonic, "cxx_compile");
        assert_eq!(action_data[0].actions_executed, 2);
        assert_eq!(action_data[0].actions_created, 0);
        assert_eq!(action_data[0].first_started_ms, 1010);
        assert_eq!(action_data[0].last_ended_ms, 1050);
        assert_eq!(action_data[1].mnemonic, "genrule");
        assert_eq!(action_data[1].actions_executed, 1);
        assert_eq!(action_data[1].first_started_ms, 1035);
        assert_eq!(action_data[1].last_ended_ms, 1040);
    }

    #[test]
    fn command_end_emits_build_finished() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Command(
                        buck2_data::CommandEnd {
                            data: Some(buck2_data::command_end::Data::Query(
                                buck2_data::QueryCommandEnd {},
                            )),
                            is_success: false,
                            build_result: Some(buck2_data::BuildResult {
                                build_completed: true,
                            }),
                        },
                    )),
                    duration: Some(prost_types::Duration {
                        seconds: 2,
                        nanos: 500_000_000,
                    }),
                    ..Default::default()
                },
            )),
        );

        let finished_index = events
            .iter()
            .position(is_build_finished_event)
            .expect("build finished event");
        let convenience_index = events
            .iter()
            .position(|event| {
                matches!(
                    event.payload.as_ref(),
                    Some(build_event::Payload::ConvenienceSymlinksIdentified(_))
                )
            })
            .expect("convenience symlinks event");
        assert!(convenience_index < finished_index);
        let Some(build_event::Payload::ConvenienceSymlinksIdentified(symlinks)) =
            events[convenience_index].payload.as_ref()
        else {
            panic!("expected ConvenienceSymlinksIdentified payload");
        };
        assert!(symlinks.convenience_symlinks.is_empty());
        let finished = match events[finished_index].payload.as_ref() {
            Some(build_event::Payload::Finished(finished)) => finished,
            _ => unreachable!("finished index was matched above"),
        };
        assert_eq!(events[finished_index].children, vec![build_tool_logs_id()]);
        assert!(!finished.overall_success);
        assert_eq!(finished.finish_time_millis, 1_000);
        let exit_code = finished.exit_code.as_ref().expect("exit code");
        assert_eq!(exit_code.code, 1);
        assert_eq!(exit_code.name, "FAILED");

        let logs_index = events
            .iter()
            .position(|event| {
                matches!(
                    event.payload.as_ref(),
                    Some(build_event::Payload::BuildToolLogs(_))
                )
            })
            .expect("build tool logs event");
        let logs = match events[logs_index].payload.as_ref() {
            Some(build_event::Payload::BuildToolLogs(logs)) => logs,
            _ => unreachable!("logs index was matched above"),
        };
        assert!(finished_index < logs_index);
        assert_eq!(logs_index, events.len() - 1);
        assert!(!events[finished_index].last_message);
        assert!(events[logs_index].last_message);
        let final_progress_index = events
            .iter()
            .position(|event| {
                event.id.as_ref() == Some(&progress_id(0))
                    && matches!(
                        event.payload.as_ref(),
                        Some(build_event::Payload::Progress(_))
                    )
            })
            .expect("final progress event");
        assert!(finished_index < final_progress_index);
        assert!(final_progress_index < logs_index);
        assert_eq!(logs.log[1].name, COMMAND_PROFILE_NAME);
        let profile_contents = match logs.log[1].file.as_ref() {
            Some(bep::file::File::Contents(contents)) => contents,
            other => panic!("expected inline command profile, got {other:?}"),
        };
        let mut decoder = GzDecoder::new(profile_contents.as_slice());
        let mut profile = String::new();
        decoder
            .read_to_string(&mut profile)
            .expect("valid gzip profile");
        let profile_json: serde_json::Value =
            serde_json::from_str(&profile).expect("valid profile json");
        assert!(
            profile_json["traceEvents"]
                .as_array()
                .expect("trace events array")
                .iter()
                .any(|event| event["name"] == "buck2 query" && event["dur"] == 2_500_000)
        );
    }

    #[test]
    fn invocation_record_updates_command_end_final_events() {
        let mut converter = BazelEventConverter::default();
        let command_end_events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Command(
                        buck2_data::CommandEnd {
                            data: Some(buck2_data::command_end::Data::Test(
                                buck2_data::TestCommandEnd {
                                    ..Default::default()
                                },
                            )),
                            is_success: false,
                            build_result: Some(buck2_data::BuildResult {
                                build_completed: true,
                            }),
                        },
                    )),
                    ..Default::default()
                },
            )),
        );
        let command_finished = command_end_events
            .iter()
            .find(|event| is_build_finished_event(event))
            .expect("command-end BuildFinished");
        assert_eq!(command_finished.children, vec![build_tool_logs_id()]);

        let final_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Failed as i32),
                            exit_code: Some(32),
                            exit_result_name: Some("TESTS_FAILED".to_owned()),
                            command_name: Some("test".to_owned()),
                            analysis_count: Some(1),
                            run_local_count: 1,
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let updated_finished = final_events
            .iter()
            .find(|event| is_build_finished_event(event))
            .expect("updated BuildFinished");
        assert!(
            updated_finished
                .children
                .iter()
                .any(|child| child == &build_tool_logs_id())
        );
        assert!(
            updated_finished
                .children
                .iter()
                .any(|child| child == &build_metrics_id())
        );
        let Some(build_event::Payload::Finished(finished)) = updated_finished.payload.as_ref()
        else {
            panic!("expected BuildFinished payload");
        };
        let exit_code = finished.exit_code.as_ref().expect("exit code");
        assert_eq!(exit_code.code, 32);
        assert_eq!(exit_code.name, "TESTS_FAILED");
        assert!(
            final_events
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::BuildMetrics(_))))
        );
        assert!(!final_events.iter().any(|event| {
            event.id.as_ref() == Some(&build_metrics_id())
                && matches!(event.payload, Some(build_event::Payload::Aborted(_)))
        }));

        let logs = final_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildToolLogs(logs)) => Some(logs),
                _ => None,
            })
            .expect("updated build tool logs");
        let contents = match logs.log[0].file.as_ref() {
            Some(bep::file::File::Contents(contents)) => contents,
            other => panic!("expected inline invocation summary, got {other:?}"),
        };
        let summary: serde_json::Value =
            serde_json::from_slice(contents).expect("valid summary json");
        assert_eq!(summary["command"], "test");
        assert_eq!(summary["exit_result_name"], "TESTS_FAILED");
        assert_eq!(summary["actions"]["local"], 1);
    }

    #[test]
    fn finished_aborts_announced_events_without_payloads() {
        let mut converter = BazelEventConverter::default();
        let first_events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::ConsoleMessage(
                        buck2_data::ConsoleMessage {
                            message: "starting".to_owned(),
                        },
                    )),
                },
            )),
        );
        assert!(matches!(
            first_events[0].payload,
            Some(build_event::Payload::Started(_))
        ));

        let final_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Failed as i32),
                            exit_code: Some(1),
                            exit_result_name: Some("FAILED".to_owned()),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let aborted_index = final_events
            .iter()
            .position(|event| event.id.as_ref() == Some(&unstructured_command_line_id()))
            .expect("aborted unstructured command line event");
        let Some(build_event::Payload::Aborted(aborted)) =
            final_events[aborted_index].payload.as_ref()
        else {
            panic!("expected Aborted payload");
        };
        assert_eq!(aborted.reason, bep::aborted::AbortReason::Incomplete as i32);
        assert!(aborted.description.contains("announced by Buck2"));

        let finished_index = final_events
            .iter()
            .position(is_build_finished_event)
            .expect("build finished event");
        let logs_index = final_events
            .iter()
            .position(|event| {
                matches!(
                    event.payload.as_ref(),
                    Some(build_event::Payload::BuildToolLogs(_))
                )
            })
            .expect("build tool logs event");
        assert!(finished_index < aborted_index);
        assert!(aborted_index < logs_index);
        assert_eq!(logs_index, final_events.len() - 1);
        assert!(!final_events[finished_index].last_message);
        assert!(final_events[logs_index].last_message);
        assert!(final_events.iter().any(|event| {
            event.id.as_ref() == Some(&progress_id(1))
                && matches!(
                    event.payload.as_ref(),
                    Some(build_event::Payload::Progress(_))
                )
        }));
        assert!(!final_events.iter().any(|event| {
            event.id.as_ref() == Some(&progress_id(1))
                && matches!(
                    event.payload.as_ref(),
                    Some(build_event::Payload::Aborted(_))
                )
        }));
    }

    #[test]
    fn command_profile_includes_buck_spans_and_lanes() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();
        let command_start = trace_event_at(
            1,
            0,
            0,
            buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::Command(
                    buck2_data::CommandStart {
                        cli_args: vec![
                            "buck2".to_owned(),
                            "build".to_owned(),
                            "//:main".to_owned(),
                        ],
                        data: Some(buck2_data::command_start::Data::Build(
                            buck2_data::BuildCommandStart {},
                        )),
                        ..Default::default()
                    },
                )),
            }),
        );
        converter.convert(1, &command_start);

        let analysis_start = trace_event_at(
            2,
            1,
            1_000,
            buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::Analysis(
                    buck2_data::AnalysisStart {
                        target: Some(buck2_data::analysis_start::Target::StandardTarget(
                            target.clone(),
                        )),
                        rule: "genrule".to_owned(),
                    },
                )),
            }),
        );
        converter.convert(2, &analysis_start);

        let analysis_end = trace_event_at(
            2,
            1,
            11_000,
            buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
                data: Some(buck2_data::span_end_event::Data::Analysis(
                    buck2_data::AnalysisEnd {
                        target: Some(buck2_data::analysis_end::Target::StandardTarget(
                            target.clone(),
                        )),
                        rule: "genrule".to_owned(),
                        declared_actions: Some(1),
                        ..Default::default()
                    },
                )),
                duration: Some(prost_types::Duration {
                    seconds: 0,
                    nanos: 10_000_000,
                }),
                ..Default::default()
            }),
        );
        converter.convert(3, &analysis_end);

        let action_key = buck2_data::ActionKey {
            owner: Some(buck2_data::action_key::Owner::TargetLabel(target.clone())),
            key: "action-key".to_owned(),
            ..Default::default()
        };
        let action_name = buck2_data::ActionName {
            category: "write".to_owned(),
            identifier: "main".to_owned(),
        };
        let action_start = trace_event_at(
            3,
            1,
            12_000,
            buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::ActionExecution(
                    buck2_data::ActionExecutionStart {
                        key: Some(action_key.clone()),
                        kind: buck2_data::ActionKind::Write as i32,
                        name: Some(action_name.clone()),
                    },
                )),
            }),
        );
        converter.convert(4, &action_start);

        let action_end = trace_event_at(
            3,
            1,
            32_000,
            buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
                data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                    buck2_data::ActionExecutionEnd {
                        key: Some(action_key),
                        kind: buck2_data::ActionKind::Write as i32,
                        name: Some(action_name),
                        execution_kind: buck2_data::ActionExecutionKind::Local as i32,
                        output_size: 42,
                        ..Default::default()
                    },
                ))),
                duration: Some(prost_types::Duration {
                    seconds: 0,
                    nanos: 20_000_000,
                }),
                ..Default::default()
            }),
        );
        converter.convert(5, &action_end);

        let command_end = trace_event_at(
            1,
            0,
            50_000,
            buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
                data: Some(buck2_data::span_end_event::Data::Command(
                    buck2_data::CommandEnd {
                        data: Some(buck2_data::command_end::Data::Build(
                            buck2_data::BuildCommandEnd::default(),
                        )),
                        is_success: true,
                        ..Default::default()
                    },
                )),
                duration: Some(prost_types::Duration {
                    seconds: 0,
                    nanos: 50_000_000,
                }),
                ..Default::default()
            }),
        );
        let events = converter.convert(6, &command_end);
        let logs = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildToolLogs(logs)) => Some(logs),
                _ => None,
            })
            .expect("build tool logs event");
        let profile_json = decode_profile_from_logs(logs);
        let trace_events = profile_json["traceEvents"]
            .as_array()
            .expect("trace events array");
        assert!(
            trace_events
                .iter()
                .filter(|event| event["ph"] == "X")
                .count()
                >= 3
        );
        assert!(
            trace_events
                .iter()
                .any(|event| { event["ph"] == "M" && event["args"]["name"] == "Buck2 command" })
        );
        assert!(
            trace_events
                .iter()
                .any(|event| { event["ph"] == "M" && event["args"]["name"] == "Buck2 analysis" })
        );
        assert!(
            trace_events
                .iter()
                .any(|event| event["ph"] == "M" && event["args"]["name"] == "Buck2 actions")
        );
        assert!(
            trace_events
                .iter()
                .any(|event| event["name"] == "analysis //pkg:main")
        );
        assert!(
            trace_events
                .iter()
                .any(|event| event["name"] == "write:main //pkg:main")
        );
    }

    #[test]
    fn invocation_record_emits_build_finished_when_command_end_did_not() {
        let mut converter = BazelEventConverter::default();

        let final_events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Failed as i32),
                            exit_code: Some(32),
                            exit_result_name: Some("TESTS_FAILED".to_owned()),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let finished = final_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::Finished(finished)) => Some(finished),
                _ => None,
            })
            .expect("build finished");
        assert!(!finished.overall_success);
        let exit_code = finished.exit_code.as_ref().expect("exit code");
        assert_eq!(exit_code.code, 32);
        assert_eq!(exit_code.name, "TESTS_FAILED");
    }

    #[test]
    fn re_session_emits_remote_execution_metadata() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::ReSession(
                        buck2_data::RemoteExecutionSessionCreated {
                            session_id: "re-session".to_owned(),
                            experiment_name: String::new(),
                            persistent_cache_mode: None,
                        },
                    )),
                },
            )),
        );

        let metadata = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildMetadata(metadata)) => Some(metadata),
                _ => None,
            })
            .expect("build metadata");
        assert_eq!(
            metadata
                .metadata
                .get("REMOTE_EXECUTION_ENABLED")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            metadata
                .metadata
                .get("BUCK2_RE_SESSION_ID")
                .map(String::as_str),
            Some("re-session")
        );
    }

    #[test]
    fn invocation_record_emits_version_control_metadata() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            hg_revision: Some("abcdef123456".to_owned()),
                            has_local_changes: Some(true),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let metadata = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildMetadata(metadata)) => Some(metadata),
                _ => None,
            })
            .expect("build metadata");
        assert_eq!(
            metadata.metadata.get("COMMIT_SHA").map(String::as_str),
            Some("abcdef123456")
        );
        assert_eq!(
            metadata
                .metadata
                .get("HAS_LOCAL_CHANGES")
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn non_command_event_synthesizes_started() {
        let mut converter = BazelEventConverter::default();
        let event = trace_event(buck2_data::buck_event::Data::Instant(
            buck2_data::InstantEvent {
                data: Some(buck2_data::instant_event::Data::ConsoleMessage(
                    buck2_data::ConsoleMessage {
                        message: "hello".to_owned(),
                    },
                )),
            },
        ));

        let events = converter.convert(1, &event);
        assert!(matches!(
            events[0].payload,
            Some(build_event::Payload::Started(_))
        ));
        assert!(
            events[0]
                .children
                .iter()
                .any(|child| child == &progress_id(0))
        );
        assert!(matches!(
            events[1].payload,
            Some(build_event::Payload::Progress(_))
        ));
        assert_eq!(events[1].id.as_ref(), Some(&progress_id(0)));
        assert_eq!(events[1].children, vec![progress_id(1)]);
    }

    #[test]
    fn buck_cell_labels_are_normalized_for_bazel() {
        assert_eq!(normalize_buck_label("//:main").as_deref(), Some("//:main"));
        assert_eq!(
            normalize_buck_label("//root//:main").as_deref(),
            Some("//:main")
        );
        assert_eq!(
            normalize_buck_label("root//pkg:main").as_deref(),
            Some("//pkg:main")
        );
        assert_eq!(
            normalize_buck_label("//prelude//cpu:x86_64").as_deref(),
            Some("@prelude//cpu:x86_64")
        );
        assert_eq!(
            label_for_target(&buck2_data::TargetLabel {
                package: "root//pkg".to_owned(),
                name: "main".to_owned(),
            })
            .as_deref(),
            Some("//pkg:main")
        );
    }

    #[test]
    fn final_completed_update_links_actions_to_target() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target_with_package("root//", "main", "cfg");

        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Analysis(
                        buck2_data::AnalysisEnd {
                            rule: "cxx_binary".to_owned(),
                            target: Some(buck2_data::analysis_end::Target::StandardTarget(
                                target.clone(),
                            )),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                },
            )),
        );
        let action = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                        buck2_data::ActionExecutionEnd {
                            key: Some(buck2_data::ActionKey {
                                key: "action-key".to_owned(),
                                owner: Some(buck2_data::action_key::Owner::TargetLabel(
                                    target.clone(),
                                )),
                                ..Default::default()
                            }),
                            outputs: vec![buck2_data::ActionOutput {
                                tiny_digest: "buck-out/main.o".to_owned(),
                                path: "buck-out/main.o".to_owned(),
                                digest: "abcd:10".to_owned(),
                                size: 10,
                                is_directory: false,
                            }],
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                },
            )),
        );
        let final_events = converter.convert(
            3,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let Some(build_event_id::Id::ActionCompleted(action_id)) =
            action[0].id.as_ref().and_then(|id| id.id.as_ref())
        else {
            panic!("expected ActionCompleted id");
        };
        assert_eq!(action_id.label, "//:main");
        let Some(build_event::Payload::Action(action_payload)) = action[0].payload.as_ref() else {
            panic!("expected ActionExecuted payload");
        };
        let primary_output = action_payload
            .primary_output
            .as_ref()
            .expect("successful action primary output");
        assert_eq!(primary_output.name, "buck-out/main.o");
        assert_eq!(primary_output.digest, "abcd:10");
        assert_eq!(primary_output.length, 10);
        assert!(primary_output.file.is_none());

        let completed_update = final_events
            .iter()
            .find(|event| {
                matches!(
                    event.id.as_ref().and_then(|id| id.id.as_ref()),
                    Some(build_event_id::Id::TargetCompleted(_))
                ) && !event.children.is_empty()
            })
            .expect("expected TargetComplete update with action children");
        let Some(build_event_id::Id::TargetCompleted(completed_id)) =
            completed_update.id.as_ref().and_then(|id| id.id.as_ref())
        else {
            panic!("expected TargetCompleted id");
        };
        assert_eq!(completed_id.label, "//:main");
        assert!(
            completed_update
                .children
                .iter()
                .any(|child| child == &action[0].id.clone().unwrap())
        );
        assert!(
            completed_update
                .children
                .iter()
                .any(|child| matches!(child.id, Some(build_event_id::Id::TargetSummary(_))))
        );
        let Some(build_event::Payload::Completed(completed)) = completed_update.payload.as_ref()
        else {
            panic!("expected TargetComplete payload");
        };
        assert_eq!(completed.output_group.len(), 1);
        assert_eq!(completed.important_output.len(), 1);
        let important_output = &completed.important_output[0];
        assert_eq!(important_output.name, "buck-out/main.o");
        assert_eq!(important_output.digest, "abcd:10");
        assert_eq!(important_output.length, 10);
        let target_summary = final_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::TargetSummary(summary)) => Some(summary),
                _ => None,
            })
            .expect("target summary event");
        assert!(target_summary.overall_build_success);
        assert_eq!(
            target_summary.overall_test_status,
            bep::TestStatus::NoStatus as i32
        );
        assert!(final_events.iter().any(|event| matches!(
            event.payload,
            Some(build_event::Payload::NamedSetOfFiles(_))
        )));
    }

    #[test]
    fn final_completed_update_reports_directory_outputs() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target_with_package("root//", "main", "cfg");

        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Analysis(
                        buck2_data::AnalysisEnd {
                            target: Some(buck2_data::analysis_end::Target::StandardTarget(
                                target.clone(),
                            )),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                },
            )),
        );
        converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                        buck2_data::ActionExecutionEnd {
                            key: Some(buck2_data::ActionKey {
                                key: "directory-action".to_owned(),
                                owner: Some(buck2_data::action_key::Owner::TargetLabel(target)),
                                ..Default::default()
                            }),
                            outputs: vec![buck2_data::ActionOutput {
                                tiny_digest: "buck-out/main.resources".to_owned(),
                                path: "buck-out/main.resources".to_owned(),
                                digest: "tree-digest:42".to_owned(),
                                size: 42,
                                is_directory: true,
                            }],
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                },
            )),
        );
        let final_events = converter.convert(
            3,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let completed = final_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::Completed(completed))
                    if !completed.directory_output.is_empty() =>
                {
                    Some(completed)
                }
                _ => None,
            })
            .expect("TargetComplete with directory output");
        assert_eq!(completed.directory_output.len(), 1);
        let directory = &completed.directory_output[0];
        assert_eq!(directory.name, "buck-out/main.resources");
        assert_eq!(directory.digest, "tree-digest:42");
        assert_eq!(directory.length, 42);
        assert!(directory.file.is_none());
        assert_eq!(completed.output_group.len(), 1);
        assert_eq!(completed.important_output.len(), 1);
        assert_eq!(
            completed.important_output[0].name,
            "buck-out/main.resources"
        );
        let named_set = final_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::NamedSetOfFiles(named_set)) => Some(named_set),
                _ => None,
            })
            .expect("NamedSetOfFiles event");
        assert_eq!(named_set.files.len(), 1);
        assert_eq!(named_set.files[0].name, "buck-out/main.resources");
        assert_eq!(named_set.files[0].digest, "tree-digest:42");
        assert_eq!(named_set.files[0].length, 42);
        assert!(named_set.files[0].file.is_none());
    }

    #[test]
    fn final_completed_update_links_tests_to_target() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();

        let test_events = convert_through_end_of_results(
            &mut converter,
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::TestRun(Box::new(
                        buck2_data::TestRunEnd {
                            suite: Some(buck2_data::TestSuite {
                                suite_name: "suite".to_owned(),
                                test_names: vec!["test_passes".to_owned()],
                                target_label: Some(target.clone()),
                                labels: vec!["ci".to_owned(), "small".to_owned()],
                            }),
                            command_report: Some(buck2_data::CommandExecution {
                                details: Some(buck2_data::CommandExecutionDetails {
                                    signed_exit_code: Some(0),
                                    ..Default::default()
                                }),
                                status: Some(buck2_data::command_execution::Status::Success(
                                    buck2_data::command_execution::Success {},
                                )),
                                ..Default::default()
                            }),
                            timeout: Some(prost_types::Duration {
                                seconds: 45,
                                nanos: 0,
                            }),
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                },
            )),
        );
        let test_result_id = test_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::TestResult(_)) => event.id.clone(),
                _ => None,
            })
            .expect("expected TestResult event");
        let test_summary_id = test_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::TestSummary(_)) => event.id.clone(),
                _ => None,
            })
            .expect("expected TestSummary event");

        let final_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let completed_update = final_events
            .iter()
            .find(|event| {
                matches!(
                    event.id.as_ref().and_then(|id| id.id.as_ref()),
                    Some(build_event_id::Id::TargetCompleted(_))
                ) && !event.children.is_empty()
            })
            .expect("expected TargetComplete update with test children");
        assert!(
            completed_update
                .children
                .iter()
                .any(|child| child == &test_result_id)
        );
        assert!(
            completed_update
                .children
                .iter()
                .any(|child| child == &test_summary_id)
        );
        let Some(build_event::Payload::Completed(completed)) = completed_update.payload.as_ref()
        else {
            panic!("expected TargetComplete payload");
        };
        assert_eq!(completed.tag, vec!["ci", "small"]);
        assert_eq!(completed.test_timeout_seconds, 45);
        assert_eq!(
            completed
                .test_timeout
                .as_ref()
                .map(|timeout| timeout.seconds),
            Some(45)
        );
        let target_summary = final_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::TargetSummary(summary)) => Some(summary),
                _ => None,
            })
            .expect("target summary event");
        assert!(target_summary.overall_build_success);
        assert_eq!(
            target_summary.overall_test_status,
            bep::TestStatus::Passed as i32
        );
    }

    #[test]
    fn remote_cached_tests_report_cache_metadata() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();

        let test_events = convert_through_end_of_results(&mut converter,
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::TestRun(
                        Box::new(buck2_data::TestRunEnd {
                            suite: Some(buck2_data::TestSuite {
                                suite_name: "suite".to_owned(),
                                test_names: vec!["test_passes".to_owned()],
                                target_label: Some(target),
                                labels: Vec::new(),
                            }),
                            command_report: Some(buck2_data::CommandExecution {
                                details: Some(buck2_data::CommandExecutionDetails {
                                    signed_exit_code: Some(0),
                                    command_kind: Some(buck2_data::CommandExecutionKind {
                                        command: Some(
                                            buck2_data::command_execution_kind::Command::RemoteCommand(
                                                buck2_data::RemoteCommand {
                                                    cache_hit: true,
                                                    ..Default::default()
                                                },
                                            ),
                                        ),
                                    }),
                                    ..Default::default()
                                }),
                                status: Some(buck2_data::command_execution::Status::Success(
                                    buck2_data::command_execution::Success {},
                                )),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                    )),
                    ..Default::default()
                },
            )),
        );

        let result = test_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::TestResult(result)) => Some(result),
                _ => None,
            })
            .expect("expected TestResult event");
        assert!(
            result
                .execution_info
                .as_ref()
                .expect("test execution info")
                .cached_remotely
        );

        let summary = test_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::TestSummary(summary)) => Some(summary),
                _ => None,
            })
            .expect("expected TestSummary event");
        assert_eq!(summary.total_num_cached, 1);
    }

    fn test_run_end_event(
        target: &buck2_data::ConfiguredTargetLabel,
        exit_code: i32,
    ) -> buck2_data::BuckEvent {
        let status = if exit_code == 0 {
            buck2_data::command_execution::Status::Success(
                buck2_data::command_execution::Success {},
            )
        } else {
            buck2_data::command_execution::Status::Failure(
                buck2_data::command_execution::Failure::default(),
            )
        };
        trace_event(buck2_data::buck_event::Data::SpanEnd(
            buck2_data::SpanEndEvent {
                data: Some(buck2_data::span_end_event::Data::TestRun(Box::new(
                    buck2_data::TestRunEnd {
                        suite: Some(buck2_data::TestSuite {
                            suite_name: "suite".to_owned(),
                            test_names: Vec::new(),
                            target_label: Some(target.clone()),
                            labels: Vec::new(),
                        }),
                        command_report: Some(buck2_data::CommandExecution {
                            details: Some(buck2_data::CommandExecutionDetails {
                                signed_exit_code: Some(exit_code),
                                ..Default::default()
                            }),
                            status: Some(status),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ))),
                ..Default::default()
            },
        ))
    }

    fn test_case_event(
        target: &buck2_data::ConfiguredTargetLabel,
        status: buck2_data::TestStatus,
        msg: &str,
    ) -> buck2_data::BuckEvent {
        trace_event(buck2_data::buck_event::Data::Instant(
            buck2_data::InstantEvent {
                data: Some(buck2_data::instant_event::Data::TestResult(
                    buck2_data::TestResult {
                        name: "test_fails".to_owned(),
                        status: status as i32,
                        msg: Some(buck2_data::test_result::OptionalMsg {
                            msg: msg.to_owned(),
                        }),
                        target_label: Some(target.clone()),
                        ..Default::default()
                    },
                )),
            },
        ))
    }

    fn end_of_test_results_event() -> buck2_data::BuckEvent {
        trace_event(buck2_data::buck_event::Data::Instant(
            buck2_data::InstantEvent {
                data: Some(buck2_data::instant_event::Data::EndOfTestResults(
                    buck2_data::EndOfTestResults::default(),
                )),
            },
        ))
    }

    fn test_discovery_event(target: &buck2_data::ConfiguredTargetLabel) -> buck2_data::BuckEvent {
        trace_event(buck2_data::buck_event::Data::SpanStart(
            buck2_data::SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::TestDiscovery(
                    buck2_data::TestDiscoveryStart {
                        suite_name: "suite".to_owned(),
                        target_label: Some(target.clone()),
                        labels: Vec::new(),
                    },
                )),
            },
        ))
    }

    fn count_test_events(events: &[bep::BuildEvent]) -> (usize, usize) {
        let results = events
            .iter()
            .filter(|e| matches!(e.payload, Some(build_event::Payload::TestResult(_))))
            .count();
        let summaries = events
            .iter()
            .filter(|e| matches!(e.payload, Some(build_event::Payload::TestSummary(_))))
            .count();
        (results, summaries)
    }

    /// buck2's test runner reports a target's per-test result after the run's span ends
    /// (buck2_test_runner src/runner.rs), so the summary must wait for it to carry its log.
    #[test]
    fn test_run_waits_for_its_test_result_reported_after_the_span_end() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();

        let at_end = converter.convert(1, &test_run_end_event(&target, 1));
        assert_eq!(
            count_test_events(&at_end),
            (0, 0),
            "TestResult and TestSummary went out before the run's test result arrived"
        );

        let at_case = converter.convert(
            2,
            &test_case_event(&target, buck2_data::TestStatus::Fail, "expected true"),
        );
        assert_eq!(count_test_events(&at_case), (1, 1));
        let summary = at_case
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::TestSummary(summary)) => Some(summary),
                _ => None,
            })
            .expect("TestSummary");
        let log = summary
            .failed
            .iter()
            .find(|file| file.name == "test.log")
            .expect("test summary log");
        let Some(bep::file::File::Contents(contents)) = log.file.as_ref() else {
            panic!("expected an inline test log, got {:?}", log.file);
        };
        assert!(
            std::str::from_utf8(contents)
                .expect("utf8")
                .contains("test_fails: expected true"),
            "the summary's log is missing the test case reported after the span end"
        );

        let at_end_of_results = converter.convert(3, &end_of_test_results_event());
        assert_eq!(count_test_events(&at_end_of_results), (0, 0), "sent twice");
        assert!(converter.held_test_ends.is_empty());
        assert!(
            converter.test_cases.is_empty(),
            "a case was kept after its summary"
        );
    }

    /// A test command's PatternExpanded goes out with its first target event, so a test target
    /// configured later needs an announcement of its own for BuildBuddy's target tracker.
    #[test]
    fn test_target_configured_after_the_pattern_expansion_is_announced_first() {
        let mut converter = BazelEventConverter::default();
        let first = configured_target();
        let second = configured_target_with_package("pkg", "second", "cfg");
        let second_id = target_configured_id("//pkg:second".to_owned());

        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            cli_args: vec![
                                "buck2".to_owned(),
                                "test".to_owned(),
                                "//pkg/...".to_owned(),
                            ],
                            data: Some(buck2_data::command_start::Data::Test(
                                buck2_data::TestCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );
        let mut first_events = converter.convert(2, &test_discovery_event(&first));
        first_events.extend(converter.convert(3, &test_run_end_event(&first, 0)));
        first_events.extend(converter.convert(
            4,
            &test_case_event(&first, buck2_data::TestStatus::Pass, ""),
        ));
        let first_expansion = first_events
            .iter()
            .find(|event| matches!(event.payload, Some(build_event::Payload::Expanded(_))))
            .expect("the first PatternExpanded");
        assert!(!first_expansion.children.contains(&second_id));

        let later = converter.convert(5, &test_discovery_event(&second));
        let announced = later
            .iter()
            .position(|event| {
                matches!(event.payload, Some(build_event::Payload::Expanded(_)))
                    && event.children.contains(&second_id)
            })
            .expect("a PatternExpanded naming the late test target");
        let configured = later
            .iter()
            .position(|event| event.id.as_ref() == Some(&second_id))
            .expect("the late target's TargetConfigured");
        assert!(
            announced < configured,
            "announced after its TargetConfigured"
        );
    }

    /// Converts a test run's span end and then the stream's EndOfTestResults, the order a test
    /// command sends them in, and returns the events of both.
    fn convert_through_end_of_results(
        converter: &mut BazelEventConverter,
        sequence: i64,
        event: &buck2_data::BuckEvent,
    ) -> Vec<bep::BuildEvent> {
        let mut events = converter.convert(sequence, event);
        events.extend(converter.convert(
            sequence + 1,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::EndOfTestResults(
                        buck2_data::EndOfTestResults::default(),
                    )),
                },
            )),
        ));
        events
    }

    #[test]
    fn test_summary_includes_test_case_logs() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();
        let testcase = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::TestResult(
                        buck2_data::TestResult {
                            name: "test_fails".to_owned(),
                            status: buck2_data::TestStatus::Fail as i32,
                            msg: Some(buck2_data::test_result::OptionalMsg {
                                msg: "expected true".to_owned(),
                            }),
                            details: "assertion details".to_owned(),
                            target_label: Some(target.clone()),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );
        assert!(!testcase.iter().any(|event| matches!(
            event.payload,
            Some(build_event::Payload::TestResult(_)) | Some(build_event::Payload::TestSummary(_))
        )));

        let test_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::TestRun(Box::new(
                        buck2_data::TestRunEnd {
                            suite: Some(buck2_data::TestSuite {
                                suite_name: "suite".to_owned(),
                                test_names: vec!["test_fails".to_owned()],
                                target_label: Some(target),
                                labels: Vec::new(),
                            }),
                            command_report: Some(buck2_data::CommandExecution {
                                details: Some(buck2_data::CommandExecutionDetails {
                                    signed_exit_code: Some(1),
                                    ..Default::default()
                                }),
                                status: Some(buck2_data::command_execution::Status::Failure(
                                    buck2_data::command_execution::Failure::default(),
                                )),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                },
            )),
        );

        let summary = test_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::TestSummary(summary)) => Some(summary),
                _ => None,
            })
            .expect("expected TestSummary event");
        assert!(summary.passed.is_empty());
        let log = summary
            .failed
            .iter()
            .find(|file| file.name == "test.log")
            .expect("test summary log");
        let contents = match log.file.as_ref() {
            Some(bep::file::File::Contents(contents)) => contents,
            other => panic!("expected inline test log, got {other:?}"),
        };
        let contents = std::str::from_utf8(contents).expect("test log utf8");
        assert!(contents.contains("test_fails: expected true"));
    }

    #[test]
    fn test_result_details_are_streamed_to_progress() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();

        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::TestResult(
                        buck2_data::TestResult {
                            name: "gh_facebook_buck2//app/buck2_core:soft_error".to_owned(),
                            status: buck2_data::TestStatus::Pass as i32,
                            duration: Some(prost_types::Duration {
                                seconds: 0,
                                nanos: 30_000_000,
                            }),
                            details: "---- STDOUT ----\nrunning 2 tests\n---- STDERR ----\n"
                                .to_owned(),
                            target_label: Some(target.clone()),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );

        let progress = progress_payload(&events);
        assert_eq!(progress.stdout, "");
        assert!(
            progress
                .stderr
                .contains("✓ Pass: gh_facebook_buck2//app/buck2_core:soft_error (0.0s)")
        );
        assert!(progress.stderr.contains("---- STDOUT ----"));
        assert!(progress.stderr.contains("running 2 tests"));
        assert!(progress.stderr.contains("---- STDERR ----"));

        let without_details = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::TestResult(
                        buck2_data::TestResult {
                            name: "quiet_test".to_owned(),
                            status: buck2_data::TestStatus::Pass as i32,
                            target_label: Some(target),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );
        assert!(
            !without_details
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::Progress(_))))
        );
    }

    #[test]
    fn test_events_include_legacy_timing_millis() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();

        let test_events = convert_through_end_of_results(
            &mut converter,
            1,
            &trace_event_at(
                1,
                0,
                5_000_000,
                buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::TestRun(Box::new(
                        buck2_data::TestRunEnd {
                            suite: Some(buck2_data::TestSuite {
                                suite_name: "suite".to_owned(),
                                test_names: vec!["test_passes".to_owned()],
                                target_label: Some(target),
                                labels: Vec::new(),
                            }),
                            command_report: Some(buck2_data::CommandExecution {
                                details: Some(buck2_data::CommandExecutionDetails {
                                    signed_exit_code: Some(0),
                                    ..Default::default()
                                }),
                                status: Some(buck2_data::command_execution::Status::Success(
                                    buck2_data::command_execution::Success {},
                                )),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ))),
                    duration: Some(prost_types::Duration {
                        seconds: 2,
                        nanos: 500_000_000,
                    }),
                    ..Default::default()
                }),
            ),
        );

        let result = test_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::TestResult(result)) => Some(result),
                _ => None,
            })
            .expect("expected TestResult event");
        assert_eq!(result.test_attempt_start_millis_epoch, 3_500);
        assert_eq!(result.test_attempt_duration_millis, 2_500);

        let summary = test_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::TestSummary(summary)) => Some(summary),
                _ => None,
            })
            .expect("expected TestSummary event");
        assert_eq!(summary.first_start_time_millis, 3_500);
        assert_eq!(summary.last_stop_time_millis, 6_000);
        assert_eq!(summary.total_run_duration_millis, 2_500);
    }

    #[test]
    fn test_execution_info_includes_timing_breakdown() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();

        let test_events = convert_through_end_of_results(
            &mut converter,
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::TestRun(Box::new(
                        buck2_data::TestRunEnd {
                            suite: Some(buck2_data::TestSuite {
                                suite_name: "suite".to_owned(),
                                test_names: vec!["test_passes".to_owned()],
                                target_label: Some(target),
                                labels: Vec::new(),
                            }),
                            command_report: Some(buck2_data::CommandExecution {
                                details: Some(buck2_data::CommandExecutionDetails {
                                    signed_exit_code: Some(0),
                                    metadata: Some(buck2_data::CommandExecutionMetadata {
                                        wall_time: Some(prost_types::Duration {
                                            seconds: 3,
                                            nanos: 0,
                                        }),
                                        execution_time: Some(prost_types::Duration {
                                            seconds: 2,
                                            nanos: 0,
                                        }),
                                        input_materialization_duration: Some(
                                            prost_types::Duration {
                                                seconds: 0,
                                                nanos: 500_000_000,
                                            },
                                        ),
                                        queue_duration: Some(prost_types::Duration {
                                            seconds: 0,
                                            nanos: 400_000_000,
                                        }),
                                        hashing_duration: Some(prost_types::Duration {
                                            seconds: 0,
                                            nanos: 100_000_000,
                                        }),
                                        suspend_duration: Some(prost_types::Duration {
                                            seconds: 0,
                                            nanos: 50_000_000,
                                        }),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                }),
                                status: Some(buck2_data::command_execution::Status::Success(
                                    buck2_data::command_execution::Success {},
                                )),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                },
            )),
        );

        let result = test_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::TestResult(result)) => Some(result),
                _ => None,
            })
            .expect("expected TestResult event");
        let timing = result
            .execution_info
            .as_ref()
            .and_then(|info| info.timing_breakdown.as_ref())
            .expect("test execution timing breakdown");
        assert_eq!(timing.name, "totalTime");
        assert_eq!(timing.time_millis, 3_000);
        assert_eq!(timing.time.as_ref().unwrap().seconds, 3);

        let child_millis = |name: &str| {
            timing
                .child
                .iter()
                .find(|child| child.name == name)
                .map(|child| child.time_millis)
        };
        assert_eq!(child_millis("parseTime"), Some(0));
        assert_eq!(child_millis("fetchTime"), Some(0));
        assert_eq!(child_millis("queueTime"), Some(400));
        assert_eq!(child_millis("uploadTime"), Some(0));
        assert_eq!(child_millis("setupTime"), Some(0));
        assert_eq!(child_millis("inputMaterializationTime"), Some(500));
        assert_eq!(child_millis("executionWallTime"), Some(2_000));
        assert_eq!(child_millis("processOutputsTime"), Some(0));
        assert_eq!(child_millis("networkTime"), Some(0));
        assert_eq!(child_millis("hashingTime"), Some(100));
        assert_eq!(child_millis("suspendTime"), Some(50));
    }

    #[test]
    fn test_execution_info_includes_resource_usage() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();

        let test_events = convert_through_end_of_results(
            &mut converter,
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::TestRun(Box::new(
                        buck2_data::TestRunEnd {
                            suite: Some(buck2_data::TestSuite {
                                suite_name: "suite".to_owned(),
                                test_names: vec!["test_passes".to_owned()],
                                target_label: Some(target),
                                labels: Vec::new(),
                            }),
                            command_report: Some(buck2_data::CommandExecution {
                                details: Some(buck2_data::CommandExecutionDetails {
                                    signed_exit_code: Some(0),
                                    metadata: Some(buck2_data::CommandExecutionMetadata {
                                        execution_stats: Some(buck2_data::CommandExecutionStats {
                                            memory_peak: Some(123_456),
                                            cpu_instructions_user: Some(10),
                                            cpu_instructions_kernel: Some(20),
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                }),
                                status: Some(buck2_data::command_execution::Status::Success(
                                    buck2_data::command_execution::Success {},
                                )),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                },
            )),
        );

        let result = test_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::TestResult(result)) => Some(result),
                _ => None,
            })
            .expect("expected TestResult event");
        let execution_info = result.execution_info.as_ref().expect("test execution info");
        let usage_value = |name: &str| {
            execution_info
                .resource_usage
                .iter()
                .find(|usage| usage.name == name)
                .map(|usage| usage.value)
        };
        assert_eq!(usage_value("memory_peak_bytes"), Some(123_456));
        assert_eq!(usage_value("cpu_instructions_user"), Some(10));
        assert_eq!(usage_value("cpu_instructions_kernel"), Some(20));
    }

    #[test]
    fn analysis_only_target_completes_at_invocation_end() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target_with_package("root//", "main", "cfg");

        let analysis_end = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Analysis(
                        buck2_data::AnalysisEnd {
                            rule: "cxx_binary".to_owned(),
                            target: Some(buck2_data::analysis_end::Target::StandardTarget(target)),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                },
            )),
        );
        assert!(
            !analysis_end
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::Completed(_))))
        );

        let final_events = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );

        let completed = final_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::Completed(completed)) => Some(completed),
                _ => None,
            })
            .expect("analysis-only target completion");
        assert!(completed.success);
    }

    #[test]
    fn configured_targets_are_deduplicated() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target_with_package("root//", "main", "cfg");
        let event = trace_event(buck2_data::buck_event::Data::SpanStart(
            buck2_data::SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::Analysis(
                    buck2_data::AnalysisStart {
                        rule: "cxx_binary".to_owned(),
                        target: Some(buck2_data::analysis_start::Target::StandardTarget(target)),
                    },
                )),
            },
        ));

        let first = converter.convert(1, &event);
        let configured = first
            .iter()
            .find(|event| matches!(event.payload, Some(build_event::Payload::Configured(_))))
            .expect("configured target event");
        let Some(build_event_id::Id::TargetCompleted(completed_id)) =
            configured.children[0].id.as_ref()
        else {
            panic!("expected configured target to announce completion");
        };
        assert_eq!(completed_id.label, "//:main");
        assert_eq!(completed_id.configuration.as_ref().unwrap().id, "cfg");

        let second = converter.convert(2, &event);
        assert!(
            !second
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::Configured(_))))
        );
    }

    #[test]
    fn test_rule_configured_targets_have_default_test_size() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();
        let events = converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Analysis(
                        buck2_data::AnalysisStart {
                            rule: "prelude//rules.bzl:rust_test".to_owned(),
                            target: Some(buck2_data::analysis_start::Target::StandardTarget(
                                target,
                            )),
                        },
                    )),
                },
            )),
        );

        let configured = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::Configured(configured)) => Some(configured),
                _ => None,
            })
            .expect("configured target event");
        assert_eq!(configured.target_kind, "prelude//rules.bzl:rust_test rule");
        assert_eq!(configured.test_size, bep::TestSize::Medium as i32);
    }

    #[test]
    fn test_discovery_labels_update_configured_tags() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();

        let start = trace_event(buck2_data::buck_event::Data::SpanStart(
            buck2_data::SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::TestDiscovery(
                    buck2_data::TestDiscoveryStart {
                        suite_name: "suite".to_owned(),
                        target_label: Some(target.clone()),
                        labels: Vec::new(),
                    },
                )),
            },
        ));
        assert!(
            converter
                .convert(1, &start)
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::Configured(_))))
        );

        let end = trace_event(buck2_data::buck_event::Data::SpanEnd(
            buck2_data::SpanEndEvent {
                data: Some(buck2_data::span_end_event::Data::TestDiscovery(Box::new(
                    buck2_data::TestDiscoveryEnd {
                        suite_name: "suite".to_owned(),
                        target_label: Some(target),
                        labels: vec!["ci".to_owned(), "small".to_owned()],
                        ..Default::default()
                    },
                ))),
                ..Default::default()
            },
        ));
        let configured = converter
            .convert(2, &end)
            .into_iter()
            .find_map(|event| match event.payload {
                Some(build_event::Payload::Configured(configured)) => Some(configured),
                _ => None,
            })
            .expect("updated configured target event");
        assert_eq!(configured.tag, vec!["ci", "small"]);
        assert_eq!(configured.test_size, bep::TestSize::Small as i32);
    }

    #[test]
    fn target_action_and_test_events_use_stable_ids() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();

        converter.convert(
            1,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart {
                            data: Some(buck2_data::command_start::Data::Build(
                                buck2_data::BuildCommandStart {},
                            )),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );

        let configured = converter.convert(
            2,
            &trace_event(buck2_data::buck_event::Data::SpanStart(
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Analysis(
                        buck2_data::AnalysisStart {
                            rule: "cxx_binary".to_owned(),
                            target: Some(buck2_data::analysis_start::Target::StandardTarget(
                                target.clone(),
                            )),
                        },
                    )),
                },
            )),
        );
        let completed = converter.convert(
            3,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::Analysis(
                        buck2_data::AnalysisEnd {
                            rule: "cxx_binary".to_owned(),
                            target: Some(buck2_data::analysis_end::Target::StandardTarget(
                                target.clone(),
                            )),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                },
            )),
        );
        let action = converter.convert(
            4,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                        buck2_data::ActionExecutionEnd {
                            key: Some(buck2_data::ActionKey {
                                key: "action-key".to_owned(),
                                owner: Some(buck2_data::action_key::Owner::TargetLabel(
                                    target.clone(),
                                )),
                                ..Default::default()
                            }),
                            name: Some(buck2_data::ActionName {
                                category: "cxx_compile".to_owned(),
                                identifier: "main.cpp".to_owned(),
                            }),
                            failed: true,
                            outputs: vec![buck2_data::ActionOutput {
                                tiny_digest: "buck-out/main.o".to_owned(),
                                path: "buck-out/main.o".to_owned(),
                                digest: "abcd:10".to_owned(),
                                size: 10,
                                is_directory: false,
                            }],
                            commands: vec![buck2_data::CommandExecution {
                                details: Some(buck2_data::CommandExecutionDetails {
                                    signed_exit_code: Some(1),
                                    cmd_stdout: "compiler stdout".to_owned(),
                                    cmd_stderr: "compile failed".to_owned(),
                                    command_kind: Some(buck2_data::CommandExecutionKind {
                                        command: Some(
                                            buck2_data::command_execution_kind::Command::LocalCommand(
                                                buck2_data::LocalCommand {
                                                    argv: vec!["cc".to_owned(), "main.cpp".to_owned()],
                                                    ..Default::default()
                                                },
                                            ),
                                        ),
                                    }),
                                    ..Default::default()
                                }),
                                status: Some(buck2_data::command_execution::Status::Failure(
                                    buck2_data::command_execution::Failure::default(),
                                )),
                                ..Default::default()
                            }],
                            ..Default::default()
                        },
                    ))),
                    ..Default::default()
                },
            )),
        );
        let testcase = converter.convert(
            5,
            &trace_event(buck2_data::buck_event::Data::Instant(
                buck2_data::InstantEvent {
                    data: Some(buck2_data::instant_event::Data::TestResult(
                        buck2_data::TestResult {
                            name: "test_passes".to_owned(),
                            status: buck2_data::TestStatus::Pass as i32,
                            target_label: Some(target.clone()),
                            details: "passed".to_owned(),
                            ..Default::default()
                        },
                    )),
                },
            )),
        );
        assert!(!testcase.iter().any(|event| matches!(
            event.payload,
            Some(build_event::Payload::TestResult(_)) | Some(build_event::Payload::TestSummary(_))
        )));
        assert!(
            progress_payloads(&testcase)
                .iter()
                .any(|progress| progress.stderr.contains("✓ Pass: test_passes"))
        );
        let test_result = converter.convert(
            6,
            &trace_event(buck2_data::buck_event::Data::SpanEnd(
                buck2_data::SpanEndEvent {
                    data: Some(buck2_data::span_end_event::Data::TestRun(
                        Box::new(buck2_data::TestRunEnd {
                            suite: Some(buck2_data::TestSuite {
                                suite_name: "suite".to_owned(),
                                test_names: vec!["test_passes".to_owned()],
                                target_label: Some(target.clone()),
                                labels: Vec::new(),
                            }),
                            command_report: Some(buck2_data::CommandExecution {
                                details: Some(buck2_data::CommandExecutionDetails {
                                    signed_exit_code: Some(0),
                                    cmd_stdout: "test stdout".to_owned(),
                                    command_kind: Some(buck2_data::CommandExecutionKind {
                                        command: Some(
                                            buck2_data::command_execution_kind::Command::LocalCommand(
                                                buck2_data::LocalCommand {
                                                    argv: vec!["test-binary".to_owned()],
                                                    ..Default::default()
                                                },
                                            ),
                                        ),
                                    }),
                                    ..Default::default()
                                }),
                                status: Some(buck2_data::command_execution::Status::Success(
                                    buck2_data::command_execution::Success {},
                                )),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                    )),
                    ..Default::default()
                },
            )),
        );

        let Some(build_event_id::Id::TargetConfigured(configured_id)) =
            configured[0].id.as_ref().and_then(|id| id.id.as_ref())
        else {
            panic!("expected TargetConfigured id");
        };
        assert_eq!(configured_id.label, "//pkg:main");
        assert!(matches!(
            configured[0].payload,
            Some(build_event::Payload::Configured(_))
        ));

        assert!(
            !completed
                .iter()
                .any(|event| matches!(event.payload, Some(build_event::Payload::Completed(_))))
        );

        let Some(build_event_id::Id::ActionCompleted(action_id)) =
            action[0].id.as_ref().and_then(|id| id.id.as_ref())
        else {
            panic!("expected ActionCompleted id");
        };
        assert_eq!(action_id.label, "//pkg:main");
        assert_eq!(action_id.configuration.as_ref().unwrap().id, "cfg");
        assert!(matches!(
            action[0].payload,
            Some(build_event::Payload::Action(bep::ActionExecuted {
                success: false,
                ..
            }))
        ));
        let Some(build_event::Payload::Action(action_payload)) = action[0].payload.as_ref() else {
            panic!("expected ActionExecuted payload");
        };
        assert!(action_payload.primary_output.is_none());
        let action_stdout = match action_payload
            .stdout
            .as_ref()
            .and_then(|file| file.file.as_ref())
        {
            Some(bep::file::File::Contents(contents)) => contents,
            other => panic!("expected inline action stdout, got {other:?}"),
        };
        assert_eq!(
            std::str::from_utf8(action_stdout).expect("action stdout utf8"),
            "compiler stdout"
        );
        let action_stderr = match action_payload
            .stderr
            .as_ref()
            .and_then(|file| file.file.as_ref())
        {
            Some(bep::file::File::Contents(contents)) => contents,
            other => panic!("expected inline action stderr, got {other:?}"),
        };
        assert_eq!(
            std::str::from_utf8(action_stderr).expect("action stderr utf8"),
            "compile failed"
        );
        assert_eq!(
            action_payload
                .failure_detail
                .as_ref()
                .map(|detail| detail.message.as_str()),
            Some("Action failed for //pkg:main: compile failed")
        );
        assert_eq!(action_payload.strategy_details.len(), 1);
        assert_eq!(action_payload.strategy_details[0].type_url, STRUCT_TYPE_URL);
        let strategy_details = Struct::decode(action_payload.strategy_details[0].value.as_slice())
            .expect("strategy details struct");
        assert_eq!(
            struct_string_field(&strategy_details, "strategy"),
            Some("local")
        );
        assert_eq!(
            struct_string_field(&strategy_details, "runner"),
            Some("local")
        );
        assert_eq!(
            struct_string_field(&strategy_details, "label"),
            Some("//pkg:main")
        );
        assert_eq!(
            struct_string_field(&strategy_details, "mnemonic"),
            Some("cxx_compile")
        );
        assert_eq!(struct_string_field(&strategy_details, "argv0"), Some("cc"));
        assert_eq!(
            struct_number_field(&strategy_details, "exit_code"),
            Some(1.0)
        );

        let Some(build_event_id::Id::TestResult(test_id)) =
            test_result[0].id.as_ref().and_then(|id| id.id.as_ref())
        else {
            panic!("expected TestResult id");
        };
        assert_eq!(test_id.label, "//pkg:main");
        assert_eq!(test_id.configuration.as_ref().unwrap().id, "cfg");
        assert!(matches!(
            test_result[0].payload,
            Some(build_event::Payload::TestResult(bep::TestResult {
                status,
                ..
            })) if status == bep::TestStatus::Passed as i32
        ));
        let Some(build_event::Payload::TestResult(result)) = test_result[0].payload.as_ref() else {
            panic!("expected TestResult payload");
        };
        assert!(
            result
                .test_action_output
                .iter()
                .any(|file| file.name == "test.log")
        );
        assert!(
            result
                .test_action_output
                .iter()
                .any(|file| file.name == "test.xml")
        );
        let Some(build_event::Payload::TestSummary(summary)) = test_result[1].payload.as_ref()
        else {
            panic!("expected TestSummary payload");
        };
        let summary_log = summary
            .passed
            .iter()
            .find(|file| file.name == "test.log")
            .and_then(|file| file.file.as_ref())
            .expect("test summary log");
        let bep::file::File::Contents(summary_log) = summary_log else {
            panic!("expected inline test summary log");
        };
        assert!(
            std::str::from_utf8(summary_log)
                .expect("test summary log utf8")
                .contains("test_passes: passed")
        );

        let final_events = converter.convert(
            7,
            &trace_event(buck2_data::buck_event::Data::Record(
                buck2_data::RecordEvent {
                    data: Some(buck2_data::record_event::Data::InvocationRecord(Box::new(
                        buck2_data::InvocationRecord {
                            outcome: Some(buck2_data::InvocationOutcome::Failed as i32),
                            exit_code: Some(1),
                            exit_result_name: Some("FAILED".to_owned()),
                            ..Default::default()
                        },
                    ))),
                },
            )),
        );
        let completed = final_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::Completed(completed)) if !event.children.is_empty() => {
                    Some(completed)
                }
                _ => None,
            })
            .expect("failed target completion");
        assert_eq!(
            completed
                .failure_detail
                .as_ref()
                .map(|detail| detail.message.as_str()),
            Some("Target failed: //pkg:main")
        );
        let finished = final_events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::Finished(finished)) => Some(finished),
                _ => None,
            })
            .expect("build finished");
        assert_eq!(
            finished
                .failure_detail
                .as_ref()
                .map(|detail| detail.message.as_str()),
            Some("Buck2 invocation failed with FAILED (1)")
        );
    }

    fn command_end_event(
        data: buck2_data::command_end::Data,
        is_success: bool,
        build_completed: Option<bool>,
    ) -> buck2_data::BuckEvent {
        trace_event(buck2_data::buck_event::Data::SpanEnd(
            buck2_data::SpanEndEvent {
                data: Some(buck2_data::span_end_event::Data::Command(
                    buck2_data::CommandEnd {
                        data: Some(data),
                        is_success,
                        build_result: build_completed
                            .map(|build_completed| buck2_data::BuildResult { build_completed }),
                    },
                )),
                ..Default::default()
            },
        ))
    }

    fn end_of_test_results_with_exit_code(exit_code: i32) -> buck2_data::BuckEvent {
        trace_event(buck2_data::buck_event::Data::Instant(
            buck2_data::InstantEvent {
                data: Some(buck2_data::instant_event::Data::EndOfTestResults(
                    buck2_data::EndOfTestResults { exit_code },
                )),
            },
        ))
    }

    /// The BuildFinished exit code, its name, and overall_success, and the outcome and exit code
    /// in buck2-invocation.json, which has to say the same.
    fn finished_outcome(events: &[bep::BuildEvent]) -> (i32, String, bool, serde_json::Value) {
        let finished = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::Finished(finished)) => Some(finished),
                _ => None,
            })
            .expect("BuildFinished");
        let exit_code = finished.exit_code.as_ref().expect("exit code");
        let summary = events
            .iter()
            .find_map(|event| match event.payload.as_ref() {
                Some(build_event::Payload::BuildToolLogs(logs)) => logs
                    .log
                    .iter()
                    .find(|file| file.name == "buck2-invocation.json"),
                _ => None,
            })
            .and_then(|file| match file.file.as_ref() {
                Some(bep::file::File::Contents(contents)) => {
                    serde_json::from_slice::<serde_json::Value>(contents).ok()
                }
                _ => None,
            })
            .expect("buck2-invocation.json");
        (
            exit_code.code,
            exit_code.name.clone(),
            finished.overall_success,
            serde_json::json!([summary["outcome"], summary["exit_code"]]),
        )
    }

    /// A build with failed targets returns Ok with the failures in BuildResponse.errors
    /// (buck2_server_commands build.rs), so `is_success` is true and only `build_completed`
    /// says the build failed.
    #[test]
    fn build_with_errors_finishes_as_build_failure() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &command_end_event(
                buck2_data::command_end::Data::Build(buck2_data::BuildCommandEnd::default()),
                true,
                Some(false),
            ),
        );
        assert_eq!(
            finished_outcome(&events),
            (
                1,
                "BUILD_FAILURE".to_owned(),
                false,
                serde_json::json!(["failed", 1])
            )
        );
    }

    #[test]
    fn clean_build_finishes_as_success() {
        let mut converter = BazelEventConverter::default();
        let events = converter.convert(
            1,
            &command_end_event(
                buck2_data::command_end::Data::Build(buck2_data::BuildCommandEnd::default()),
                true,
                Some(true),
            ),
        );
        assert_eq!(
            finished_outcome(&events),
            (
                0,
                "SUCCESS".to_owned(),
                true,
                serde_json::json!(["success", 0])
            )
        );
    }

    /// A test run whose tests fail still returns Ok with every target built, so the failure is
    /// only in the test events: the executor's exit code, which `buck2 test` exits with.
    #[test]
    fn failed_test_run_finishes_as_tests_failed() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();
        converter.convert(
            1,
            &test_case_event(&target, buck2_data::TestStatus::Fail, "expected true"),
        );
        converter.convert(2, &end_of_test_results_with_exit_code(32));
        let events = converter.convert(
            3,
            &command_end_event(
                buck2_data::command_end::Data::Test(buck2_data::TestCommandEnd::default()),
                true,
                Some(true),
            ),
        );
        assert_eq!(
            finished_outcome(&events),
            (
                3,
                "TESTS_FAILED".to_owned(),
                false,
                serde_json::json!(["failed", 3])
            )
        );
    }

    /// Without an EndOfTestResults, as when the executor dies before sending it, a failing
    /// result is what is left to go on.
    #[test]
    fn failing_test_result_without_end_of_results_finishes_as_tests_failed() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();
        converter.convert(
            1,
            &test_case_event(&target, buck2_data::TestStatus::Timeout, "timed out"),
        );
        let events = converter.convert(
            2,
            &command_end_event(
                buck2_data::command_end::Data::Test(buck2_data::TestCommandEnd::default()),
                true,
                Some(true),
            ),
        );
        assert_eq!(finished_outcome(&events).1, "TESTS_FAILED");
    }

    #[test]
    fn passing_test_run_finishes_as_success() {
        let mut converter = BazelEventConverter::default();
        let target = configured_target();
        converter.convert(
            1,
            &test_case_event(&target, buck2_data::TestStatus::Pass, ""),
        );
        converter.convert(2, &end_of_test_results_with_exit_code(0));
        let events = converter.convert(
            3,
            &command_end_event(
                buck2_data::command_end::Data::Test(buck2_data::TestCommandEnd::default()),
                true,
                Some(true),
            ),
        );
        assert_eq!(finished_outcome(&events).1, "SUCCESS");
    }

    /// A test whose targets fail to build is a build failure, as in Bazel, whatever the tests
    /// that did run reported.
    #[test]
    fn test_run_with_build_errors_finishes_as_build_failure() {
        let mut converter = BazelEventConverter::default();
        converter.convert(1, &end_of_test_results_with_exit_code(32));
        let events = converter.convert(
            2,
            &command_end_event(
                buck2_data::command_end::Data::Test(buck2_data::TestCommandEnd::default()),
                true,
                Some(false),
            ),
        );
        assert_eq!(finished_outcome(&events).1, "BUILD_FAILURE");
    }
}
