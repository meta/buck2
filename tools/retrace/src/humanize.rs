/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Human-readable magnitudes for the UI.
//!
//! Use decimal single-unit durations for scanning magnitude, and compound
//! clock durations where readers need to compare long spans precisely.
//! Keep formatting separate from the raw values used for sorting and analysis.

/// Decimal single-unit duration from microseconds. Pairs with a raw value
/// for sorting: display this, sort on the µs integer.
pub fn dur(us: f64) -> String {
    if !us.is_finite() {
        return "n/a".to_owned();
    }
    let us = if us.abs() < 0.5 { 0.0 } else { us };
    let a = us.abs();
    let (v, unit) = if a.round() < 1_000.0 {
        (us, "µs")
    } else if (a / 1e3).round() < 1_000.0 {
        (us / 1e3, "ms")
    } else if (a / 1e6).round() < 90.0 {
        (us / 1e6, "s")
    } else if (a / 60e6).round() < 90.0 {
        (us / 60e6, "minutes")
    } else if (a / 3600e6).round() < 48.0 {
        (us / 3600e6, "hours")
    } else {
        // No years tier on purpose: total build work is idiomatically
        // reported in CPU-days in this project ("559 CPU-days"), and
        // "1.5 years" of aggregate compute reads as a mistake.
        (us / 86400e6, "days")
    };
    // one decimal below 10, none above: "1.5 minutes", "3.4 hours", "134 hours"
    if unit == "µs" || v.abs() >= 10.0 {
        format!("{:.0} {}", v, unit).replace(" µs", "µs")
    } else {
        format!("{:.1} {}", v, unit)
    }
}

/// Clock-style `1h04m09s` / `49m47s` / `8s` from microseconds. For a
/// single long duration the reader is expected to sit with -- a critical
/// path, a build wall -- where `dur`'s one-magnitude "50 minutes" hides
/// the part that distinguishes two runs. Pair it with the raw seconds
/// rather than replacing them: minutes are for judging, seconds are for
/// comparing.
pub fn clock(us: f64) -> String {
    if !us.is_finite() || us < 0.0 {
        return "n/a".to_owned();
    }
    let total = (us / 1e6).round() as i64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Shorten to `max` characters by cutting the middle, never the end.
/// Action names of one target share a long prefix and differ only in
/// the trailing source file, so a right-truncated name is
/// indistinguishable from its siblings; the tail is the part that
/// identifies the row. The tail keeps the larger half of the budget.
pub fn mid_ellipsis(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max || max < 3 {
        return s.to_owned();
    }
    let keep = max - 1;
    let head = keep / 3;
    let tail = keep - head;
    let mut out: String = s.chars().take(head).collect();
    out.push('\u{2026}');
    out.extend(s.chars().skip(n - tail));
    out
}

/// Thousands-separated integer ("599,968") — sortable on the raw value.
pub fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Comma-grouped with one decimal ("5,314,130.4") — for seconds-valued
/// figures in the status bar, where the canonical unit is seconds but the
/// magnitude is unreadable ungrouped.
pub fn num1(v: f64) -> String {
    let neg = v < 0.0;
    let a = v.abs();
    let whole = a.trunc() as u64;
    let frac = ((a - a.trunc()) * 10.0).round() as u64;
    let (whole, frac) = if frac >= 10 {
        (whole + 1, 0)
    } else {
        (whole, frac)
    };
    format!("{}{}.{}", if neg { "-" } else { "" }, count(whole), frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_shapes_match_the_request() {
        assert_eq!(dur(900.0), "900µs");
        assert_eq!(dur(250_000.0), "250 ms");
        assert_eq!(dur(1_000_000.0), "1.0 s");
        assert_eq!(dur(1_500_000.0), "1.5 s");
        assert_eq!(dur(90_000_000.0), "1.5 minutes");
        assert_eq!(dur(12_240_000_000.0), "3.4 hours");
        assert_eq!(dur(484_000_000_000.0), "5.6 days");
        // large enough that a decimal is noise
        assert_eq!(dur(1_559_700_000.0), "26 minutes");
        assert_eq!(dur(5_314_130_400_000.0), "62 days"); // 61.5 rounds up
    }

    #[test]
    fn seconds_group_thousands_with_one_decimal() {
        assert_eq!(num1(0.0), "0.0");
        assert_eq!(num1(1556.6), "1,556.6");
        assert_eq!(num1(5_314_130.44), "5,314,130.4");
        assert_eq!(num1(999.96), "1,000.0"); // carries into the grouping
    }

    #[test]
    fn shortening_keeps_what_tells_names_apart() {
        assert_eq!(mid_ellipsis("short", 72), "short");
        // siblings of one target differ only in the trailing source file
        let a = "fbcode//very/long/target/path:lib (cxx_compile foo/bar/alpha.cpp)";
        let b = "fbcode//very/long/target/path:lib (cxx_compile foo/bar/omega.cpp)";
        assert_ne!(mid_ellipsis(a, 40), mid_ellipsis(b, 40));
        assert!(mid_ellipsis(a, 40).ends_with("alpha.cpp)"));
        assert_eq!(mid_ellipsis(a, 40).chars().count(), 40);
        // multi-byte input must not be split mid-character
        assert_eq!(
            mid_ellipsis("\u{00e9}".repeat(20).as_str(), 5)
                .chars()
                .count(),
            5
        );
    }

    #[test]
    fn clock_is_compound_and_zero_padded() {
        assert_eq!(clock(0.0), "0s");
        assert_eq!(clock(8.4e6), "8s");
        assert_eq!(clock(59.6e6), "1m00s");
        // the case that motivated it: `dur` renders this as "50 minutes"
        assert_eq!(clock(2_986_900_000.0), "49m47s");
        assert_eq!(clock(3_849_000_000.0), "1h04m09s");
        // negatives cannot arise from a duration, but must not underflow
        assert_eq!(clock(-5.0e6), "n/a");
        assert_eq!(clock(f64::NAN), "n/a");
        assert_eq!(clock(f64::INFINITY), "n/a");
        assert_eq!(dur(f64::NAN), "n/a");
        assert_eq!(dur(999.6), "1.0 ms");
        assert_eq!(dur(-0.1), "0µs");
    }

    #[test]
    fn counts_group_thousands() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_000), "1,000");
        assert_eq!(count(599_968), "599,968");
        assert_eq!(count(1_601_203), "1,601,203");
    }
}
