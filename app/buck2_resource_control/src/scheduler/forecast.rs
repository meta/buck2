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
use std::time::Instant;

use crate::scheduler::timeseries::Timeseries;

const PRESUMED_OOMD_LOOKBACK: Duration = Duration::from_secs(60);
const APPROX_CURRENT_PRESSURE_WINDOW: Duration = Duration::from_secs(10);

pub(crate) trait PressureForecast: Send + Sync {
    fn estimated_point_of_oom_kill(
        &self,
        pressure: &Timeseries,
        now: Instant,
        oomd_threshold: f64,
    ) -> Option<Instant>;
}

/// Pressure forecast corresponding to the previous variant 2 implementation.
///
/// Start with recent pressure, inflate it using the historic heuristic, and then assume the
/// inflated pressure persists indefinitely.
pub(crate) struct InflatedCurrentPressureForecast;

impl PressureForecast for InflatedCurrentPressureForecast {
    fn estimated_point_of_oom_kill(
        &self,
        pressure: &Timeseries,
        now: Instant,
        oomd_threshold: f64,
    ) -> Option<Instant> {
        // Start with our pressure in the recent past.
        let approx_current_pressure = pressure.average_over_last(APPROX_CURRENT_PRESSURE_WINDOW);
        // And take an educated guess about how much it's likely to increase.
        let estimated_future_pressure = f64::min(
            // Halfway between current value and max.
            (100.0 + approx_current_pressure) / 2.0,
            approx_current_pressure * 1.5,
        );

        pressure
            .predict_average_over_last_values(PRESUMED_OOMD_LOOKBACK, |_| estimated_future_pressure)
            .find(|(_, expected_average_pressure)| *expected_average_pressure > oomd_threshold)
            .map(|(estimated_point_of_oom_kill, _)| estimated_point_of_oom_kill)
            .filter(|estimated_point_of_oom_kill| *estimated_point_of_oom_kill >= now)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn pressure_timeseries(now: Instant, pressure: f64) -> Timeseries {
        let mut timeseries = Timeseries::new(Duration::from_secs(60), now, 0.0);
        for offset in 1..=60 {
            timeseries.add_sample(now + Duration::from_secs(offset), pressure);
        }
        timeseries
    }

    #[test]
    fn test_baseline_forecast_predicts_oom_for_sustained_high_pressure() {
        let now = Instant::now();
        let pressure = pressure_timeseries(now, 80.0);

        let estimated_oom = InflatedCurrentPressureForecast.estimated_point_of_oom_kill(
            &pressure,
            now + Duration::from_secs(60),
            60.0,
        );

        assert!(estimated_oom.is_some());
    }

    #[test]
    fn test_baseline_forecast_does_not_predict_oom_for_low_pressure() {
        let now = Instant::now();
        let pressure = pressure_timeseries(now, 20.0);

        let estimated_oom = InflatedCurrentPressureForecast.estimated_point_of_oom_kill(
            &pressure,
            now + Duration::from_secs(60),
            60.0,
        );

        assert!(estimated_oom.is_none());
    }
}
