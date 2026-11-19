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
use std::time::SystemTime;

use dupe::Dupe;
use tokio::time;
use tokio::time::Instant;
use tokio::time::Interval;
use tokio::time::MissedTickBehavior;

/// A simple wrapper around a [Interval] that tracks information about start/elapsed time and tick numbers. Note
/// that ticks are not necessarily sequential, some may be skipped (and this indicates that ticks are running
/// slower than requested).
pub(crate) struct Ticker {
    interval: Interval,
    current_tps: u32,
}

impl Ticker {
    pub(crate) fn new(ticks_per_second: u32) -> Self {
        let interval_duration = Duration::from_secs_f64(1.0 / (ticks_per_second as f64));
        let mut interval = time::interval(interval_duration);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        Self {
            interval,
            current_tps: ticks_per_second,
        }
    }

    /// Change the tick rate. Takes effect on the next tick.
    pub(crate) fn set_ticks_per_second(&mut self, tps: u32) {
        if tps != self.current_tps && tps > 0 {
            self.current_tps = tps;
            let interval_duration = Duration::from_secs_f64(1.0 / (tps as f64));
            let mut interval = time::interval(interval_duration);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            self.interval = interval;
        }
    }

    pub(crate) async fn tick(&mut self) -> Tick {
        let current = self.interval.tick().await;
        self.tick_at(current)
    }

    pub(crate) fn tick_now(&mut self) -> Tick {
        self.tick_at(Instant::now())
    }

    fn tick_at(&mut self, current: Instant) -> Tick {
        Tick {
            current_monotonic: current,
            current_realtime: SystemTime::now(),
        }
    }
}

/// Information about tick timing.
#[derive(Debug, Clone, Copy)]
pub struct Tick {
    pub current_monotonic: Instant,
    /// The current time, as reported by the system clock
    pub current_realtime: SystemTime,
}

// tokio instant dupe not implemented
impl Dupe for Tick {}

impl Tick {
    pub fn now() -> Tick {
        Self {
            current_monotonic: Instant::now(),
            current_realtime: SystemTime::now(),
        }
    }
}
