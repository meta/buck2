/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! PGO (Profile Guided Optimization) support for the buck2 daemon.
//!
//! When buck2 is built with `-Cprofile-generate`, the LLVM profiler runtime
//! is linked in and profile data is written to `.profraw` files on process
//! exit. However, the daemon's exit paths often use `libc::_exit()` or
//! `SIGKILL`, which bypass the normal atexit handlers that flush profile data.
//!
//! This module provides `flush_pgo_profile()`, which explicitly calls
//! `__llvm_profile_write_file()` to flush accumulated profile counters to disk.
//!
//! Detection uses weak linkage: we define a weak no-op stub for
//! `__llvm_profile_write_file`. When the binary is built with
//! `-Cprofile-generate`, the LLVM profiler runtime provides a strong
//! definition that overrides our stub. When not instrumented, the weak
//! stub is used (returns -1, signaling PGO is inactive).
//!
//! See LLVM's profile runtime documentation:
//! <https://clang.llvm.org/docs/SourceBasedCodeCoverage.html>

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

// Weak stub for __llvm_profile_write_file. When the LLVM profiler runtime
// is linked (via -Cprofile-generate), its strong definition overrides this.
// When not instrumented, this no-op stub is used instead.
//
// The real function returns 0 on success, non-zero on failure, and appends
// to an existing on-disk profile when called multiple times.
// See: https://clang.llvm.org/docs/SourceBasedCodeCoverage.html
//
// Our stub returns -1 to distinguish "not instrumented" from a real error.
#[cfg(unix)]
#[unsafe(no_mangle)]
#[linkage = "weak"]
pub unsafe extern "C" fn __llvm_profile_write_file() -> i32 {
    -1
}

/// Cached result: `false` means our weak stub is active (no PGO).
/// Once set, avoids repeated FFI calls on non-instrumented builds.
static PGO_INACTIVE: AtomicBool = AtomicBool::new(false);

/// Flush PGO profile data to disk by calling `__llvm_profile_write_file()`.
///
/// Returns `true` if profile data was successfully flushed, `false` if PGO
/// is not active or the write failed.
pub fn flush_pgo_profile() -> bool {
    if PGO_INACTIVE.load(Ordering::Relaxed) {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: Either the real LLVM profiler runtime is linked (flushes
        // counters to disk, returns 0) or our weak stub runs (returns -1).
        let ret = unsafe { __llvm_profile_write_file() };
        if ret == -1 {
            PGO_INACTIVE.store(true, Ordering::Relaxed);
            return false;
        }
        if ret != 0 {
            eprintln!(
                "Warning: __llvm_profile_write_file returned {} (profile flush may have failed)",
                ret
            );
            return false;
        }
        true
    }
    #[cfg(not(unix))]
    {
        PGO_INACTIVE.store(true, Ordering::Relaxed);
        false
    }
}
