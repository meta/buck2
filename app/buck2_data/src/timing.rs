/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Precision of per-event timing values in the event log.
//!
//! Durations are recorded at two significant figures, which is more than any per-action analysis
//! needs. Full microseconds are close to random bytes that don't compress: on a representative
//! large optimized build, recording the per-action timings at this precision made the event log
//! 1.6% smaller than at full microseconds.

use std::time::Duration;

/// A duration in microseconds at two significant figures, for the event log's `*_us` fields.
///
/// The only way to make one is from a [`Duration`], so every value written to the event log is
/// rounded the same way. Convert to `u64` to assign it: `proto.some_us = Micros::from(d).into()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Micros(u64);

impl From<Duration> for Micros {
    fn from(d: Duration) -> Self {
        Micros(round_micros(
            u64::try_from(d.as_micros()).unwrap_or(u64::MAX),
        ))
    }
}

impl From<Micros> for u64 {
    fn from(m: Micros) -> u64 {
        m.0
    }
}

/// Rounds a microsecond count half up to two significant figures: 8, 28, 190, 6500, 54000.
fn round_micros(us: u64) -> u64 {
    if us < 100 {
        return us;
    }
    let unit = 10u64.pow(us.ilog10() - 1);
    (us / unit + u64::from(us % unit >= unit / 2)).saturating_mul(unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_micros() {
        for (us, rounded) in [
            (0, 0),
            (8, 8),
            (99, 99),
            (100, 100),
            (149, 150),
            (150, 150),
            (999, 1000),
            (1049, 1000),
            (1050, 1100),
            (54047, 54000),
            (u64::MAX, 18_000_000_000_000_000_000),
        ] {
            assert_eq!(round_micros(us), rounded, "{us}");
        }
    }

    #[test]
    fn test_micros() {
        assert_eq!(u64::from(Micros::from(Duration::from_nanos(189_600))), 190);
        assert_eq!(u64::from(Micros::from(Duration::from_micros(6456))), 6500);
    }
}
