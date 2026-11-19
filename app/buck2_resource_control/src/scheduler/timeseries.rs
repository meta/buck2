/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::VecDeque;
use std::time::Duration;
use std::time::Instant;

// Type alias in case we want to change this at some point
type Value = f64;

/// A timeseries of f64s.
pub(crate) struct Timeseries {
    /// The points in the timeseries.
    ///
    /// Ordered from oldest to newest. Invariant: non-empty.
    points: VecDeque<(Instant, Value)>,
    /// How far into the past we keep data
    window_size: Duration,
}

impl Timeseries {
    pub(crate) fn new(window_size: Duration, init_time: Instant, init_point: Value) -> Self {
        Self {
            points: VecDeque::from([(init_time, init_point)]),
            window_size,
        }
    }

    pub(crate) fn add_sample(&mut self, timestamp: Instant, value: Value) {
        assert!(
            self.points.back().unwrap().0 <= timestamp,
            "Non-monotonic time"
        );
        self.points.push_back((timestamp, value));

        while timestamp - self.points.front().unwrap().0 > self.window_size {
            self.points.pop_front();
        }
    }

    /// Assuming a linear interpolation between two adjascent points,
    /// the area under the graph is then average of the two values,
    /// multiplied by the time between them.
    fn segment_integral(&self, start_idx: usize) -> Value {
        let segment_start = self.points[start_idx];
        let segment_end = self.points[start_idx + 1];
        let segment_duration = segment_end.0 - segment_start.0;
        segment_duration.as_secs_f64() * (segment_end.1 + segment_start.1) / 2.0
    }

    /// Returns the average value of the timeseries over some timespan ending at the last data point
    /// and having length the given duration.
    pub(crate) fn average_over_last(&self, duration: Duration) -> Value {
        if duration.is_zero() {
            // We maybe don't expect this but lets just be safe and return a reasonable value
            return self.points.back().unwrap().1;
        }

        let start_time = self.points.back().unwrap().0 - duration;

        let mut total = 0.0;
        let mut segment_end_idx = self.points.len() - 1;
        while segment_end_idx > 0 && self.points[segment_end_idx - 1].0 >= start_time {
            // The entire segment is included in our interval
            total += self.segment_integral(segment_end_idx - 1);
            segment_end_idx -= 1;
        }
        let first_sample_in_range = self.points[segment_end_idx];
        total += (first_sample_in_range.0 - start_time).as_secs_f64() * first_sample_in_range.1;
        total / duration.as_secs_f64()
    }

