/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The harness for buck2's end-to-end tests written in Rust: each test runs the buck2 under test
//! against a project of its own.
//!
//! A test is an `async fn` marked `#[buck_test]`, in a module with the conventions of the Python
//! harness (`tests/README.md`): `test_foo.rs` next to its test data `test_foo_data/`. The test
//! takes a `buck: &Buck` (or `&mut Buck`, to change its environment) and, if it wants one, a
//! `tmp_path: &Path` to a fresh directory, and returns `anyhow::Result<()>` or nothing. Each
//! command takes its arguments as a `&[&str]`.
//!
//! ```ignore
//! use buck2_e2e::prelude::*;
//!
//! #[buck_test]
//! async fn test_build_fails(buck: &Buck) -> anyhow::Result<()> {
//!     buck.build(&["//:good"]).await?;
//!     buck.build(&["//:bad"])
//!         .await
//!         .expect_failure()
//!         .assert_stderr_matches("Error running analysis");
//!     Ok(())
//! }
//! ```
//!
//! The options of `#[buck_test(...)]` are the methods of [`BuckTest`], written `name = value` (or
//! just `name` for `name = true`): `#[buck_test(data_dir = "sub", skip_for_os = ["windows"])]`.
//!
//! When the test passes, the harness runs `buck2 clean`, and fails an isolated test whose daemon
//! panicked unless it sets `allow_daemon_panics`; when it fails, by an error or a panic, it stops
//! the daemon. The project is deleted either way, unless `BUCK_E2E_KEEP_TEMP=1`.
//!
//! Tests read and write their files with `std::fs`: each test runs on a single-threaded runtime of
//! its own, so a short blocking call holds up nothing else. Waiting is different: sleep with
//! `tokio::time::sleep`, so that the commands the test started keep running.

// The `#[buck_test]` expansion names the crate, which this crate's own tests use too.
#[cfg(test)]
extern crate self as buck2_e2e;

pub mod ansi;
pub mod approx;
mod buck;
mod fixture;
pub mod golden;
pub mod http;
pub mod json;
pub mod lsp;
pub mod manifold;
pub mod mode;
mod output;
pub mod piped;
pub mod process;
pub mod subscribe;
pub mod testx;
pub mod utils;

pub use buck2_e2e_macros::buck_test;

pub use crate::buck::Buck;
pub use crate::buck::BuckCommand;
pub use crate::buck::BuckProcess;
pub use crate::fixture::BuckTest;
pub use crate::golden::golden;
pub use crate::output::BuckError;
pub use crate::output::BuckOutput;
pub use crate::output::BuildReport;
pub use crate::output::ExitCode;
pub use crate::output::ExpectFailure;
pub use crate::output::InvocationRecord;

/// What most tests use, including the crates they commonly need, so that test targets do not have
/// to depend on them.
pub mod prelude {
    pub use anyhow;
    pub use futures;
    pub use serde_json;
    pub use tokio;

    pub use crate::Buck;
    pub use crate::ExitCode;
    pub use crate::ExpectFailure;
    pub use crate::buck_test;
    pub use crate::golden;
}

#[doc(hidden)]
pub mod __macro_support {
    pub use crate::macro_support::*;
}

mod macro_support {
    use std::future::Future;
    use std::path::PathBuf;
    use std::pin::Pin;

    use crate::Buck;

    pub type TestResult = anyhow::Result<()>;

    pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

    /// What a test gets.
    pub struct Fixture {
        pub buck: Option<Buck>,
        pub tmp_path: PathBuf,
    }

    pub fn buck(buck: &mut Option<Buck>) -> &mut Buck {
        buck.as_mut().expect("a test that takes `buck` gets one")
    }

    /// What a test may return.
    pub trait IntoTestResult {
        fn into_test_result(self) -> TestResult;
    }

    impl IntoTestResult for () {
        fn into_test_result(self) -> TestResult {
            Ok(())
        }
    }

    impl<E: Into<anyhow::Error>> IntoTestResult for Result<(), E> {
        fn into_test_result(self) -> TestResult {
            self.map_err(Into::into)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::buck_test;

    #[buck_test(inplace = false)]
    async fn test_a_test_without_buck_gets_a_fresh_tmp_path(tmp_path: &Path) {
        assert!(tmp_path.is_dir());
        assert_eq!(std::fs::read_dir(tmp_path).unwrap().count(), 0);
    }

    #[buck_test(inplace = false, skip_for_os = ["darwin", "linux", "windows"])]
    async fn test_a_test_skipped_on_this_os_does_not_run() {
        panic!("a skipped test ran");
    }

    #[buck_test(inplace = false)]
    async fn test_a_test_may_return_a_result() -> anyhow::Result<()> {
        Ok(())
    }
}
