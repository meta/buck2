/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cmp::max;
use std::collections::VecDeque;
use std::time::Duration;
use std::time::SystemTime;

/// Maintains sliding windows in an ascending sequence of values
pub struct SlidingWindow {
    duration: Duration,
    queue: VecDeque<(SystemTime, u64)>,
    max_per_sec: Option<u64>,
}

impl SlidingWindow {
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            queue: VecDeque::new(),
            max_per_sec: None,
        }
    }

    /// Report the value at a new timestamp
    ///
    /// Each update value must be greater than or equal to the previous one
    pub fn update(&mut self, timestamp: SystemTime, value: u64) {
        self.queue.push_back((timestamp, value));

        while let Some(current_duration) = self.window_duration() {
            // we want to have at least two elements in window to calculate value per second
            if current_duration > self.duration && self.queue.len() > 2 {
                let _ = self.queue.pop_front();
            } else {
                self.max_per_sec = max(self.max_per_sec, self.current_per_second());
                break;
            }
        }
    }

    pub fn max_per_second(&self) -> Option<u64> {
        self.max_per_sec
    }

    fn current_per_second(&self) -> Option<u64> {
        let (_, first_val) = self.queue.front()?;
        let (_, last_val) = self.queue.back()?;
        let delta_value = last_val.checked_sub(*first_val)?;
        // Non-zero by construction, so the division is safe even below a microsecond.
        let duration = self.window_duration()?;
        let per_second = u128::from(delta_value) * 1_000_000_000 / duration.as_nanos();
        Some(u64::try_from(per_second).unwrap_or(u64::MAX))
    }

    fn window_duration(&self) -> Option<Duration> {
        let (first_ts, _) = self.queue.front()?;
        let (last_ts, _) = self.queue.back()?;
        let duration = last_ts.duration_since(*first_ts).ok()?;
        if duration.is_zero() {
            return None;
        }
        Some(duration)
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Add;
    use std::time::Duration;
    use std::time::SystemTime;

    use super::SlidingWindow;

    #[test]
    fn test_sliding_window() {
        let t0 = SystemTime::UNIX_EPOCH.add(Duration::from_secs(100000));

        let mut windows = SlidingWindow::new(Duration::from_secs(2));
        assert_eq!(None, windows.current_per_second());
        assert_eq!(None, windows.max_per_second());

        windows.update(t0, 100);
        assert_eq!(None, windows.current_per_second());
        assert_eq!(None, windows.max_per_second());

        windows.update(t0.add(Duration::from_secs(1)), 200);
        assert_eq!(Some(100), windows.current_per_second());
        assert_eq!(Some(100), windows.max_per_second());

        windows.update(t0.add(Duration::from_secs(2)), 400);
        assert_eq!(Some(150), windows.current_per_second());
        assert_eq!(Some(150), windows.max_per_second());

        windows.update(t0.add(Duration::from_secs(3)), 450);
        assert_eq!(Some(125), windows.current_per_second());
        assert_eq!(Some(150), windows.max_per_second());
    }

    #[test]
    fn test_sub_microsecond_window() {
        let t0 = SystemTime::UNIX_EPOCH.add(Duration::from_secs(100000));

        let mut windows = SlidingWindow::new(Duration::from_secs(1));
        windows.update(t0, 100);
        windows.update(t0.add(Duration::from_nanos(500)), 200);
        assert_eq!(Some(200_000_000), windows.max_per_second());
    }

    #[test]
    fn test_over_window_size() {
        let t0 = SystemTime::UNIX_EPOCH.add(Duration::from_secs(100000));

        let mut windows = SlidingWindow::new(Duration::from_secs(1));
        assert_eq!(None, windows.current_per_second());
        assert_eq!(None, windows.max_per_second());

        windows.update(t0, 100);
        assert_eq!(None, windows.current_per_second());
        assert_eq!(None, windows.max_per_second());

        windows.update(t0.add(Duration::from_secs(2)), 300);
        assert_eq!(Some(100), windows.current_per_second());
        assert_eq!(Some(100), windows.max_per_second());
    }
}
