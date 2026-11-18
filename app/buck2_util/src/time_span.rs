/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::ops::Add;
use std::ops::Sub;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use allocative::Allocative;
use dupe::Dupe;

/// Errors that can occur when creating a `TimeSpan`.
#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
pub enum TimeSpanError {
    #[error("End time ({end:?}) must be after or equal to start ({start:?}) time")]
    InvalidTimeOrder { start: Instant, end: Instant },
}

/// A time span representing a duration between two instants.
///
/// This struct holds a start and end `Instant` and provides various utilities
/// for working with time spans, including duration calculations, formatting,
/// and serialization.
#[derive(Debug, Clone, Dupe, Copy, PartialEq, Eq, Hash, Allocative)]
pub struct TimeSpan {
    start: Instant,
    end: Instant,
}

impl TimeSpan {
    /// Creates a new `TimeSpan` with the given start and end instants.
    ///
    /// Returns an error if `end` is before `start`.
    pub fn new(start: Instant, end: Instant) -> Result<Self, TimeSpanError> {
        if end >= start {
            Ok(Self { start, end })
        } else {
            Err(TimeSpanError::InvalidTimeOrder { start, end })
        }
    }

    /// Creates a new `TimeSpan` for the given start and end instants.
    ///
    /// If `end` is before `start`, the resulting time span will be equivalent to `TimeSpan::from_start_and_duration(start, Duration::ZERO)`.
    pub fn new_saturating(start: Instant, end: Instant) -> Self {
        Self {
            start,
            end: start.max(end),
        }
    }

    /// Creates a new `TimeSpan` with the given start and end instants without validation.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `end >= start`, otherwise duration calculations
    /// may panic or produce incorrect results.
    pub fn new_unchecked(start: Instant, end: Instant) -> Self {
        Self { start, end }
    }

    /// Creates a `TimeSpan` starting at the given instant with the specified duration.
    pub fn from_start_and_duration(start: Instant, duration: Duration) -> Self {
        Self {
            start,
            end: start + duration,
        }
    }

    /// Creates a `TimeSpan` ending at the given instant with the specified duration.
    pub fn from_end_and_duration(end: Instant, duration: Duration) -> Self {
        Self {
            start: end - duration,
            end,
        }
    }

    /// Creates a `TimeSpan` starting now.
    pub fn start_now() -> TimeSpanBuilder {
        TimeSpanBuilder {
            start: Instant::now(),
        }
    }

    /// Like `start_now`, but also reads the wall clock, for callers that need to place the span
    /// on the client's timeline. Durations still come from the monotonic clock.
    pub fn start_now_with_wall_time() -> AnchoredTimeSpanBuilder {
        AnchoredTimeSpanBuilder {
            span: Self::start_now(),
            start_time: SystemTime::now(),
        }
    }

    /// Creates an empty `TimeSpan` starting now.
    pub fn empty_now() -> Self {
        let now = Instant::now();
        Self::new_unchecked(now, now)
    }

    /// Returns the start instant.
    pub fn start(&self) -> Instant {
        self.start
    }

    /// Returns the end instant.
    pub fn end(&self) -> Instant {
        self.end
    }

    /// Returns the duration of this time span.
    pub fn duration(&self) -> Duration {
        self.end - self.start
    }

    /// Returns the duration in seconds as a floating-point number.
    pub fn duration_secs_f64(&self) -> f64 {
        self.duration().as_secs_f64()
    }

    /// Returns the duration in milliseconds.
    pub fn duration_millis(&self) -> u128 {
        self.duration().as_millis()
    }

    /// Returns the duration in microseconds.
    pub fn duration_micros(&self) -> u128 {
        self.duration().as_micros()
    }

    /// Returns the duration in nanoseconds.
    pub fn duration_nanos(&self) -> u128 {
        self.duration().as_nanos()
    }

    /// Checks if this time span contains the given instant.
    pub fn contains(&self, instant: Instant) -> bool {
        instant >= self.start && instant <= self.end
    }

    /// Checks if this time span overlaps with another time span.
    pub fn overlaps(&self, other: &TimeSpan) -> bool {
        self.start <= other.end && other.start <= self.end
    }

    /// Returns the intersection of this time span with another, if any.
    pub fn intersection(&self, other: &TimeSpan) -> Option<TimeSpan> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);

        if start <= end {
            Some(TimeSpan::new_unchecked(start, end))
        } else {
            None
        }
    }

    /// Returns the union of this time span with another.
    /// The result spans from the earliest start to the latest end.
    pub fn union(&self, other: &TimeSpan) -> TimeSpan {
        TimeSpan::new_unchecked(self.start.min(other.start), self.end.max(other.end))
    }

    /// Extends this time span to include the given instant.
    pub fn extend_to_include(&mut self, instant: Instant) {
        if instant < self.start {
            self.start = instant;
        } else if instant > self.end {
            self.end = instant;
        }
    }

    /// Returns a new time span extended to include the given instant.
    pub fn extended_to_include(&self, instant: Instant) -> TimeSpan {
        let mut result = *self;
        result.extend_to_include(instant);
        result
    }

    /// Shifts this time span by the given duration.
    pub fn shift(&mut self, duration: Duration) {
        self.start += duration;
        self.end += duration;
    }

    /// Returns a new time span shifted by the given duration.
    pub fn shifted(&self, duration: Duration) -> TimeSpan {
        TimeSpan::new_unchecked(self.start + duration, self.end + duration)
    }

    /// Checks if this time span is empty (zero duration).
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Returns a human-readable string representation of the duration.
    pub fn duration_display(&self) -> String {
        format_duration(self.duration())
    }
}

