/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::dice_state::DiceState;
use crate::span_tracker::Roots;
use crate::span_tracker::SpanTrackable;

/// Estimate how many things are still left to do in a build. This is an approximation since our
/// roots and DICE state are not necessarily entirely in sync.
pub fn pending_estimate<T: SpanTrackable>(roots: &Roots<T>, dice: &DiceState) -> u64 {
    let mut total = 0;
    for k in &["BuildKey", "AnalysisKey"] {
        let from_dice = dice
            .key_states()
            .get(*k)
            .map_or(0, |v| v.started.saturating_sub(v.finished));

        let from_roots = roots.dice_counts().get(k).copied().unwrap_or(0);

        total += u64::from(from_dice).saturating_sub(from_roots);
    }

    total
}

pub fn estimate_completion_percentage<T: SpanTrackable>(roots: &Roots<T>, dice: &DiceState) -> u8 {
    let from_dice = dice
        .key_states()
        .get("BuildKey")
        .map_or((0, 0), |v| (v.started, v.finished));

    let from_roots = roots.dice_counts().get("BuildKey").copied().unwrap_or(0);

    let started = u64::from(from_dice.0).saturating_sub(from_roots);
    let finished = u64::from(from_dice.1).saturating_sub(from_roots);
    if started == 0 {
        // Avoid divide by zero.
        return 0;
    }
    ((finished as f64 / started as f64) * 100f64) as u8
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::UNIX_EPOCH;

    use buck2_data::SpanStartEvent;
    use buck2_events::BuckEvent;
    use buck2_events::span::SpanId;
    use buck2_hash::IntentionallyStdHashMap;
    use buck2_wrapper_common::invocation_id::TraceId;

    use crate::dice_state::DiceState;
    use crate::pending_estimate::estimate_completion_percentage;
    use crate::pending_estimate::pending_estimate;
    use crate::span_tracker::BuckEventSpanTracker;

    fn setup_roots(tracker: &mut BuckEventSpanTracker) {
        let span = Arc::new(BuckEvent::new(
            UNIX_EPOCH,
            TraceId::new(),
            Some(SpanId::next()),
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
        ));
        tracker.start_at(&span).unwrap();
    }

    fn setup_dice_state(dice_state: &mut DiceState, finished: u32, total: u32) {
        dice_state.update(&buck2_data::DiceStateSnapshot {
            key_states: {
                let mut map = IntentionallyStdHashMap::new();
                map.insert(
                    "BuildKey".to_owned(),
                    buck2_data::DiceKeyState {
                        started: total,
                        finished,
                        check_deps_started: 0,
                        check_deps_finished: 0,
                        compute_started: 0,
                        compute_finished: 0,
                    },
                );
                map
            },
            core_state_queue_depth: 0,
            core_state_processed_requests: 0,
        });
    }

    #[test]
    fn test_completion_no_progress() -> buck2_error::Result<()> {
        let mut dice = DiceState::new();
        let mut tracker = BuckEventSpanTracker::new();

        setup_roots(&mut tracker);
        setup_dice_state(&mut dice, 0, 100);
        assert_eq!(estimate_completion_percentage(tracker.roots(), &dice), 0);
        Ok(())
    }

    #[test]
    fn test_completion_percentage_build_complete() -> buck2_error::Result<()> {
        let mut dice = DiceState::new();
        let mut tracker = BuckEventSpanTracker::new();

        setup_roots(&mut tracker);
        setup_dice_state(&mut dice, 100, 100);
        assert_eq!(estimate_completion_percentage(tracker.roots(), &dice), 100);
        Ok(())
    }

    #[test]
    fn test_completion_percentage_intermediate_state() -> buck2_error::Result<()> {
        let mut dice = DiceState::new();
        let mut tracker = BuckEventSpanTracker::new();

        setup_roots(&mut tracker);
        // 26/101 -> 25/100 since we have 1 subtracted for the ActionExecutionStart
        setup_dice_state(&mut dice, 26, 101);
        assert_eq!(estimate_completion_percentage(tracker.roots(), &dice), 25);
        Ok(())
    }

    #[test]
    fn test_completion_percentage_invalid_dice_state() -> buck2_error::Result<()> {
        let mut dice = DiceState::new();
        let mut tracker = BuckEventSpanTracker::new();

        setup_roots(&mut tracker);
        setup_dice_state(&mut dice, 10, 0);
        assert_eq!(estimate_completion_percentage(tracker.roots(), &dice), 0);
        Ok(())
    }

    /// A snapshot with more finished than started keys counts as nothing pending, as the
    /// percentage estimate next to it already assumes.
    #[test]
    fn test_pending_estimate_invalid_dice_state() {
        let mut dice = DiceState::new();
        let tracker = BuckEventSpanTracker::new();

        setup_dice_state(&mut dice, 10, 0);
        assert_eq!(0, pending_estimate(tracker.roots(), &dice));
    }

    #[test]
    fn test_completion_percentage_empty_span() -> buck2_error::Result<()> {
        let mut dice = DiceState::new();
        let tracker = BuckEventSpanTracker::new();

        setup_dice_state(&mut dice, 26, 101);
        assert_eq!(estimate_completion_percentage(tracker.roots(), &dice), 25);
        Ok(())
    }
}
