/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! retrace: buck2 build-trace explorer.
//!
//! Modules land one reviewable diff at a time; docs/contract.md holds
//! the honesty rules every emitted number follows.

pub mod activity;
pub mod analyses;
pub mod blast;
pub mod command_profile;
pub mod config;
pub mod dag;
pub mod detail;
pub mod execution;
pub mod execution_fetch;
pub mod export;
pub mod findings;
pub mod graph;
pub mod gui;
pub mod help;
pub mod humanize;
pub mod ingest;
pub mod interrupt;
pub mod lanes;
mod parallelism;
pub mod parse;
pub mod quality;
pub mod sim;
pub mod transfer;
pub mod types;
pub mod upload;

/// Absolute path to a `testdata/` fixture, for both the in-crate tests
/// and the acceptance suite.
///
/// The test targets set `RETRACE_TESTDATA` because the fixtures reach a
/// buck test as build artifacts rather than at a source path. Joined
/// through `Path` so the result never mixes separator styles with what
/// `Path` operations downstream produce.
/// Cargo uses the checked-in fixtures; a missing fixture is a test failure,
/// never a silently skipped test. Buck supplies its artifact directory.
pub fn testdata(name: &str) -> String {
    let dir = std::env::var("RETRACE_TESTDATA").unwrap_or_else(|_| {
        match option_env!("CARGO_MANIFEST_DIR") {
            Some(manifest_dir) => format!("{manifest_dir}/testdata"),
            None => panic!("Buck must set RETRACE_TESTDATA"),
        }
    });
    std::path::Path::new(&dir)
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// A scratch path for a test to write a fixture to, distinct from every
/// other path this function hands out.
///
/// The process id alone is not enough. Under buck each test case is its
/// own process, but a cargo build runs the whole lib's tests as threads
/// of one — so two tests naming a file after what they hold share it,
/// and one deletes what the other has yet to read. `stem` carries the
/// extension, which stays at the end of the name.
#[cfg(test)]
pub fn temp_path(stem: &str) -> std::path::PathBuf {
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering;

    static NEXT: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "retrace-{}-{}-{stem}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
