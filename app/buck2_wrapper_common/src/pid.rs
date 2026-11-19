/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::num::NonZeroU32;

use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use dupe::Dupe;

/// Process id.
#[derive(Debug, Clone, Copy, Dupe, derive_more::Display)]
#[display("{}", pid)]
pub struct Pid {
    pid: NonZeroU32,
}

impl Pid {
    pub fn from_u32(pid: u32) -> buck2_error::Result<Self> {
        Ok(Pid {
            pid: NonZeroU32::new(pid).internal_error("pid must be non-zero")?,
        })
    }

    pub fn from_i64(pid: i64) -> buck2_error::Result<Self> {
        Self::from_u32(
            pid.try_into()
                .buck_error_context("integer overflow converting pid to u32")?,
        )
    }

    pub fn to_u32(self) -> u32 {
        self.pid.get()
    }

    #[cfg(unix)]
    pub fn to_nix(self) -> buck2_error::Result<nix::unistd::Pid> {
        Ok(nix::unistd::Pid::from_raw(
            self.pid
                .get()
                .try_into()
                .buck_error_context("Integer overflow converting pid to pid_t")?,
        ))
    }
}
