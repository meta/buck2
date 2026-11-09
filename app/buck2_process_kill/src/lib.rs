/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Building blocks for stopping processes: signals, waits and drop guards.
//!
//! Each one states who reaps what. A process that has exited stays a zombie until its parent
//! waits for it, and until then its pid, which also names its process group if it led one, cannot
//! be reused. So whether a helper may reap decides whether a later signal to that pid is safe.
//!
//! Depends on `libc` alone, so that `buck2_miniperf` can use it.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(unix)]
mod unix;

#[cfg(target_os = "linux")]
pub use crate::linux::*;
#[cfg(unix)]
pub use crate::unix::*;
