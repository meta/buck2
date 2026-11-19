/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use dupe::Dupe;

/// NOTE: those structs don't need to maintain wire compatibility, we use them only with matching
/// writer & reader.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct MiniperfOutput {
    pub raw_exit_code: Result<i32, String>,
    pub counters: Result<MiniperfCounters, String>,
}

#[derive(
    serde::Serialize,
    serde::Deserialize,
    Copy,
    Clone,
    Dupe,
    PartialEq,
    Debug,
    Default
)]
pub struct MiniperfCounters {
    /// Total instructions executed.
    pub user_instructions: MiniperfCounter,
    pub kernel_instructions: MiniperfCounter,
}

impl MiniperfOutput {
    // This is the size we expect this record to take if the command worked out fine.
    pub const EXPECTED_SIZE: usize = 60;
}

/// The fields here come straight out of `perf_event_open`. The count is
#[derive(
    serde::Serialize,
    serde::Deserialize,
    Copy,
    Clone,
    Dupe,
    PartialEq,
    Debug,
    Default
)]
pub struct MiniperfCounter {
    /// Value of the counter.
    pub count: u64,
    /// Time this counter was active, in ns. Equals time_running unless multiplexing happens.
    pub time_enabled: u64,
    /// Time the measured processes were running, in ns.
    pub time_running: u64,
}

impl MiniperfCounter {
    /// Return the count, adjusted for multiplexing.
    pub fn adjusted_count(&self) -> u64 {
        if self.time_enabled == self.time_running {
            return self.count;
        }

        let ratio = self.time_enabled as f64 / self.time_running as f64;
        (self.count as f64 * ratio) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adjusted_count() {
        assert_eq!(
            MiniperfCounter {
                count: 123,
                time_enabled: 100,
                time_running: 100
            }
            .adjusted_count(),
            123
        );

        assert_eq!(
            MiniperfCounter {
                count: 123,
                time_enabled: 100,
                time_running: 50
            }
            .adjusted_count(),
            246
        );
    }

    #[test]
    fn test_expected_size() {
        let max_counter = MiniperfCounter {
            count: u64::MAX,
            time_enabled: u64::MAX,
            time_running: u64::MAX,
        };
        let output = MiniperfOutput {
            raw_exit_code: Ok(i32::MAX),
            counters: Ok(MiniperfCounters {
                user_instructions: max_counter,
                kernel_instructions: max_counter,
            }),
        };

        assert_eq!(
            bincode::serde::encode_to_vec(&output, bincode::config::legacy())
                .unwrap()
                .len(),
            MiniperfOutput::EXPECTED_SIZE
        );
    }
}
