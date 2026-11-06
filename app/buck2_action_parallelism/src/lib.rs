/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Shared computation for buck2 action-parallelism (concurrency distribution).
//!
//! Given the action-execution events of a single build invocation, this computes
//! the distribution of how many actions were *executing* concurrently over the
//! course of the build, summarised as percentiles plus a time-weighted average.
//!
//! "Execution-only" semantics: an action is considered in flight only during its
//! actual execution window `[metadata.start_time, start_time + execution_time]`,
//! excluding queue and cache-check time. Actions served from cache (which do not
//! execute) are excluded entirely.
//!
//! This module is consumed by both the client-side `InvocationRecorder` (which
//! computes the distribution live and emits it on the `InvocationRecord`) and the
//! offline backfill binary (which recomputes it from a decoded `.pb.zst` event
//! log), so the two paths produce identical numbers. It operates on
//! `buck2_data::ActionExecutionEnd`.

use std::collections::BTreeMap;

use buck2_event_observer::last_command_execution_kind::LastCommandExecutionKind;
use buck2_event_observer::last_command_execution_kind::get_last_command_execution_kind;

/// Percentiles reported into the `buck2_action_parallelism` Scuba table.
/// `100` resolves to the maximum observed concurrency level.
///
/// Only percentile values that have a matching `concurrency_pN` Scuba column
/// are logged; any other value is dropped by the logger rather than misplaced
/// (see `action_parallelism_scuba::set_percentiles`). Because the reported
/// distribution is self-describing — each concurrency carries its own
/// percentile label and is matched to its column by value — this list can change
/// over time without corrupting historical rows: old samples keep their own labels,
/// and any percentile whose column was since added or removed is simply ignored.
pub const PERCENTILES: [u32; 9] = [5, 10, 25, 50, 75, 90, 95, 99, 100];

/// The wall-clock execution window of a single action, in microseconds (in the
/// proto timestamp epoch; only relative positions matter for the computation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionInterval {
    pub start_us: i64,
    pub end_us: i64,
}

/// Result of a parallelism computation over one invocation's actions.
#[derive(Clone, Debug, PartialEq)]
pub struct ParallelismResult {
    /// The concurrency distribution as `(percentile, concurrency_level)` pairs,
    /// one per percentile requested from [`compute`]. Self-describing (each
    /// value carries its percentile) so the reported set of percentiles can
    /// change over time without misinterpreting historical data.
    pub percentiles: Vec<(u32, u64)>,
    /// Time-weighted mean concurrency over the time at least one action executed.
    pub avg_concurrency: f64,
    /// Number of actions that actually executed (and were counted).
    pub executed_action_count: u64,
    /// Total wall-clock time (us) during which at least one action was executing.
    pub total_active_duration_us: i64,
}

impl ParallelismResult {
    /// Result for a build with no executed actions (or only zero-length ones).
    fn empty(executed_action_count: u64, percentiles: &[u32]) -> Self {
        Self {
            percentiles: percentiles.iter().map(|&p| (p, 0)).collect(),
            avg_concurrency: 0.0,
            executed_action_count,
            total_active_duration_us: 0,
        }
    }
}

/// Extract the execution-only interval for an action from its terminal
/// `ActionExecutionEnd`, or `None` if the action did not actually execute (cache
/// hit / no command) or is missing timing metadata.
pub fn extract_interval(end: &buck2_data::ActionExecutionEnd) -> Option<ActionInterval> {
    match get_last_command_execution_kind(end) {
        LastCommandExecutionKind::Local
        | LastCommandExecutionKind::LocalWorker
        | LastCommandExecutionKind::Remote
        | LastCommandExecutionKind::RemoteDeduplicated => {}
        // Cache hits, including the service's own, results shared in flight, and no-command
        // actions do not occupy execution capacity.
        LastCommandExecutionKind::Cached
        | LastCommandExecutionKind::RemoteDepFileCached
        | LastCommandExecutionKind::RemoteServedFromCache
        | LastCommandExecutionKind::InFlight
        | LastCommandExecutionKind::NoCommand => return None,
    }

    let metadata = end.commands.last()?.details.as_ref()?.metadata.as_ref()?;
    let start = metadata.start_time.as_ref()?;
    let execution = metadata.execution_time.as_ref()?;

    let start_us = to_us(start.seconds, start.nanos)?;
    let exec_us = to_us(execution.seconds, execution.nanos)?.max(0);
    Some(ActionInterval {
        start_us,
        end_us: start_us.checked_add(exec_us)?,
    })
}

