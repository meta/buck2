/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Write;
use std::time::Duration;

use buck2_event_observer::display;
use buck2_event_observer::display::TargetDisplayOptions;
use buck2_event_observer::fmt_duration;
use buck2_event_observer::span_tracker::BuckEventSpanHandle;
use buck2_event_observer::span_tracker::BuckEventSpanTracker;
use superconsole::Component;
use superconsole::Dimensions;
use superconsole::DrawMode;
use superconsole::Line;
use superconsole::Lines;
use superconsole::components::DrawVertical;

use self::table_builder::Table;
use crate::subscribers::superconsole::SuperConsoleState;
use crate::subscribers::superconsole::timed_list::table_builder::Row;
use crate::subscribers::superconsole::timed_list::table_builder::SummaryRow;
use crate::subscribers::superconsole::timed_list::table_builder::TimedRow;

mod table_builder;

/// The minimum time disparity between a single subaction and a target elapsed time
/// before the former will be displayed separately from the latter's time.
/// Heuristic uses a percent difference to normalize for long running actions.
const DISPLAY_SUBACTION_CUTOFF: f64 = 0.9;

/// Information about notable event durations.
#[derive(Debug)]
pub struct Cutoffs {
    /// Cutoff for normal execution time.
    pub inform: Duration,
    /// Cutoff for abnormal but still OK execution time.
    pub warn: Duration,
    /// Minimum time an event must be alive before it is worth displaying.
    pub _notable: Duration,
}

/// This component renders each event and a timer indicating for how long the event has been ongoing.
struct TimedListBody<'c> {
    cutoffs: &'c Cutoffs,
    state: &'c SuperConsoleState,
}

impl TimedListBody<'_> {
    /// Render a root  as `root [first child + remaining children]`
    fn draw_root_first_child(
        &self,
        root: &BuckEventSpanHandle,
        single_child: BuckEventSpanHandle,
        remaining_children: usize,
        display_platform: bool,
    ) -> buck2_error::Result<TimedRow> {
        let info = root.info();
        let child_info = single_child.info();

        let root_event = display::display_event(
            &info.event,
            TargetDisplayOptions::for_console(display_platform),
        )?;
        let child_event = display::display_event(
            &child_info.event,
            TargetDisplayOptions::for_console(display_platform),
        )?;

        let child_info_elapsed = self.state.timekeeper.duration_since(child_info.start);
        let info_elapsed = self.state.timekeeper.duration_since(info.start);
        let subaction_ratio = child_info_elapsed.as_secs_f64() / info_elapsed.as_secs_f64();

        let mut aux = child_event.to_string();
        if subaction_ratio < DISPLAY_SUBACTION_CUTOFF {
            aux.push(' ');
            aux.push_str(&fmt_duration::fmt_duration(child_info_elapsed));
        }
        if remaining_children > 0 {
            write!(aux, " + {remaining_children}").expect("Write to String is not fallible");
        }

        // Escalate the row color on time in the current stage, and only while
        // actively executing -- total elapsed is mostly queueing under contention.
        let style_age = if display::is_active_execution_stage(&child_info.event) {
            child_info_elapsed
        } else {
            Duration::ZERO
        };

        TimedRow::new(
            0,
            root_event.label,
            root_event.detail,
            root_event.category,
            Some(aux),
            fmt_duration::fmt_duration(info_elapsed),
            style_age,
            self.cutoffs,
        )
    }

    fn draw_root(&self, root: &BuckEventSpanHandle) -> buck2_error::Result<Vec<TimedRow>> {
        let timekeeper = &self.state.timekeeper;
        let config = &self.state.config;
        let two_lines = config.two_lines;
        let display_platform = config.display_platform;
        let info = root.info();

        let mut it = root.children();

        match it.next() {
            Some(first) if !two_lines => Ok(vec![self.draw_root_first_child(
                root,
                first,
                it.len(),
                display_platform,
            )?]),
            first => {
                let mut rows = Vec::new();
                rows.push(TimedRow::span(
                    0,
                    info,
                    timekeeper,
                    self.cutoffs,
                    display_platform,
                )?);

                for child in first.into_iter().chain(it) {
                    rows.push(TimedRow::span(
                        2,
                        child.info(),
                        timekeeper,
                        self.cutoffs,
                        display_platform,
                    )?);
                }
                Ok(rows)
            }
        }
    }
}