    /// Predicts average values in the future.
    ///
    /// Given a `prediction` function, which accepts instants in the *future* and returns a
    /// prediction of the instantaneous value of the timeseries at that time, returns an iterator
    /// over what the `average_over_last` values will be for the given duration, at the timestamps
    /// indicated in the iterator.
    ///
    /// Which timestamps are returned in the output iterator is not guaranteed; however, this
    /// function generally works well when the granularity of values in the timeseris is
    /// significantly finer than the duration.
    pub(crate) fn predict_average_over_last_values(
        &self,
        duration: Duration,
        mut prediction: impl FnMut(Instant) -> Value,
    ) -> impl Iterator<Item = (Instant, Value)> {
        if duration.is_zero() {
            panic!("Expected non-zero duration");
        }

        let last_point = self.points.back().unwrap();

        // Find first point in the window ending at the last point
        let mut start_idx = self
            .points
            .iter()
            .position(|(t, _)| *t + duration > last_point.0)
            .unwrap_or(self.points.len());

        // Area under the curve excluding the very last segment
        let mut integral = (start_idx..(self.points.len() - 1))
            .map(|i| self.segment_integral(i))
            .sum::<f64>();
        let mut end_time = last_point.0;
        let mut end_value = last_point.1;

        std::iter::from_fn(move || {
            if start_idx >= self.points.len() {
                return None;
            }

            let window_start_point = self.points[start_idx];
            let emit_time = window_start_point.0 + duration;

            // Add new predicted segment from prev_time to emit_time
            let predicted_value = prediction(emit_time);
            let segment_duration = emit_time - end_time;
            integral += segment_duration.as_secs_f64() * (end_value + predicted_value) / 2.0;
            end_time = emit_time;
            end_value = predicted_value;

            let average = integral / duration.as_secs_f64();

            // Update for next iteration: subtract the segment starting at idx
            if start_idx < self.points.len() - 1 {
                integral -= self.segment_integral(start_idx);
            }
            start_idx += 1;

            Some((emit_time, average))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::time::Instant;

    use crate::scheduler::timeseries::Timeseries;

    struct TestTime(Instant);

    impl TestTime {
        fn new() -> Self {
            Self(Instant::now())
        }

        fn at(&self, t: u64) -> Instant {
            self.0 + Duration::from_secs(t)
        }
    }

    macro_rules! assert_float_eq {
        ($actual:expr, $expected:expr) => {{
            let actual: f64 = $actual;
            let expected: f64 = $expected;
            let rel_diff = (actual - expected).abs() / expected.abs();
            assert!(rel_diff < 1e-5, "{} != {}", actual, expected)
        }};
    }

    #[test]
    fn test_eviction_length_one() {
        let t = TestTime::new();
        let mut ts = Timeseries::new(Duration::from_secs(10), t.at(0), 0.0);

        ts.add_sample(t.at(11), 1.0);
        // Check that the timeseries doesn't know about the 0 anymore
        assert_float_eq!(ts.average_over_last(Duration::from_secs(100)), 1.0);
    }

    #[test]
    fn test_eviction_multi() {
        let t = TestTime::new();
        let mut ts = Timeseries::new(Duration::from_secs(10), t.at(0), 0.0);
        ts.add_sample(t.at(1), 0.0);
        ts.add_sample(t.at(2), 0.0);
        ts.add_sample(t.at(3), 0.0);

        ts.add_sample(t.at(15), 1.0);
        // Check that the timeseries doesn't know about the 0s anymore
        assert_float_eq!(ts.average_over_last(Duration::from_secs(100)), 1.0);
    }

    #[test]
    fn test_average_length_one() {
        let t = TestTime::new();
        let ts = Timeseries::new(Duration::from_secs(10), t.at(0), 134.0);
        assert_float_eq!(ts.average_over_last(Duration::from_secs(10)), 134.0);
    }

    #[test]
    fn test_average_off_by_ones() {
        let t = TestTime::new();
        let mut ts = Timeseries::new(Duration::from_secs(1000), t.at(0), 0.0);
        for i in 1..=10 {
            ts.add_sample(t.at(i), i as f64);
        }

        // This exercises a pretty wide variety of potential edge cases
        for i in 1..=10 {
            assert_float_eq!(
                ts.average_over_last(Duration::from_secs(i)),
                10.0 - (i as f64) / 2.0
            );
        }
    }

    #[test]
    fn test_average_duration_zero() {
        let t = TestTime::new();
        let ts = Timeseries::new(Duration::from_secs(1000), t.at(0), 100.0);
        assert_float_eq!(ts.average_over_last(Duration::ZERO), 100.0);
    }

    #[test]
    fn test_average_between_points() {
        let t = TestTime::new();
        let mut ts = Timeseries::new(Duration::from_secs(1000), t.at(0), 1.0);
        ts.add_sample(t.at(2), 2.0);

        // Obviously maybe not 100% accurate, could do linear interpolation to get a bit more precise
        assert_float_eq!(ts.average_over_last(Duration::from_secs(1)), 2.0);
        assert_float_eq!(ts.average_over_last(Duration::from_secs(3)), 4.0 / 3.0);
    }

    #[test]
    fn test_predict_single_point_constant() {
        let t = TestTime::new();
        let ts = Timeseries::new(Duration::from_secs(1000), t.at(0), 5.0);

        let results: Vec<_> = ts
            .predict_average_over_last_values(Duration::from_secs(10), |_| 5.0)
            .collect();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, t.at(10));
        assert_float_eq!(results[0].1, 5.0);
    }

    #[test]
    fn test_predict_single_point_linear() {
        let t = TestTime::new();
        // Start at time 0 with value 0
        let ts = Timeseries::new(Duration::from_secs(1000), t.at(0), 0.0);

        // Predict linearly: value at time t is t
        let results: Vec<_> = ts
            .predict_average_over_last_values(Duration::from_secs(10), |instant| {
                instant.duration_since(t.at(0)).as_secs_f64()
            })
            .collect();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, t.at(10));
        // Linear from 0 to 10 over 10 seconds: average is 5
        assert_float_eq!(results[0].1, 5.0);
    }

    #[test]
    fn test_predict_emits_correct_timestamps() {
        let t = TestTime::new();
        let mut ts = Timeseries::new(Duration::from_secs(1000), t.at(0), 0.0);
        ts.add_sample(t.at(1), 1.0);
        ts.add_sample(t.at(12), 2.0);
        ts.add_sample(t.at(13), 3.0);

        let results: Vec<_> = ts
            .predict_average_over_last_values(Duration::from_secs(5), |_| 0.0)
            .collect();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, t.at(17));
        assert_eq!(results[1].0, t.at(18));
    }

