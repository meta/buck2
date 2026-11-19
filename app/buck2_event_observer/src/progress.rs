/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_data::ActionExecutionStart;
use buck2_data::AnalysisEnd;
use buck2_data::AnalysisStageStart;
use buck2_data::AnalysisStart;
use buck2_data::ExecutorStageStart;
use buck2_data::LoadBuildFileEnd;
use buck2_data::LocalStage;
use buck2_data::ReStage;
use buck2_data::analysis_stage_start;
use buck2_data::executor_stage_start;
use buck2_data::instant_event;
use buck2_data::local_stage;
use buck2_data::re_stage;
use buck2_data::span_end_event;
use buck2_data::span_start_event;
use buck2_events::BuckEvent;
use buck2_events::span::SpanId;
use buck2_hash::BuckMutMap;

use crate::last_command_execution_kind::get_last_command_execution_time;
use crate::unpack_event::UnpackedBuckEvent;
use crate::unpack_event::unpack_event;

#[derive(Debug, Clone, Copy)]
enum State {
    Started,
    Running,
    Finished,
}

#[derive(Debug, Default)]
pub struct SpanMap<T> {
    map: BuckMutMap<SpanId, (State, T)>,
    running: u64,
    finished: u64,
    cancelled: u64,

    min_started: u64,
    min_finished: u64,
}

impl<T> SpanMap<T> {
    fn started(&mut self, id: SpanId, data: T) {
        self.map.insert(id, (State::Started, data));
        self.cancelled = self.cancelled.saturating_sub(1);
    }

    fn cancelled(&mut self, id: SpanId) -> Option<T> {
        self.map.remove(&id).map(|(state, v)| {
            match state {
                State::Started => {}
                State::Running => {
                    self.running -= 1;
                }
                State::Finished => {
                    self.finished -= 1;
                }
            }
            self.cancelled += 1;
            v
        })
    }

    fn running(&mut self, id: SpanId) -> Option<&mut T> {
        if let Some((state, v)) = self.map.get_mut(&id) {
            if let State::Started = state {
                *state = State::Running;
                self.running += 1;
            }
            Some(v)
        } else {
            None
        }
    }

    fn finished(&mut self, id: SpanId) -> Option<&mut T> {
        match self.map.get_mut(&id) {
            Some((state, v)) => {
                match state {
                    State::Started => {
                        self.finished += 1;
                    }
                    State::Running => {
                        self.running -= 1;
                        self.finished += 1;
                    }
                    State::Finished => {}
                }

                *state = State::Finished;
                Some(v)
            }
            None => None,
        }
    }

