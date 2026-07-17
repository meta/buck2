/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

/// Format a duration in adaptive, human-readable units (ns/µs/ms/s).
///
/// Unlike [`fmt_duration`], which rounds to tenths of a second (and so renders
/// anything under ~50ms as `0.0s`), this keeps sub-second values legible by
/// picking a unit per value. Use it when durations span a wide range down to the
/// microsecond, e.g. `buck2 log critical-path`.
pub fn fmt_duration_precise(d: Duration) -> String {
    /// Rounds `ns` half-up to a whole number of `step`-nanosecond increments.
    fn round_to(ns: u128, step: u128) -> u128 {
        (ns + step / 2) / step
    }

    // The unit is picked from the *rounded* value rather than the raw one, so
    // that e.g. 999_960ns renders as `1.0ms` instead of `1000.0µs`. Integer
    // arithmetic keeps the rounding exact at the boundaries, where `{:.1}` on
    // an `f64` would be at the mercy of binary representation.
    let ns = d.as_nanos();
    if ns == 0 {
        return "0".to_owned();
    }
    if ns < 1_000 {
        return format!("{ns}ns");
    }
    let tenth_us = round_to(ns, 100);
    if tenth_us < 10_000 {
        return format!("{}.{}µs", tenth_us / 10, tenth_us % 10);
    }
    let tenth_ms = round_to(ns, 100_000);
    if tenth_ms < 10_000 {
        return format!("{}.{}ms", tenth_ms / 10, tenth_ms % 10);
    }
    let hundredth_s = round_to(ns, 10_000_000);
    format!("{}.{:02}s", hundredth_s / 100, hundredth_s % 100)
}

pub fn fmt_duration(elapsed: Duration) -> String {
    let nanos = elapsed.as_nanos().try_into().unwrap_or(u64::MAX);
    let millis = nanos.saturating_add(50_000_000); // Round up.
    let subsec = millis % 1_000_000_000;
    let secs = millis / 1_000_000_000;
    let mins = secs / 60;
    let secs_of_min = secs % 60;
    let hours = mins / 60;
    let mins_of_hour = mins % 60;
    if hours != 0 {
        format!(
            "{}:{:02}:{:02}.{}s",
            hours,
            mins_of_hour,
            secs_of_min,
            subsec / 100_000_000
        )
    } else if mins != 0 {
        format!("{}:{:02}.{}s", mins, secs_of_min, subsec / 100_000_000)
    } else {
        format!("{}.{}s", secs, subsec / 100_000_000)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::fmt_duration::fmt_duration;
    use crate::fmt_duration::fmt_duration_precise;

    #[test]
    fn test_fmt_duration_precise() {
        assert_eq!("0", fmt_duration_precise(Duration::ZERO));
        assert_eq!("1ns", fmt_duration_precise(Duration::from_nanos(1)));
        assert_eq!("999ns", fmt_duration_precise(Duration::from_nanos(999)));
        assert_eq!("1.0µs", fmt_duration_precise(Duration::from_nanos(1_000)));
        assert_eq!("48.0µs", fmt_duration_precise(Duration::from_micros(48)));
        assert_eq!(
            "999.9µs",
            fmt_duration_precise(Duration::from_nanos(999_949))
        );
        assert_eq!("1.0ms", fmt_duration_precise(Duration::from_nanos(999_950)));
        assert_eq!("1.0ms", fmt_duration_precise(Duration::from_micros(1_000)));
        assert_eq!("30.0ms", fmt_duration_precise(Duration::from_millis(30)));
        assert_eq!(
            "999.9ms",
            fmt_duration_precise(Duration::from_micros(999_949))
        );
        assert_eq!(
            "1.00s",
            fmt_duration_precise(Duration::from_micros(999_950))
        );
        assert_eq!("1.00s", fmt_duration_precise(Duration::from_millis(1_000)));
        assert_eq!("1.23s", fmt_duration_precise(Duration::from_millis(1_234)));
        assert_eq!("90.00s", fmt_duration_precise(Duration::from_secs(90)));
    }

    #[test]
    fn test_fmt_duration() {
        fn hmss(h: u64, m: u64, s: u64, ms: u64) -> Duration {
            Duration::from_millis(h * 3_600_000 + m * 60_000 + s * 1000 + ms)
        }

        assert_eq!("0.0s", fmt_duration(hmss(0, 0, 0, 0)));
        assert_eq!("0.0s", fmt_duration(hmss(0, 0, 0, 49)));
        assert_eq!("0.1s", fmt_duration(hmss(0, 0, 0, 50)));
        assert_eq!("0.1s", fmt_duration(hmss(0, 0, 0, 99)));
        assert_eq!("0.1s", fmt_duration(hmss(0, 0, 0, 100)));
        assert_eq!("0.9s", fmt_duration(hmss(0, 0, 0, 949)));
        assert_eq!("1.0s", fmt_duration(hmss(0, 0, 0, 999)));
        assert_eq!("1.0s", fmt_duration(hmss(0, 0, 1, 0)));
        assert_eq!("59.9s", fmt_duration(hmss(0, 0, 59, 949)));
        assert_eq!("1:00.0s", fmt_duration(hmss(0, 0, 59, 999)));
        assert_eq!("1:00.0s", fmt_duration(hmss(0, 1, 0, 0)));
        assert_eq!("59:59.9s", fmt_duration(hmss(0, 59, 59, 949)));
        assert_eq!("1:00:00.0s", fmt_duration(hmss(0, 59, 59, 950)));
        assert_eq!("1:00:00.0s", fmt_duration(hmss(1, 0, 0, 0)));
        assert_eq!("9876:54:32.1s", fmt_duration(hmss(9876, 54, 32, 100)));
    }
}