/// Convert a proto `seconds` + `nanos` pair to microseconds. Integer division by
/// 1_000 does not lose precision: all buck2 timestamps have microsecond precision.
fn to_us(seconds: i64, nanos: i32) -> Option<i64> {
    seconds
        .checked_mul(1_000_000)?
        .checked_add((nanos / 1_000) as i64)
}

/// Compute the concurrency distribution over `intervals` using a sweep line.
///
/// Mirrors `fbobjc/buck2/speed_analysis/parallel_actions/parallel_actions.py`:
/// each interval contributes a `+1` event at its start and a `-1` at its end;
/// at equal timestamps ends are processed before starts so an action ending
/// exactly as another begins does not spuriously bump the count.
pub fn compute(intervals: &[ActionInterval], percentiles: &[u32]) -> ParallelismResult {
    if intervals.is_empty() {
        return ParallelismResult::empty(0, percentiles);
    }

    // Two endpoint events per interval: +1 at start, -1 at end.
    let mut events: Vec<(i64, i8)> = intervals
        .iter()
        .flat_map(|iv| [(iv.start_us, 1i8), (iv.end_us, -1)])
        .collect();
    // Sort by time, then by delta so that -1 (end) precedes +1 (start).
    events.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    // concurrency level -> total microseconds spent at exactly that level (>0 only).
    let mut durations: BTreeMap<u64, i64> = BTreeMap::new();
    let mut current: i64 = 0;
    let mut last_time: Option<i64> = None;

    for (time, delta) in events {
        if current > 0 {
            if let Some(last) = last_time {
                let dt = time - last;
                if dt > 0 {
                    *durations.entry(current as u64).or_insert(0) += dt;
                }
            }
        }
        current += delta as i64;
        last_time = Some(time);
    }

    let total_active_duration_us: i64 = durations.values().copied().sum();
    if total_active_duration_us <= 0 {
        return ParallelismResult::empty(intervals.len() as u64, percentiles);
    }

    // Time-weighted average concurrency.
    let weighted: i128 = durations
        .iter()
        .map(|(level, dur)| (*level as i128) * (*dur as i128))
        .sum();
    let avg_concurrency = weighted as f64 / total_active_duration_us as f64;

    let percentile_values = percentiles
        .iter()
        .map(|&p| (p, percentile_level(&durations, total_active_duration_us, p)))
        .collect();

    ParallelismResult {
        percentiles: percentile_values,
        avg_concurrency,
        executed_action_count: intervals.len() as u64,
        total_active_duration_us,
    }
}

