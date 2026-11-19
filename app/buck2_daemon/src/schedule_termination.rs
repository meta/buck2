/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::process;
use std::thread;
use std::time::Duration;

use buck2_core::warn_user_or_panic;
use buck2_util::process_stats::process_cpu_time_us;
use buck2_util::threads::thread_spawn;

fn elapsed_cpu_time_as_percents(
    cpu_time_before_us: Option<u64>,
    cpu_time_after_us: Option<u64>,
    duration: Duration,
) -> Option<u64> {
    let cpu_time_before = cpu_time_before_us?;
    let cpu_time_after = cpu_time_after_us?;
    let elapsed_cpu_time_us = cpu_time_after.checked_sub(cpu_time_before)?;
    let elapsed_cpu_time_us_avg_per_second = elapsed_cpu_time_us.checked_div(duration.as_secs())?;
    elapsed_cpu_time_us_avg_per_second.checked_div(1_000_000 / 100)
}

/// Our tests sometimes don't exit Buck 2 cleanly, and they might not get an oppportunity to do so
/// if they are terminated. This allows the daemon to self-destruct.
pub(crate) fn maybe_schedule_termination() -> buck2_error::Result<()> {
    if let Some(duration) =
        buck2_core::buck2_env!("BUCK2_TERMINATE_AFTER", type=u64, applicability=testing)?
            .map(Duration::from_secs)
            .or_else(buck2_common::self_test_timeout::until_post_test_shutdown)
    {
        thread_spawn("buck2-terminate-after", move || {
            const MEASURE_CPU_TIME_FOR: Duration = Duration::from_millis(10);
            let (sleep_before, sleep_after) = match duration.checked_sub(MEASURE_CPU_TIME_FOR) {
                Some(sleep_before) => (sleep_before, MEASURE_CPU_TIME_FOR),
                None => (Duration::ZERO, duration),
            };

            thread::sleep(sleep_before);
            let process_cpu_time_us_before = process_cpu_time_us();
            thread::sleep(sleep_after);
            let process_cpu_time_us_after = process_cpu_time_us();

            let elapsed_cpu_time_avg_in_percents = elapsed_cpu_time_as_percents(
                process_cpu_time_us_before,
                process_cpu_time_us_after,
                sleep_after,
            );
            let msg = if let Some(elapsed_cpu_time_avg_in_percents) =
                elapsed_cpu_time_avg_in_percents
            {
                format!(
                    "Buck is exiting after {duration:?} elapsed; avg process CPU in the last {sleep_after:?} is {elapsed_cpu_time_avg_in_percents}%"
                )
            } else {
                format!("Buck is exiting after {duration:?} elapsed")
            };
            warn_user_or_panic!(
                "schedule_termination_exit",
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Environment,
                    "{}",
                    msg
                ),
                task: false,
            );
            process::exit(1);
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::schedule_termination::elapsed_cpu_time_as_percents;

    #[test]
    fn test_elapsed_cpu_time_as_percents() {
        // 12 seconds wall time
        // 6 seconds of CPU time
        // equivalent to 50% CPU usage
        assert_eq!(
            Some(50),
            elapsed_cpu_time_as_percents(Some(1_000_123), Some(7_000_123), Duration::from_secs(12))
        );
    }
}