    fn get_stats(&self) -> BuildProgressPhaseStatsItem {
        BuildProgressPhaseStatsItem {
            started: std::cmp::max(self.min_started, self.map.len() as u64 + self.cancelled),
            finished: std::cmp::max(self.finished, self.min_finished),
            running: self.running,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildProgressPhaseStatsItem {
    pub started: u64,
    pub finished: u64,
    pub running: u64,
}

impl BuildProgressPhaseStatsItem {
    pub fn pending(&self) -> u64 {
        self.started - self.finished
    }

    pub fn mark_all_finished(&mut self) {
        self.finished = self.started;
        self.running = 0;
    }
}

/// Tracks some stats about what we've completed in this build.
#[derive(Default)]

pub struct BuildProgressStats {
    pub dirs_read: u64,
    pub targets: u64,

    pub actions_declared: u64,
    pub artifacts_declared: u64,

    pub running_local: u64,
    pub running_remote: u64,

    pub exec_time_ms: u64,
    pub cached_exec_time_ms: u64,
}

/// Tracks stats about ongoing work in the main phases of the build.
#[derive(Debug, Clone)]
pub struct BuildProgressPhaseStats {
    pub loads: BuildProgressPhaseStatsItem,
    pub analyses: BuildProgressPhaseStatsItem,
    pub actions: BuildProgressPhaseStatsItem,
    pub validations: BuildProgressPhaseStatsItem,
}

#[derive(Default, Clone, Copy)]
struct TrackedActionSpan {
    running_local: bool,
    running_remote: bool,
}

#[derive(Default)]
struct TrackedLoadSpan {}

#[derive(Default)]
struct TrackedAnalysisSpan {}

#[derive(Default)]
struct TrackedValidationSpan {}

#[derive(Default)]
pub struct BuildProgressStateTracker {
    stats: BuildProgressStats,

    loads: SpanMap<TrackedLoadSpan>,
    analyses: SpanMap<TrackedAnalysisSpan>,
    actions: SpanMap<TrackedActionSpan>,
    validations: SpanMap<TrackedValidationSpan>,
}

impl BuildProgressStateTracker {
    pub fn new() -> Self {
        Self {
            ..Default::default()
        }
    }

    pub fn handle_event(&mut self, event: &BuckEvent) -> buck2_error::Result<()> {
        let ev = unpack_event(event)?;

        self.handle_load(&ev)?;
        self.handle_analysis(&ev)?;
        self.handle_actions(&ev)?;

        match unpack_event(event)? {
            UnpackedBuckEvent::Instant(_, _, instant_event::Data::DiceStateSnapshot(snapshot)) => {
                if let Some(read_dir_states) = snapshot.key_states.get("ReadDirKey") {
                    self.stats.dirs_read = read_dir_states.finished as u64;
                }

                let mut analysis_min_started = 0;
                let mut analysis_min_finished = 0;

                if let Some(states) = snapshot.key_states.get("AnalysisKey") {
                    analysis_min_started += states.started as u64;
                    analysis_min_finished += states.finished as u64;
                }

                if let Some(states) = snapshot.key_states.get("AnonTargetKey") {
                    analysis_min_started += states.started as u64;
                    analysis_min_finished += states.finished as u64;
                }

                if let Some(states) = snapshot.key_states.get("DeferredCompute") {
                    analysis_min_started += states.started as u64;
                    analysis_min_finished += states.finished as u64;
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
            UnpackedBuckEvent::SpanEnd(
                BuckEvent {
                    span_id: Some(span_id),
                    ..
                },
                _,
                span_end_event::Data::SpanCancelled(..),
            ) => {
                self.loads.cancelled(*span_id);
                self.analyses.cancelled(*span_id);
                if let Some(v) = self.actions.cancelled(*span_id) {
                    self.action_finished(v);
                }
            }
            _ => {
                // ignored
            }
        }

        Ok(())
    }

    fn handle_load(&mut self, ev: &UnpackedBuckEvent) -> buck2_error::Result<()> {
        match ev {
            UnpackedBuckEvent::SpanStart(
                BuckEvent {
                    span_id: Some(span_id),
                    ..
                },
                _,
                span_start_event::Data::Load(..),
            ) => {
                self.loads.started(*span_id, TrackedLoadSpan {});
                self.loads.running(*span_id);
            }
            UnpackedBuckEvent::SpanEnd(
                BuckEvent {
                    span_id: Some(span_id),
                    ..
                },
                _,
                span_end_event::Data::Load(LoadBuildFileEnd { target_count, .. }),
            ) => {
                self.loads.finished(*span_id);
                if let Some(c) = target_count {
                    self.stats.targets += c;
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn handle_analysis(&mut self, ev: &UnpackedBuckEvent) -> buck2_error::Result<()> {
        match ev {
            UnpackedBuckEvent::SpanStart(
                BuckEvent {
                    span_id: Some(span_id),
                    ..
                },
                _,
                span_start_event::Data::Analysis(AnalysisStart { .. }),
            ) => {
                self.analyses.started(*span_id, TrackedAnalysisSpan {});
            }
            UnpackedBuckEvent::SpanStart(
                BuckEvent {
                    parent_id: Some(parent_id),
                    ..
                },
                _,
                span_start_event::Data::AnalysisStage(AnalysisStageStart {
                    stage: Some(analysis_stage_start::Stage::EvaluateRule(..)),
                }),
            ) => {
                self.analyses.running(*parent_id);
            }
            UnpackedBuckEvent::SpanEnd(
                BuckEvent {
                    span_id: Some(span_id),
                    ..
                },
                _,
                span_end_event::Data::Analysis(AnalysisEnd {
                    declared_actions,
                    declared_artifacts,
                    ..
                }),
            ) => {
                self.stats.actions_declared += declared_actions.unwrap_or(0);
                self.stats.artifacts_declared += declared_artifacts.unwrap_or(0);
                self.analyses.finished(*span_id);
            }
            _ => {}
        }
        Ok(())
    }

    fn action_finished(&mut self, data: TrackedActionSpan) {
        if data.running_local {
            self.stats.running_local -= 1;
        }
        if data.running_remote {
            self.stats.running_remote -= 1;
        }
    }

    fn handle_actions(&mut self, ev: &UnpackedBuckEvent) -> buck2_error::Result<()> {
        match ev {
            UnpackedBuckEvent::SpanStart(
                BuckEvent {
                    span_id: Some(span_id),
                    ..
                },
                _,
                span_start_event::Data::ActionExecution(ActionExecutionStart { .. }),
            ) => {
                self.actions.started(*span_id, TrackedActionSpan::default());
            }
            UnpackedBuckEvent::SpanStart(
                BuckEvent {
                    parent_id: Some(parent_id),
                    ..
                },
                _,
                span_start_event::Data::ExecutorStage(ExecutorStageStart { stage: Some(stage) }),
            ) => {
                match stage {
                    executor_stage_start::Stage::Re(ReStage {
                        stage: Some(re_stage::Stage::Execute(..)),
                    }) => {
                        if let Some(data) = self.actions.running(*parent_id) {
                            // An action can enter a stage more than once; it runs once.
                            if !data.running_remote {
                                data.running_remote = true;
                                self.stats.running_remote += 1;
                            }
                        }
                    }
                    executor_stage_start::Stage::Local(LocalStage {
                        stage: Some(local_stage::Stage::Execute(..)),
                    }) => {
                        if let Some(data) = self.actions.running(*parent_id) {
                            if !data.running_local {
                                data.running_local = true;
                                self.stats.running_local += 1;
                            }
                        }
                    }
                    _ => {}
                };
            }
            UnpackedBuckEvent::SpanEnd(
                BuckEvent {
                    span_id: Some(span_id),
                    ..
                },
                _,
                span_end_event::Data::ActionExecution(end),
            ) => {
                if let Some(data) = self.actions.finished(*span_id) {
                    let data = *data;
                    self.action_finished(data);
                }

                let exec_time = get_last_command_execution_time(end);
                self.stats.exec_time_ms += exec_time.exec_time_ms;
                self.stats.cached_exec_time_ms += exec_time.cached_exec_time_ms;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn phase_stats(&self) -> BuildProgressPhaseStats {
        BuildProgressPhaseStats {
            loads: self.loads.get_stats(),
            analyses: self.analyses.get_stats(),
            actions: self.actions.get_stats(),
            validations: self.validations.get_stats(),
        }
    }

    pub fn progress_stats(&self) -> &BuildProgressStats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use buck2_wrapper_common::invocation_id::TraceId;

    use super::*;

    fn event(
        span: u64,
        parent: Option<u64>,
        data: buck2_data::buck_event::Data,
    ) -> buck2_error::Result<BuckEvent> {
        Ok(BuckEvent::new(
            std::time::SystemTime::now(),
            TraceId::new(),
            Some(SpanId::from_u64(span)?),
            parent.map(SpanId::from_u64).transpose()?,
            data,
        ))
    }

    fn re_execute_start() -> buck2_data::buck_event::Data {
        buck2_data::SpanStartEvent {
            data: Some(span_start_event::Data::ExecutorStage(ExecutorStageStart {
                stage: Some(executor_stage_start::Stage::Re(ReStage {
                    stage: Some(re_stage::Stage::Execute(buck2_data::ReExecute::default())),
                })),
            })),
        }
        .into()
    }

    /// An action whose RE execution goes through the `EXECUTING` stage twice (the RE client opens
    /// a new `Execute` stage span on every transition into it) is one running remote action, and
    /// none once it has finished.
    #[test]
    fn test_running_remote_with_two_execute_stages() -> buck2_error::Result<()> {
        let mut tracker = BuildProgressStateTracker::new();
        let start: buck2_data::buck_event::Data = buck2_data::SpanStartEvent {
            data: Some(span_start_event::Data::ActionExecution(
                ActionExecutionStart::default(),
            )),
        }
        .into();
        let end: buck2_data::buck_event::Data = buck2_data::SpanEndEvent {
            data: Some(span_end_event::Data::ActionExecution(Box::default())),
            ..Default::default()
        }
        .into();

        tracker.handle_event(&event(1, None, start)?)?;
        tracker.handle_event(&event(2, Some(1), re_execute_start())?)?;
        tracker.handle_event(&event(3, Some(1), re_execute_start())?)?;
        assert_eq!(tracker.progress_stats().running_remote, 1);
        tracker.handle_event(&event(1, None, end)?)?;

        assert_eq!(tracker.progress_stats().running_remote, 0);
        assert_eq!(tracker.phase_stats().actions.finished, 1);
        Ok(())
    }

    #[test]
    fn test_span_map() -> buck2_error::Result<()> {
        let mut map: SpanMap<u64> = SpanMap::default();

        assert_eq!(
            map.get_stats(),
            BuildProgressPhaseStatsItem {
                started: 0,
                finished: 0,
                running: 0
            }
        );

        map.started(SpanId::from_u64(1).unwrap(), 1);

        assert_eq!(
            map.get_stats(),
            BuildProgressPhaseStatsItem {
                started: 1,
                finished: 0,
                running: 0
            }
        );

        assert_eq!(map.running(SpanId::from_u64(1).unwrap()).copied(), Some(1));

        assert_eq!(
            map.get_stats(),
            BuildProgressPhaseStatsItem {
                started: 1,
                finished: 0,
                running: 1
            }
        );

        assert!(map.finished(SpanId::from_u64(1).unwrap()).is_some());
        assert_eq!(
            map.get_stats(),
            BuildProgressPhaseStatsItem {
                started: 1,
                finished: 1,
                running: 0
            }
        );

        map.started(SpanId::from_u64(2).unwrap(), 2);
        assert_eq!(
            map.get_stats(),
            BuildProgressPhaseStatsItem {
                started: 2,
                finished: 1,
                running: 0
            }
        );

        map.cancelled(SpanId::from_u64(2).unwrap());
        assert_eq!(
            map.get_stats(),
            BuildProgressPhaseStatsItem {
                started: 2,
                finished: 1,
                running: 0
            }
        );

        // started shouldn't be incremented because we had a cancellation
        map.started(SpanId::from_u64(3).unwrap(), 3);
        assert_eq!(
            map.get_stats(),
            BuildProgressPhaseStatsItem {
                started: 2,
                finished: 1,
                running: 0
            }
        );

        // started should now increment
        map.started(SpanId::from_u64(4).unwrap(), 4);
        assert_eq!(
            map.get_stats(),
            BuildProgressPhaseStatsItem {
                started: 3,
                finished: 1,
                running: 0
            }
        );

        map.min_started = 8;

        assert_eq!(
            map.get_stats(),
            BuildProgressPhaseStatsItem {
                started: 8,
                finished: 1,
                running: 0
            }
        );

        map.min_finished = 4;
        assert_eq!(
            map.get_stats(),
            BuildProgressPhaseStatsItem {
                started: 8,
                finished: 4,
                running: 0
            }
        );

        Ok(())
    }
}