impl fmt::Display for TimeSpan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.duration_display())
    }
}

impl Add<Duration> for TimeSpan {
    type Output = TimeSpan;

    fn add(self, duration: Duration) -> Self::Output {
        self.shifted(duration)
    }
}

impl Sub<Duration> for TimeSpan {
    type Output = TimeSpan;

    fn sub(self, duration: Duration) -> Self::Output {
        TimeSpan::new_unchecked(self.start - duration, self.end - duration)
    }
}

/// A `TimeSpanBuilder` with the wall-clock time read when it started.
pub struct AnchoredTimeSpanBuilder {
    span: TimeSpanBuilder,
    start_time: SystemTime,
}

impl AnchoredTimeSpanBuilder {
    /// The wall-clock time read alongside the monotonic start.
    pub fn start_time(&self) -> SystemTime {
        self.start_time
    }

    /// Completes the time span using the monotonic clock.
    pub fn end_now(self) -> TimeSpan {
        self.span.end_now()
    }
}

/// A builder for creating `TimeSpan` instances that start at a specific time.
pub struct TimeSpanBuilder {
    start: Instant,
}

impl TimeSpanBuilder {
    /// Completes the time span by setting the end time to now.
    pub fn end_now(self) -> TimeSpan {
        TimeSpan::new_unchecked(self.start, Instant::now())
    }

    /// Completes the time span by setting the end time to the given instant.
    pub fn end_at(self, end: Instant) -> Result<TimeSpan, TimeSpanError> {
        TimeSpan::new(self.start, end)
    }

    /// Completes the time span by adding the given duration to the start time.
    pub fn with_duration(self, duration: Duration) -> TimeSpan {
        TimeSpan::new_unchecked(self.start, self.start + duration)
    }
}