impl Component for TimedListBody<'_> {
    type Error = buck2_error::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> buck2_error::Result<Lines> {
        let config = &self.state.config;
        // `Component::draw` clips the rows to `dimensions.height` from the bottom, which would
        // remove the summary row first and then roots without a trace.
        let max_lines = config.max_lines.min(dimensions.height);

        let observer = self.state.simple_console.observer();

        let spans = observer.spans();

        let mut roots = spans.iter_roots();

        let mut builder = Table::new();

        let mut first_not_rendered = None;
        let mut visible_roots = 0;

        while let Some(root) = roots.next() {
            let rows = self.draw_root(&root)?;
            let hidden_roots_after_this = roots.len();
            let reserved_summary_rows = if hidden_roots_after_this > 0 { 1 } else { 0 };

            if builder.len() + rows.len() + reserved_summary_rows > max_lines {
                first_not_rendered = Some(root);
                break;
            }

            builder.rows.extend(rows.into_iter().map(Row::from));
            visible_roots += 1;
        }

        let more = roots.len() + first_not_rendered.map_or(0, |_| 1);

        if more > 0 {
            let total = visible_roots + more;
            builder.rows.push(Row::Summary(SummaryRow {
                visible_roots,
                total_roots: total,
            }));
        }

        builder.draw(dimensions, mode)
    }
}

/// Component for timed list header
struct TimedListHeader;

impl Component for TimedListHeader {
    type Error = buck2_error::Error;

    fn draw_unchecked(
        &self,
        dimensions: Dimensions,
        _mode: DrawMode,
    ) -> buck2_error::Result<Lines> {
        Ok(Lines(vec![Line::unstyled(&"─".repeat(dimensions.width))?]))
    }
}

/// Component that displays ongoing events and their durations + summary stats.
pub struct TimedList<'a> {
    cutoffs: &'a Cutoffs,
    state: &'a SuperConsoleState,
}

impl<'a> TimedList<'a> {
    /// * `cutoffs` determines durations for warnings, time-outs, and baseline notability.
    pub fn new(cutoffs: &'a Cutoffs, state: &'a SuperConsoleState) -> Self {
        Self { cutoffs, state }
    }
}

