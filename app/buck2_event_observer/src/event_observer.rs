/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_events::BuckEvent;
use buck2_wrapper_common::invocation_id::TraceId;

use crate::action_stats::ActionStats;
use crate::debug_events::DebugEventsState;
use crate::dice_state::DiceState;
use crate::progress::BuildProgressStateTracker;
use crate::re_state::ReState;
use crate::session_info::SessionInfo;
use crate::span_tracker::BuckEventSpanTracker;
use crate::starlark_debug::StarlarkDebuggerState;
use crate::test_state::TestState;
use crate::two_snapshots::TwoSnapshots;

pub struct EventObserver<E> {
    pub span_tracker: BuckEventSpanTracker,
    pub action_stats: ActionStats,
    re_state: ReState,
    two_snapshots: TwoSnapshots, // NOTE: We got many more copies of this than we should.
    system_info: buck2_data::SystemInfo,
    session_info: SessionInfo,
    test_state: TestState,
    starlark_debugger_state: StarlarkDebuggerState,
    dice_state: DiceState,
    /// When running without the Superconsole, we skip some state that we don't need. This might be
    /// premature optimization.
    extra: E,
}

impl<E> EventObserver<E>
where
    E: EventObserverExtra,
{
    pub fn new(trace_id: TraceId) -> Self {
        Self {
            span_tracker: BuckEventSpanTracker::new(),
            action_stats: ActionStats::default(),
            re_state: ReState::new(),
            two_snapshots: TwoSnapshots::default(),
            system_info: buck2_data::SystemInfo::default(),
            session_info: SessionInfo {
                trace_id: trace_id.clone(),
                test_session: None,
                legacy_dice: false,
            },
            test_state: TestState::default(),
            starlark_debugger_state: StarlarkDebuggerState::new(),
            dice_state: DiceState::new(),
            extra: E::new(),
        }
    }

    pub async fn observe(&mut self, event: &Arc<BuckEvent>) -> buck2_error::Result<()> {
        self.span_tracker.handle_event(event)?;

        {
            use buck2_data::buck_event::Data::*;

            match event.data() {
                SpanEnd(end) => {
                    use buck2_data::span_end_event::Data::*;

                    if let ActionExecution(action_execution_end) = end
                        .data
                        .as_ref()
                        .internal_error("Missing `data` in SpanEnd")?
                    {
                        self.action_stats.update(action_execution_end);
                    }
                }
                Instant(instant) => {
                    use buck2_data::instant_event::Data::*;

                    match instant
                        .data
                        .as_ref()
                        .internal_error("Missing `data` in `Instant`")?
                    {
                        ReSession(re_session) => {
                            self.re_state.add_re_session(re_session);
                        }
                        Snapshot(snapshot) => {
                            self.re_state.update(snapshot);
                            self.two_snapshots.update(event.timestamp(), snapshot);
                        }
                        TestDiscovery(discovery) => {
                            use buck2_data::test_discovery::Data::*;

                            match discovery.data.as_ref().ok_or_else(|| {
                                internal_error!("Missing `data` in `TestDiscovery`")
                            })? {
                                Session(session) => {
                                    self.session_info.test_session = Some(session.clone());
                                }
                                Tests(tests) => {
                                    self.test_state.discovered += tests.test_names.len() as u64
                                }
                            }
                        }
                        TestResult(result) => {
                            self.test_state.update(result)?;
                        }
                        DebugAdapterSnapshot(snapshot) => {
                            self.starlark_debugger_state
                                .update(event.timestamp(), snapshot)?;
                        }
                        TagEvent(tags) => {
                            if tags.tags.contains(&"which-dice:Legacy".to_owned()) {
                                self.session_info.legacy_dice = true;
                            }
                        }
                        DiceStateSnapshot(dice) => {
                            self.dice_state.update(dice);
                        }
                        SystemInfo(system_info) => {
                            self.system_info = system_info.clone();
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        self.extra.observe(event)?;

        Ok(())
    }

    pub fn spans(&self) -> &BuckEventSpanTracker {
        &self.span_tracker
    }

    pub fn action_stats(&self) -> &ActionStats {
        &self.action_stats
    }

    pub fn re_state(&self) -> &ReState {
        &self.re_state
    }

    pub fn two_snapshots(&self) -> &TwoSnapshots {
        &self.two_snapshots
    }

    pub fn system_info(&self) -> &buck2_data::SystemInfo {
        &self.system_info
    }

    pub fn session_info(&self) -> &SessionInfo {
        &self.session_info
    }

    pub fn starlark_debugger_state(&self) -> &StarlarkDebuggerState {
        &self.starlark_debugger_state
    }

    pub fn test_state(&self) -> &TestState {
        &self.test_state
    }

    pub fn extra(&self) -> &E {
        &self.extra
    }

    pub fn dice_state(&self) -> &DiceState {
        &self.dice_state
    }
}

pub trait EventObserverExtra: Send {
    fn new() -> Self;

    fn observe(&mut self, event: &Arc<BuckEvent>) -> buck2_error::Result<()>;
}

/// This has more fields for debug info. We don't always capture those.
pub struct DebugEventObserverExtra {
    debug_events: DebugEventsState,
    progress_state: BuildProgressStateTracker,
}

impl EventObserverExtra for DebugEventObserverExtra {
    fn new() -> Self {
        Self {
            debug_events: DebugEventsState::new(),
            progress_state: BuildProgressStateTracker::new(),
        }
    }

    fn observe(&mut self, event: &Arc<BuckEvent>) -> buck2_error::Result<()> {
        self.debug_events.handle_event(event)?;
        self.progress_state.handle_event(event)?;

        Ok(())
    }
}

impl DebugEventObserverExtra {
    pub fn debug_events(&self) -> &DebugEventsState {
        &self.debug_events
    }

    pub fn progress_state(&self) -> &BuildProgressStateTracker {
        &self.progress_state
    }
}

pub struct NoopEventObserverExtra;

impl EventObserverExtra for NoopEventObserverExtra {
    fn new() -> Self {
        Self
    }

    fn observe(&mut self, _event: &Arc<BuckEvent>) -> buck2_error::Result<()> {
        // Noop
        Ok(())
    }
}