/// Formats a duration in a human-readable way.
fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let nanos = duration.subsec_nanos();

    if total_secs == 0 {
        if nanos >= 1_000_000 {
            format!("{:.2}ms", nanos as f64 / 1_000_000.0)
        } else if nanos >= 1_000 {
            format!("{:.2}μs", nanos as f64 / 1_000.0)
        } else {
            format!("{nanos}ns")
        }
    } else if total_secs < 60 {
        format!("{:.2}s", duration.as_secs_f64())
    } else if total_secs < 3600 {
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        format!("{mins}m{secs:02}s")
    } else {
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        let secs = total_secs % 60;
        format!("{hours}h{mins:02}m{secs:02}s")
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    // Reads the wall clock, which Miri's isolation hides.
    #[cfg(not(miri))]
    #[test]
    fn test_anchored_builder_reads_the_wall_clock_at_start() {
        let before = SystemTime::now();
        let builder = TimeSpan::start_now_with_wall_time();
        let after = SystemTime::now();
        assert!(before <= builder.start_time() && builder.start_time() <= after);
    }

    #[test]
    fn test_new_timespan() {
        let start = Instant::now();
        let end = start + Duration::from_millis(100);
        let span = TimeSpan::new(start, end).unwrap();

        assert_eq!(span.start(), start);
        assert_eq!(span.end(), end);
        assert_eq!(span.duration(), Duration::from_millis(100));
    }

    #[test]
    fn test_new_timespan_invalid() {
        let start = Instant::now();
        let end = start - Duration::from_millis(100);
        let result = TimeSpan::new(start, end);

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(TimeSpanError::InvalidTimeOrder { .. })
        ));
    }

    #[test]
    fn test_from_start_and_duration() {
        let start = Instant::now();
        let duration = Duration::from_millis(500);
        let span = TimeSpan::from_start_and_duration(start, duration);

        assert_eq!(span.start(), start);
        assert_eq!(span.duration(), duration);
    }

    #[test]
    fn test_from_end_and_duration() {
        let end = Instant::now();
        let duration = Duration::from_millis(500);
        let span = TimeSpan::from_end_and_duration(end, duration);

        assert_eq!(span.end(), end);
        assert_eq!(span.duration(), duration);
    }

    #[test]
    fn test_builder_pattern() {
        let span = TimeSpan::start_now().with_duration(Duration::from_millis(100));
        assert_eq!(span.duration(), Duration::from_millis(100));
    }

    #[test]
    fn test_contains() {
        let start = Instant::now();
        let middle = start + Duration::from_millis(50);
        let end = start + Duration::from_millis(100);
        let span = TimeSpan::new(start, end).unwrap();

        assert!(span.contains(start));
        assert!(span.contains(middle));
        assert!(span.contains(end));
        assert!(!span.contains(start - Duration::from_millis(1)));
        assert!(!span.contains(end + Duration::from_millis(1)));
    }

    #[test]
    fn test_overlaps() {
        let start1 = Instant::now();
        let end1 = start1 + Duration::from_millis(100);
        let span1 = TimeSpan::new(start1, end1).unwrap();

        let start2 = start1 + Duration::from_millis(50);
        let end2 = start2 + Duration::from_millis(100);
        let span2 = TimeSpan::new(start2, end2).unwrap();

        let start3 = end1 + Duration::from_millis(10);
        let end3 = start3 + Duration::from_millis(100);
        let span3 = TimeSpan::new(start3, end3).unwrap();

        assert!(span1.overlaps(&span2));
        assert!(span2.overlaps(&span1));
        assert!(!span1.overlaps(&span3));
        assert!(!span3.overlaps(&span1));
    }

    #[test]
    fn test_intersection() {
        let start1 = Instant::now();
        let end1 = start1 + Duration::from_millis(100);
        let span1 = TimeSpan::new(start1, end1).unwrap();

        let start2 = start1 + Duration::from_millis(50);
        let end2 = start2 + Duration::from_millis(100);
        let span2 = TimeSpan::new(start2, end2).unwrap();

        let intersection = span1.intersection(&span2).unwrap();
        assert_eq!(intersection.start(), start2);
        assert_eq!(intersection.end(), end1);

        let start3 = end1 + Duration::from_millis(10);
        let end3 = start3 + Duration::from_millis(100);
        let span3 = TimeSpan::new(start3, end3).unwrap();

        assert!(span1.intersection(&span3).is_none());
    }

    #[test]
    fn test_union() {
        let start1 = Instant::now();
        let end1 = start1 + Duration::from_millis(100);
        let span1 = TimeSpan::new(start1, end1).unwrap();

        let start2 = start1 + Duration::from_millis(50);
        let end2 = start2 + Duration::from_millis(100);
        let span2 = TimeSpan::new(start2, end2).unwrap();

        let union = span1.union(&span2);
        assert_eq!(union.start(), start1);
        assert_eq!(union.end(), end2);
    }

    #[test]
    fn test_extend_to_include() {
        let start = Instant::now();
        let end = start + Duration::from_millis(100);
        let mut span = TimeSpan::new(start, end).unwrap();

        let earlier = start - Duration::from_millis(50);
        span.extend_to_include(earlier);
        assert_eq!(span.start(), earlier);

        let later = end + Duration::from_millis(50);
        span.extend_to_include(later);
        assert_eq!(span.end(), later);
    }

    #[test]
    fn test_shift() {
        let start = Instant::now();
        let end = start + Duration::from_millis(100);
        let mut span = TimeSpan::new(start, end).unwrap();
        let duration = span.duration();

        let shift_amount = Duration::from_millis(50);
        span.shift(shift_amount);

        assert_eq!(span.start(), start + shift_amount);
        assert_eq!(span.end(), end + shift_amount);
        assert_eq!(span.duration(), duration);
    }

    #[test]
    fn test_is_empty() {
        let instant = Instant::now();
        let empty_span = TimeSpan::new(instant, instant).unwrap();
        assert!(empty_span.is_empty());

        let non_empty_span = TimeSpan::new(instant, instant + Duration::from_millis(1)).unwrap();
        assert!(!non_empty_span.is_empty());
    }

    #[test]
    fn test_format_duration() {
        #[track_caller]
        fn assert_format(duration: Duration, expected: &str) {
            assert_eq!(format_duration(duration), expected, "for {duration:?}")
        }
        assert_format(Duration::from_nanos(500), "500ns");
        assert_format(Duration::from_nanos(1500), "1.50μs");
        assert_format(Duration::from_micros(1500), "1.50ms");
        assert_format(Duration::from_secs(30), "30.00s");
        assert_format(Duration::from_secs(90), "1m30s");
        assert_format(Duration::from_secs(3661), "1h01m01s");
    }

    #[test]
    fn test_arithmetic_operations() {
        let start = Instant::now();
        let span = TimeSpan::from_start_and_duration(start, Duration::from_millis(100));
        let shift = Duration::from_millis(50);

        let shifted_forward = span + shift;
        assert_eq!(shifted_forward.start(), start + shift);
        assert_eq!(shifted_forward.end(), span.end() + shift);

        let shifted_backward = span - shift;
        assert_eq!(shifted_backward.start(), start - shift);
        assert_eq!(shifted_backward.end(), span.end() - shift);
    }
}