    #[test]
    fn test_predict_with_historical_and_future() {
        let t = TestTime::new();
        let mut ts = Timeseries::new(Duration::from_secs(1000), t.at(0), 0.0);
        ts.add_sample(t.at(1), 1.0);
        ts.add_sample(t.at(2), 2.0);

        // Predict constant value of 2 (same as last historical)
        let results: Vec<_> = ts
            .predict_average_over_last_values(Duration::from_secs(3), |_| 2.0)
            .collect();

        assert_eq!(results.len(), 3);

        // At t=3: window [0,3], historical [0,2], predicted [2,3]
        // Historical integral: (0+1)/2 * 1 + (1+2)/2 * 1 = 0.5 + 1.5 = 2.0
        // Predicted integral: (2+2)/2 * 1 = 2.0
        // Total = 4.0, average = 4.0/3
        assert_eq!(results[0].0, t.at(3));
        assert_float_eq!(results[0].1, 4.0 / 3.0);

        // At t=4: window [1,4], historical [1,2], predicted [2,4]
        // Historical integral: (1+2)/2 * 1 = 1.5
        // Predicted integral: (2+2)/2 * 2 = 4.0
        // Total = 5.5, average = 5.5/3
        assert_eq!(results[1].0, t.at(4));
        assert_float_eq!(results[1].1, 5.5 / 3.0);

        // At t=5: window [2,5], historical just point at 2, predicted [2,5]
        // Historical integral: 0 (no segments)
        // Predicted integral: (2+2)/2 * 3 = 6.0
        // Total = 6.0, average = 2.0
        assert_eq!(results[2].0, t.at(5));
        assert_float_eq!(results[2].1, 2.0);
    }

    #[test]
    fn test_uses_all_predictions() {
        let t = TestTime::new();
        let mut ts = Timeseries::new(Duration::from_secs(1000), t.at(0), 0.0);
        ts.add_sample(t.at(1), 1.0);
        ts.add_sample(t.at(2), 2.0);

        // Prediction returns 5.0 on first call, then 0.0
        let mut first_prediction = Some(5.0);
        let results: Vec<_> = ts
            .predict_average_over_last_values(Duration::from_secs(3), |_| {
                first_prediction.take().unwrap_or(0.0)
            })
            .collect();

        assert_eq!(results.len(), 3);

        // At t=3: window [0,3], historical [0,2], predicted [2,3]
        // Historical integral: (0+1)/2 * 1 + (1+2)/2 * 1 = 0.5 + 1.5 = 2.0
        // Predicted integral: (2+5)/2 * 1 = 3.5
        // Total = 5.5, average = 5.5/3
        assert_eq!(results[0].0, t.at(3));
        assert_float_eq!(results[0].1, 5.5 / 3.0);

        // At t=4: window [1,4], historical [1,2], predicted [2,4]
        // Historical integral: (1+2)/2 * 1 = 1.5
        // Predicted integral: (2+5)/2 * 1 + (5+0)/2 * 1 = 3.5 + 2.5 = 6.0
        // Total = 7.5, average = 7.5/3 = 2.5
        assert_eq!(results[1].0, t.at(4));
        assert_float_eq!(results[1].1, 7.5 / 3.0);

        // At t=5: window [2,5], historical just point at 2, predicted [2,5]
        // Historical integral: 0 (no segments)
        // Predicted integral: (2+5)/2 * 1 + (5+0)/2 * 1 + (0+0)/2 * 1 = 3.5 + 2.5 + 0 = 6.0
        // Total = 6.0, average = 2.0
        assert_eq!(results[2].0, t.at(5));
        assert_float_eq!(results[2].1, 2.0);
    }
}
