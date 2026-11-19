/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Instant;

/// Metadata about scenes that are eligible to be suspended.
///
/// Candidates are passed in actual suspension order, meaning the first element is the next scene
/// that would be suspended if we issue a single suspend.
pub(crate) struct SuspendCandidate {
    #[allow(dead_code)] // Used by upcoming suspend timing implementations.
    pub(crate) memory_current: u64,
}

pub(crate) trait SuspendTiming: Send + Sync {
    fn suspends_to_issue(
        &mut self,
        now: Instant,
        estimated_point_of_oom_kill: Instant,
        candidates: &[SuspendCandidate],
    ) -> usize;

    fn on_enter_decrease_mode(&mut self, _entered_at: Instant) {}
}

/// Suspend timing corresponding to the previous variant 2 implementation.
///
/// Once an OOM is predicted, spread suspends across half the currently running scenes, rounded up.
pub(crate) struct SpreadHalfSuspendTiming {
    last_suspend_or_decrease_time: Option<Instant>,
}

impl SpreadHalfSuspendTiming {
    pub(crate) fn new() -> Self {
        Self {
            last_suspend_or_decrease_time: None,
        }
    }
}

impl SuspendTiming for SpreadHalfSuspendTiming {
    fn suspends_to_issue(
        &mut self,
        now: Instant,
        estimated_point_of_oom_kill: Instant,
        candidates: &[SuspendCandidate],
    ) -> usize {
        // Spread suspends across half the scenes (rounded up). Integer division avoids the
        // panics of `div_f64`; `max(1)` guards the empty-candidates case.
        let divisor = u32::try_from(candidates.len().div_ceil(2))
            .unwrap_or(u32::MAX)
            .max(1);
        let suspend_interval = estimated_point_of_oom_kill.saturating_duration_since(now) / divisor;
        let suspends_to_issue = match self.last_suspend_or_decrease_time {
            Some(last_suspend_or_decrease_time) => {
                usize::from(now - last_suspend_or_decrease_time >= suspend_interval)
            }
            None => 0,
        };
        let suspends_to_issue = suspends_to_issue.min(candidates.len());
        if suspends_to_issue > 0 {
            self.last_suspend_or_decrease_time = Some(now);
        }
        suspends_to_issue
    }

    fn on_enter_decrease_mode(&mut self, entered_at: Instant) {
        self.last_suspend_or_decrease_time = Some(entered_at);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::time::Instant;

    use super::*;

    fn candidates() -> [SuspendCandidate; 2] {
        [
            SuspendCandidate { memory_current: 1 },
            SuspendCandidate { memory_current: 2 },
        ]
    }

    /// With no candidates nothing can be suspended, so no suspend is requested.
    #[test]
    fn test_no_candidates() {
        let mut timing = SpreadHalfSuspendTiming::new();
        let now = Instant::now();
        timing.on_enter_decrease_mode(now);
        assert_eq!(timing.suspends_to_issue(now, now, &[]), 0);
    }

    #[test]
    fn test_suspend_timing_waits_when_not_yet_due() {
        let mut timing = SpreadHalfSuspendTiming::new();
        let now = Instant::now();
        timing.on_enter_decrease_mode(now - Duration::from_secs(10));
        let decision = timing.suspends_to_issue(now, now + Duration::from_secs(20), &candidates());

        assert_eq!(decision, 0);
    }

    #[test]
    fn test_suspend_timing_suspends_when_due() {
        let mut timing = SpreadHalfSuspendTiming::new();
        let now = Instant::now();
        timing.on_enter_decrease_mode(now - Duration::from_secs(20));
        let decision = timing.suspends_to_issue(now, now + Duration::from_secs(20), &candidates());

        assert_eq!(decision, 1);
    }

    #[test]
    fn test_suspend_timing_does_not_suspend_before_entering_decrease_mode() {
        let mut timing = SpreadHalfSuspendTiming::new();
        let now = Instant::now();
        let decision = timing.suspends_to_issue(now, now + Duration::from_secs(20), &candidates());

        assert_eq!(decision, 0);
    }

    #[test]
    fn test_suspend_timing_resets_when_reentering_decrease_mode() {
        let mut timing = SpreadHalfSuspendTiming::new();
        let now = Instant::now();
        timing.on_enter_decrease_mode(now - Duration::from_secs(20));
        assert_eq!(
            timing.suspends_to_issue(now, now + Duration::from_secs(20), &candidates()),
            1
        );
        timing.on_enter_decrease_mode(now - Duration::from_secs(5));

        let decision = timing.suspends_to_issue(now, now + Duration::from_secs(20), &candidates());

        assert_eq!(decision, 0);
    }
}
