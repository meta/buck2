/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

/// Simple wrapper for perf-event to measure the number of instructions
/// executed by a current thread.
pub struct PerThreadInstructionCounter {
    #[cfg(target_os = "linux")]
    counter: perf_event::Counter,
    #[cfg(not(target_os = "linux"))]
    non_linux: std::convert::Infallible,
}

impl PerThreadInstructionCounter {
    /// Create a new instruction counter.
    ///
    /// Return `Err` is `perf_event` failed, `None` on unsupported platforms.
    pub fn init() -> buck2_error::Result<Option<PerThreadInstructionCounter>> {
        Self::init_impl()
    }

    #[cfg(target_os = "linux")]
    fn init_impl() -> buck2_error::Result<Option<PerThreadInstructionCounter>> {
        let mut builder = perf_event::Builder::new()
            .observe_self()
            .any_cpu()
            .kind(perf_event::events::Hardware::INSTRUCTIONS);
        builder.inherit(false);
        let mut counter = builder.build()?;
        counter.enable()?;
        Ok(Some(PerThreadInstructionCounter { counter }))
    }

    #[cfg(not(target_os = "linux"))]
    fn init_impl() -> buck2_error::Result<Option<PerThreadInstructionCounter>> {
        Ok(None)
    }

    /// Collect the number of instructions executed by the thread, along with
    /// the running/enabled times.
    ///
    /// `Err` means the collection itself failed. `Ok(None)` means the counter
    /// was never scheduled — hardware counters exhausted for the whole run,
    /// e.g. by a co-tenant's pinned events — a normal outcome on shared
    /// hosts.
    pub fn collect(self) -> buck2_error::Result<Option<CollectedCount>> {
        self.collect_impl()
    }

    #[cfg(target_os = "linux")]
    fn collect_impl(mut self) -> buck2_error::Result<Option<CollectedCount>> {
        self.counter.disable()?;
        let count = self.counter.read_count_and_time()?;
        if count.time_running == 0 {
            return Ok(None);
        }
        let scaled =
            (count.count as u128) * (count.time_enabled as u128) / (count.time_running as u128);
        Ok(Some(CollectedCount {
            count: scaled as u64,
            time_enabled: count.time_enabled,
            time_running: count.time_running,
        }))
    }

    #[cfg(not(target_os = "linux"))]
    fn collect_impl(self) -> buck2_error::Result<Option<CollectedCount>> {
        match self.non_linux {}
    }
}

/// An instruction count together with the counter scheduling times that
/// qualify it (nanoseconds, from the kernel's `read_format`).
///
/// When the PMU was contended the count is a linear extrapolation from the
/// slices the counter was actually scheduled for; the `time_running /
/// time_enabled` ratio is the fraction actually measured, and so bounds how
/// inaccurate the count may be.
pub struct CollectedCount {
    pub count: u64,
    pub time_enabled: u64,
    pub time_running: u64,
}

impl CollectedCount {
    /// Fraction of the run the counter actually held a hardware slot
    /// (`time_running / time_enabled`, in `(0.0, 1.0]`): the share of the
    /// count that is measurement rather than extrapolation.
    pub fn get_sampling_ratio(&self) -> f64 {
        self.time_running as f64 / self.time_enabled as f64
    }
}

// `perf_event_open` and the inline assembly of the test workload are beyond Miri.
#[cfg(all(test, not(miri)))]
mod tests {
    use std::env;

    use three_billion_instructions::three_billion_instructions;

    use crate::per_thread_instruction_counter::PerThreadInstructionCounter;

    fn is_github_actions() -> bool {
        // Set by GitHub Actions:
        // https://docs.github.com/en/actions/learn-github-actions/variables
        env::var("GITHUB_ACTIONS").is_ok()
    }

    #[allow(unreachable_code)] // Compiler says it is uninhabited on non-linux platforms.
    #[allow(unused_variables)] // This seems like a compiler bug.
    #[test]
    fn test_perf_thread_instruction_counter() {
        if is_github_actions() {
            // Fails with permission denied on GitHub Actions CI.
            return;
        }

        if !cfg!(target_os = "linux") {
            assert!(PerThreadInstructionCounter::init().unwrap().is_none());
        } else {
            // The workload runs ~3 billion instructions, but the exact measured
            // count drifts a few percent: enable/collect boundaries and compiler
            // codegen shave a little off the bottom, while counter multiplexing on
            // contended CI hosts (the `time_enabled/time_running` scaling above) can
            // inflate it. Use a wide tolerance so this stays a "counter is roughly
            // working" sanity check rather than an exact-count assertion.
            //
            // A single measurement can also fail benignly: each CPU has a
            // small fixed number of hardware counters, so ours can allocate
            // but never get a hardware slot (`time_running == 0`, "No
            // counter data collected") — or hold one only briefly, making
            // the count mostly extrapolation. Production tolerates the
            // former by dropping the metric (`init().ok()` /
            // `collect().ok()` around the starlark evaluator); here we
            // additionally refuse to judge the count unless the counter was
            // scheduled for almost the whole run, so extrapolation noise
            // can never fail (or pass) the assertion. Retry contended
            // samples a few times; if no attempt yields a trustworthy
            // sample, the host is too contended to measure on — skip. A
            // trustworthy sample outside the window is a real failure.
            const ATTEMPTS: usize = 3;
            #[cfg_attr(
                not(target_os = "linux"),
                expect(
                    unused_mut,
                    reason = "`rejected` is only mutated in the linux-only branch"
                )
            )]
            let mut rejected = Vec::new();
            for _ in 0..ATTEMPTS {
                let counter = PerThreadInstructionCounter::init().unwrap().unwrap();
                three_billion_instructions().unwrap();
                // A collection failure is a real bug; only scheduling
                // outcomes are retried.
                match counter.collect().unwrap() {
                    Some(c) if c.get_sampling_ratio() >= 0.95 => {
                        assert!(
                            (2_900_000_000..=3_200_000_000).contains(&c.count),
                            "instruction count {} outside expected ~3 billion range \
                             (scheduled {}ns of {}ns)",
                            c.count,
                            c.time_running,
                            c.time_enabled,
                        );
                        return;
                    }
                    Some(c) => rejected.push(format!(
                        "undersampled: scheduled {}ns of {}ns",
                        c.time_running, c.time_enabled
                    )),
                    None => rejected.push("never scheduled".to_owned()),
                }
            }
            eprintln!("skipping: no trustworthy sample; PMU contended: {rejected:?}");
        }
    }
}