impl Component for TimedList<'_> {
    type Error = buck2_error::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> buck2_error::Result<Lines> {
        let span_tracker: &BuckEventSpanTracker = self.state.simple_console.observer().spans();

        match mode {
            DrawMode::Normal if !span_tracker.is_unused() => {
                let header = TimedListHeader;
                let body = TimedListBody {
                    cutoffs: self.cutoffs,
                    state: self.state,
                };

                let mut draw = DrawVertical::new(dimensions);
                draw.draw(&header, mode)?;
                draw.draw(&body, mode)?;
                Ok(draw.finish())
            }
            _ => Ok(Lines::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::SystemTime;

    use buck2_data::FakeStart;
    use buck2_data::SpanStartEvent;
    use buck2_event_observer::action_stats::ActionStats;
    use buck2_event_observer::span_tracker::EventTimestamp;
    use buck2_event_observer::verbosity::Verbosity;
    use buck2_events::BuckEvent;
    use buck2_events::span::SpanId;
    use buck2_hash::IntentionallyStdHashMap;
    use buck2_wrapper_common::invocation_id::TraceId;
    use dupe::Dupe;
    use itertools::Itertools;

    use super::*;
    use crate::subscribers::superconsole::SuperConsoleConfig;
    use crate::subscribers::superconsole::timekeeper::RealtimeClock;
    use crate::subscribers::superconsole::timekeeper::Timekeeper;
    use crate::ticker::Tick;

    const CUTOFFS: Cutoffs = Cutoffs {
        inform: Duration::from_secs(2),
        warn: Duration::from_secs(4),
        _notable: Duration::from_millis(200),
    };

    fn fake_timekeeper(tick: Tick) -> Timekeeper {
        // We run time 10x slower so that any time occurring due to the
        // test running on an overloaded server is ignored.
        //
        // Note that going to 100x slower causes Windows CI to fail, because
        // the `Instant` can't go below the time when the VM was booted, or you get an
        // underflow of `Instant`.
        Timekeeper::new(
            Box::new(RealtimeClock),
            EventTimestamp(tick.current_realtime.into()),
        )
    }

    fn fake_time(tick: &Tick, secs: u64) -> SystemTime {
        tick.current_realtime
            .checked_sub(Duration::from_secs(secs))
            .expect("System time went too low")
    }

    fn super_console_state_for_test(
        span_tracker: BuckEventSpanTracker,
        action_stats: ActionStats,
        timekeeper: Timekeeper,
        timed_list_state: SuperConsoleConfig,
    ) -> SuperConsoleState {
        let mut state = SuperConsoleState::new(
            timekeeper,
            TraceId::null(),
            Verbosity::default(),
            false,
            timed_list_state,
            None,
            None,
        )
        .unwrap();
        state.simple_console.observer.span_tracker = span_tracker;
        state.simple_console.observer.action_stats = action_stats;
        state
    }

    #[test]
    fn test_normal() -> buck2_error::Result<()> {
        let tick = Tick::now();

        let label = Arc::new(BuckEvent::new(
            fake_time(&tick, 3),
            TraceId::new(),
            Some(SpanId::next()),
            None,
            buck2_data::buck_event::Data::SpanStart(SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::Fake(FakeStart {
                    caramba: "test".to_owned(),
                })),
            }),
        ));

        let module = Arc::new(BuckEvent::new(
            fake_time(&tick, 1),
            TraceId::new(),
            Some(SpanId::next()),
            None,
            buck2_data::buck_event::Data::SpanStart(SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::Fake(FakeStart {
                    caramba: "foo".to_owned(),
                })),
            }),
        ));

        let mut state = BuckEventSpanTracker::new();
        state.start_at(&label).unwrap();
        state.start_at(&module).unwrap();

        let timekeeper = fake_timekeeper(tick);
        let action_stats = ActionStats {
            local_actions: 0,
            remote_actions: 0,
            cached_actions: 1,
            fallback_actions: 0,
            remote_dep_file_cached_actions: 0,
            remote_served_from_cache_actions: 0,
            remote_deduplicated_actions: 0,
            excess_cache_misses: 0,
        };

        let timed_list_state = SuperConsoleConfig {
            max_lines: 5,
            ..Default::default()
        };

        let output = TimedList::new(
            &CUTOFFS,
            &super_console_state_for_test(state, action_stats, timekeeper, timed_list_state),
        )
        .draw(
            Dimensions {
                width: 40,
                height: 10,
            },
            DrawMode::Normal,
        )?;
        let expected = [
            "────────────────────────────────────────",
            "test · speak of the devil           3.0s",
            "foo · speak of the devil            1.0s",
        ]
        .iter()
        .map(|l| format!("{l}\n"))
        .join("");

        pretty_assertions::assert_eq!(output.fmt_for_test().to_string(), expected);

        Ok(())
    }

    #[test]
    fn test_remaining() -> buck2_error::Result<()> {
        let tick = Tick::now();

        let e1 = BuckEvent::new(
            fake_time(&tick, 1),
            TraceId::new(),
            Some(SpanId::next()),
            None,
            buck2_data::buck_event::Data::SpanStart(SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::Fake(FakeStart {
                    caramba: "e1".to_owned(),
                })),
            }),
        );

        let e2 = BuckEvent::new(
            fake_time(&tick, 1),
            TraceId::new(),
            Some(SpanId::next()),
            None,
            buck2_data::buck_event::Data::SpanStart(SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::Fake(FakeStart {
                    caramba: "e2".to_owned(),
                })),
            }),
        );

        let e3 = BuckEvent::new(
            fake_time(&tick, 1),
            TraceId::new(),
            Some(SpanId::next()),
            None,
            buck2_data::buck_event::Data::SpanStart(SpanStartEvent {
                data: Some(buck2_data::span_start_event::Data::Fake(FakeStart {
                    caramba: "e3".to_owned(),
                })),
            }),
        );

        let mut state = BuckEventSpanTracker::new();

        for e in [e1, e2, e3] {
            state.start_at(&Arc::new(e.clone())).unwrap();
        }

        let time_speed = fake_timekeeper(tick);
        let action_stats = ActionStats {
            local_actions: 0,
            remote_actions: 0,
            cached_actions: 1,
            fallback_actions: 0,
            remote_dep_file_cached_actions: 0,
            remote_served_from_cache_actions: 0,
            remote_deduplicated_actions: 0,
            excess_cache_misses: 0,
        };

        let timed_list_state = SuperConsoleConfig {
            max_lines: 2,
            ..Default::default()
        };

        let output = TimedList::new(
            &CUTOFFS,
            &super_console_state_for_test(state, action_stats, time_speed, timed_list_state),
        )
        .draw(
            Dimensions {
                width: 40,
                height: 10,
            },
            DrawMode::Normal,
        )?;
        let expected = [
            "────────────────────────────────────────",
            "e1 · speak of the devil             1.0s",
            "<span italic>1/3 running tasks shown</span>",
        ]
        .iter()
        .map(|l| format!("{l}\n"))
        .join("");

        pretty_assertions::assert_eq!(output.fmt_for_test().to_string(), expected);

        Ok(())
    }

    #[test]
    fn test_remaining_clipped_by_height() -> buck2_error::Result<()> {
        let tick = Tick::now();

        let mut state = BuckEventSpanTracker::new();
        for name in ["e1", "e2", "e3"] {
            let e = BuckEvent::new(
                fake_time(&tick, 1),
                TraceId::new(),
                Some(SpanId::next()),
                None,
                buck2_data::buck_event::Data::SpanStart(SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Fake(FakeStart {
                        caramba: name.to_owned(),
                    })),
                }),
            );
            state.start_at(&Arc::new(e)).unwrap();
        }

        let time_speed = fake_timekeeper(tick);
        let action_stats = ActionStats {
            local_actions: 0,
            remote_actions: 0,
            cached_actions: 1,
            fallback_actions: 0,
            remote_dep_file_cached_actions: 0,
            remote_served_from_cache_actions: 0,
            remote_deduplicated_actions: 0,
            excess_cache_misses: 0,
        };

        // `max_lines` allows every root, but the terminal only has room for the header and two
        // more lines: one root and the summary row.
        let timed_list_state = SuperConsoleConfig {
            max_lines: 5,
            ..Default::default()
        };

        let output = TimedList::new(
            &CUTOFFS,
            &super_console_state_for_test(state, action_stats, time_speed, timed_list_state),
        )
        .draw(
            Dimensions {
                width: 40,
                height: 3,
            },
            DrawMode::Normal,
        )?;
        let expected = [
            "────────────────────────────────────────",
            "e1 · speak of the devil             1.0s",
            "<span italic>1/3 running tasks shown</span>",
        ]
        .iter()
        .map(|l| format!("{l}\n"))
        .join("");

        pretty_assertions::assert_eq!(output.fmt_for_test().to_string(), expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_remaining_with_pending() -> buck2_error::Result<()> {
        let tick = Tick::now();

        let mut state = SuperConsoleState::new(
            fake_timekeeper(tick),
            TraceId::null(),
            Verbosity::default(),
            false,
            SuperConsoleConfig {
                max_lines: 2,
                ..Default::default()
            },
            None,
            None,
        )?;

        state
            .simple_console
            .observer
            .observe(&span_start_event(None, fake_time(&tick, 10)))
            .await?;

        state
            .simple_console
            .observer
            .observe(&dice_snapshot(fake_time(&tick, 1)))
            .await?;

        {
            let output = TimedList::new(&CUTOFFS, &state).draw(
                Dimensions {
                    width: 60,
                    height: 10,
                },
                DrawMode::Normal,
            )?;

            let expected = [
                "────────────────────────────────────────────────────────────",
                "pkg:target [category identifier]                       10.0s",
            ]
            .iter()
            .map(|l| format!("{l}\n"))
            .join("");

            pretty_assertions::assert_eq!(output.fmt_for_test().to_string(), expected);
        }

        {
            state.config.max_lines = 1; // With fewer lines now

            let output = TimedList::new(&CUTOFFS, &state).draw(
                Dimensions {
                    width: 60,
                    height: 10,
                },
                DrawMode::Normal,
            )?;

            let expected = [
                "────────────────────────────────────────────────────────────",
                "pkg:target [category identifier]                       10.0s",
            ]
            .iter()
            .map(|l| format!("{l}\n"))
            .join("");

            pretty_assertions::assert_eq!(output.fmt_for_test().to_string(), expected);
        }

        Ok(())
    }

    #[test]
    fn test_children() -> buck2_error::Result<()> {
        let tick = Tick::now();

        let parent = SpanId::next();

        let prepare = Arc::new(BuckEvent::new(
            fake_time(&tick, 5),
            TraceId::new(),
            Some(SpanId::next()),
            Some(parent),
            SpanStartEvent {
                data: Some(
                    buck2_data::ExecutorStageStart {
                        stage: Some(buck2_data::PrepareAction {}.into()),
                    }
                    .into(),
                ),
            }
            .into(),
        ));

        let mut state = BuckEventSpanTracker::new();
        state
            .start_at(&span_start_event(Some(parent), fake_time(&tick, 10)))
            .unwrap();
        state.start_at(&prepare).unwrap();

        let action_stats = ActionStats {
            local_actions: 0,
            remote_actions: 0,
            cached_actions: 1,
            fallback_actions: 0,
            remote_dep_file_cached_actions: 0,
            remote_served_from_cache_actions: 0,
            remote_deduplicated_actions: 0,
            excess_cache_misses: 0,
        };

        let timed_list_state = SuperConsoleConfig {
            max_lines: 5,
            ..Default::default()
        };

        let output = TimedList::new(
            &CUTOFFS,
            &super_console_state_for_test(
                state.clone(),
                action_stats.dupe(),
                fake_timekeeper(tick),
                timed_list_state.clone(),
            ),
        )
        .draw(
            Dimensions {
                width: 80,
                height: 10,
            },
            DrawMode::Normal,
        )?;
        let expected = [
            "────────────────────────────────────────────────────────────────────────────────",
            "pkg:target [category identifier] prepare         5.0s                      10.0s",
        ]
        .iter()
        .map(|l| format!("{l}\n"))
        .join("");

        pretty_assertions::assert_eq!(output.fmt_for_test().to_string(), expected);

        // Now, add another action. Normally we don't have multiple stages actually running
        // concurrently but this is a test!

        let re_download = Arc::new(BuckEvent::new(
            fake_time(&tick, 2),
            TraceId::new(),
            Some(SpanId::next()),
            Some(parent),
            SpanStartEvent {
                data: Some(
                    buck2_data::ExecutorStageStart {
                        stage: Some(
                            buck2_data::ReStage {
                                stage: Some(buck2_data::ReDownload {}.into()),
                            }
                            .into(),
                        ),
                    }
                    .into(),
                ),
            }
            .into(),
        ));

        state.start_at(&re_download).unwrap();

        let output = TimedList::new(
            &CUTOFFS,
            &super_console_state_for_test(
                state,
                action_stats,
                fake_timekeeper(tick),
                timed_list_state,
            ),
        )
        .draw(
            Dimensions {
                width: 80,
                height: 10,
            },
            DrawMode::Normal,
        )?;
        let expected = [
            "────────────────────────────────────────────────────────────────────────────────",
            "pkg:target [category identifier] prepare         5.0s + 1                  10.0s",
        ]
        .iter()
        .map(|l| format!("{l}\n"))
        .join("");

        pretty_assertions::assert_eq!(output.fmt_for_test().to_string(), expected);

        Ok(())
    }

    fn dice_snapshot(time: SystemTime) -> Arc<BuckEvent> {
        Arc::new(BuckEvent::new(
            time,
            TraceId::new(),
            None,
            None,
            buck2_data::InstantEvent {
                data: Some(
                    buck2_data::DiceStateSnapshot {
                        key_states: {
                            let mut map = IntentionallyStdHashMap::new();
                            map.insert(
                                "BuildKey".to_owned(),
                                buck2_data::DiceKeyState {
                                    started: 5,
                                    finished: 2,
                                    check_deps_started: 2,
                                    check_deps_finished: 1,
                                    compute_started: 4,
                                    compute_finished: 2,
                                },
                            );
                            map
                        },
                        core_state_queue_depth: 0,
                        core_state_processed_requests: 0,
                    }
                    .into(),
                ),
            }
            .into(),
        ))
    }

    fn span_start_event(parent_span: Option<SpanId>, time: SystemTime) -> Arc<BuckEvent> {
        let span_id = Some(parent_span.unwrap_or(SpanId::next()));
        Arc::new(BuckEvent::new(
            time,
            TraceId::new(),
            span_id,
            None,
            SpanStartEvent {
                data: Some(
                    buck2_data::ActionExecutionStart {
                        key: Some(buck2_data::ActionKey {
                            id: Default::default(),
                            owner: Some(buck2_data::action_key::Owner::TargetLabel(
                                buck2_data::ConfiguredTargetLabel {
                                    label: Some(buck2_data::TargetLabel {
                                        package: "pkg".into(),
                                        name: "target".into(),
                                    }),
                                    configuration: Some(buck2_data::Configuration {
                                        full_name: "conf".into(),
                                    }),
                                    execution_configuration: None,
                                },
                            )),
                            key: "".to_owned(),
                        }),
                        name: Some(buck2_data::ActionName {
                            category: "category".into(),
                            identifier: "identifier".into(),
                        }),
                        kind: buck2_data::ActionKind::NotSet as i32,
                    }
                    .into(),
                ),
            }
            .into(),
        ))
    }
}