/// Smallest concurrency level whose cumulative duration share reaches `percentile`.
/// Compared as integers (`cum * 100 >= total * p`) to avoid float rounding.
fn percentile_level(durations: &BTreeMap<u64, i64>, total: i64, percentile: u32) -> u64 {
    let threshold = (total as i128) * (percentile as i128);
    let mut cumulative: i128 = 0;
    let mut last_level = 0;
    for (level, dur) in durations {
        cumulative += *dur as i128;
        last_level = *level;
        if cumulative * 100 >= threshold {
            return *level;
        }
    }
    last_level
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iv(start_us: i64, end_us: i64) -> ActionInterval {
        ActionInterval { start_us, end_us }
    }

    #[test]
    fn empty_input() {
        let r = compute(&[], &PERCENTILES);
        assert_eq!(r, ParallelismResult::empty(0, &PERCENTILES));
    }

    #[test]
    fn single_action_is_all_level_one() {
        let r = compute(&[iv(0, 1_000_000)], &PERCENTILES);
        assert_eq!(r.executed_action_count, 1);
        assert_eq!(r.total_active_duration_us, 1_000_000);
        assert_eq!(r.avg_concurrency, 1.0);
        assert!(r.percentiles.iter().all(|&(_, level)| level == 1));
    }

    #[test]
    fn isolated_actions_with_gap() {
        // Two non-overlapping actions: 5s then 3s, with a gap between them.
        let r = compute(
            &[iv(0, 5_000_000), iv(10_000_000, 13_000_000)],
            &PERCENTILES,
        );
        assert_eq!(r.executed_action_count, 2);
        // The gap (concurrency 0) is excluded from the active duration.
        assert_eq!(r.total_active_duration_us, 8_000_000);
        assert_eq!(r.avg_concurrency, 1.0);
        assert!(r.percentiles.iter().all(|&(_, level)| level == 1));
    }

    #[test]
    fn overlapping_actions_reach_level_two() {
        // a1 [0,5s], a2 [2s,8s]: 2s solo, 3s overlap, 3s solo => 5s@1, 3s@2.
        let r = compute(&[iv(0, 5_000_000), iv(2_000_000, 8_000_000)], &PERCENTILES);
        assert_eq!(r.total_active_duration_us, 8_000_000);
        // avg = (1*5 + 2*3) / 8 = 11/8.
        assert!((r.avg_concurrency - 1.375).abs() < 1e-9);
        // 5/8 = 62.5% of time at level 1; level 2 covers the top of the CDF.
        let expected: Vec<(u32, u64)> = vec![
            (5, 1),
            (10, 1),
            (25, 1),
            (50, 1),
            (75, 2),
            (90, 2),
            (95, 2),
            (99, 2),
            (100, 2),
        ];
        assert_eq!(r.percentiles, expected);
    }

    #[test]
    fn adjacent_actions_do_not_double_count() {
        // a2 starts exactly when a1 ends: never concurrent.
        let r = compute(&[iv(0, 5_000_000), iv(5_000_000, 10_000_000)], &PERCENTILES);
        assert_eq!(r.total_active_duration_us, 10_000_000);
        assert_eq!(r.avg_concurrency, 1.0);
        assert!(r.percentiles.iter().all(|&(_, level)| level == 1));
    }

    #[test]
    fn zero_length_actions_contribute_nothing() {
        let r = compute(&[iv(100, 100), iv(200, 200)], &PERCENTILES);
        assert_eq!(r.executed_action_count, 2);
        assert_eq!(r.total_active_duration_us, 0);
        assert!(r.percentiles.iter().all(|&(_, level)| level == 0));
    }

    #[test]
    fn empty_result_reports_requested_percentiles() {
        assert_eq!(compute(&[], &[50]).percentiles, vec![(50, 0)]);
        assert_eq!(compute(&[iv(3, 3)], &[50]).percentiles, vec![(50, 0)]);
        assert_eq!(compute(&[iv(0, 1)], &[50]).percentiles, vec![(50, 1)]);
    }

    fn local_action(start_s: i64, exec_s: i64) -> buck2_data::ActionExecutionEnd {
        use buck2_data::CommandExecution;
        use buck2_data::CommandExecutionDetails;
        use buck2_data::CommandExecutionKind;
        use buck2_data::CommandExecutionMetadata;
        use buck2_data::LocalCommand;
        use buck2_data::command_execution_kind::Command;

        buck2_data::ActionExecutionEnd {
            commands: vec![CommandExecution {
                details: Some(CommandExecutionDetails {
                    command_kind: Some(CommandExecutionKind {
                        command: Some(Command::LocalCommand(LocalCommand::default())),
                    }),
                    metadata: Some(CommandExecutionMetadata {
                        start_time: Some(prost_types::Timestamp {
                            seconds: start_s,
                            nanos: 0,
                        }),
                        execution_time: Some(prost_types::Duration {
                            seconds: exec_s,
                            nanos: 0,
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn extract_interval_reads_execution_window() {
        let action = local_action(100, 5);
        let interval = extract_interval(&action).expect("local action has an interval");
        assert_eq!(interval.start_us, 100_000_000);
        assert_eq!(interval.end_us, 105_000_000);
    }

    #[test]
    fn extract_interval_skips_cache_hits() {
        use buck2_data::CommandExecution;
        use buck2_data::CommandExecutionDetails;
        use buck2_data::CommandExecutionKind;
        use buck2_data::CommandExecutionMetadata;
        use buck2_data::RemoteCommand;
        use buck2_data::command_execution_kind::Command;

        let action = buck2_data::ActionExecutionEnd {
            commands: vec![CommandExecution {
                details: Some(CommandExecutionDetails {
                    command_kind: Some(CommandExecutionKind {
                        command: Some(Command::RemoteCommand(RemoteCommand {
                            cache_hit: true,
                            ..Default::default()
                        })),
                    }),
                    metadata: Some(CommandExecutionMetadata {
                        start_time: Some(prost_types::Timestamp {
                            seconds: 1,
                            nanos: 0,
                        }),
                        execution_time: Some(prost_types::Duration {
                            seconds: 1,
                            nanos: 0,
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(extract_interval(&action), None);
    }
}
