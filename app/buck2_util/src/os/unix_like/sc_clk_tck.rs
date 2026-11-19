/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::sync::OnceLock;

use buck2_error::BuckErrorContext;
use buck2_error::buck2_error;

#[allow(clippy::absurd_extreme_comparisons)]
pub fn sc_clk_tck() -> buck2_error::Result<u32> {
    static TICKS: OnceLock<u32> = OnceLock::new();
    TICKS
        .get_or_try_init(|| unsafe {
            let rate = libc::sysconf(libc::_SC_CLK_TCK);
            let rate: u32 = rate
                .try_into()
                .buck_error_context("Integer overflow converting ticks per second")?;
            if rate <= 0 || rate > 10_000 {
                return Err(buck2_error!(
                    buck2_error::ErrorTag::CpuStats,
                    "Invalid ticks per second: {}",
                    rate
                ));
            }
            Ok(rate)
        })
        .copied()
}

// Miri does not implement this `sysconf`.
#[cfg(all(test, not(miri)))]
mod tests {
    use crate::os::unix_like::sc_clk_tck::sc_clk_tck;

    #[test]
    fn test_ticks_per_second() {
        // It is always 100.
        assert_eq!(100, sc_clk_tck().unwrap());
    }
}
