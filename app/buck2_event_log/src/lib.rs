/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;
use std::process;
use std::time::Duration;

use buck2_core::buck2_env;
use buck2_core::ci::is_ci;
use buck2_core::soft_error_or_panic;
use buck2_error::BuckErrorContext;
use tokio::process::Child;
use tokio::task::JoinHandle;

pub mod file_names;
pub mod read;
pub mod stream_value;
pub mod tail;
pub mod ttl;
pub mod user_event_types;
pub mod utils;
pub mod write;
pub mod writer;

pub fn should_upload_log() -> buck2_error::Result<bool> {
    if buck2_core::is_open_source() {
        return Ok(false);
    }
    Ok(!buck2_env!(
        "BUCK2_TEST_DISABLE_LOG_UPLOAD",
        bool,
        applicability = testing
    )?)
}

pub fn should_block_on_log_upload() -> buck2_error::Result<bool> {
    // `BUCK2_TEST_BLOCK_ON_UPLOAD` is used by our tests.
    Ok(is_ci()? || buck2_env!("BUCK2_TEST_BLOCK_ON_UPLOAD", bool, applicability = internal)?)
}

/// Reports the failure of a child that uploads a log, when the command ends.
///
/// Waits for the child to exit if the command waits for the upload. Otherwise this only looks
/// at whether the child has exited already: one that did without success was killed or it
/// crashed, and so could not report that itself.
pub async fn wait_for_child_and_log(child: FutureChildOutput, reason: &str) {
    match wait_for_child(child).await {
        Ok(_) => {}
        Err(e) => {
            soft_error_or_panic!(
                "log_upload_child_failed",
                e.context(format!("Error uploading {}", reason))
            );
        }
    }
}

/// The failure of the child, if it has one to report. See `wait_for_child_and_log`.
async fn wait_for_child(child: FutureChildOutput) -> buck2_error::Result<()> {
    let res = if child.wait {
        tokio::time::timeout(Duration::from_secs(20), child.task)
            .await
            .buck_error_context("Timed out")?
    } else if child.task.is_finished() {
        child.task.await
    } else {
        return Ok(());
    };
    let res = res
        .buck_error_context("Task failed")?
        .buck_error_context("Process failed")?;

    if !res.status.success() {
        let stderr = String::from_utf8_lossy(&res.stderr);
        return Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::EventLogUpload,
            "Upload exited with status `{}`. Stderr: `{}`",
            res.status,
            stderr.trim(),
        ));
    };
    Ok(())
}

/// A child that uploads a log. Its output is read while it runs, so that it does not block on
/// its stderr.
pub struct FutureChildOutput {
    task: JoinHandle<io::Result<process::Output>>,
    /// Whether the command waits for the child to exit.
    wait: bool,
}

impl FutureChildOutput {
    /// `wait` is whether the command waits for the child to exit. Its stderr is then piped.
    pub fn new(child: Child, wait: bool) -> Self {
        Self {
            task: tokio::task::spawn(async move { child.wait_with_output().await }),
            wait,
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Stdio;

    use buck2_util::process::async_background_command;

    use super::*;

    /// What is reported of `sh -c <script>` when the command ends, once the script has exited
    /// if `exited`.
    async fn failure(script: &str, wait: bool, exited: bool) -> Option<String> {
        let mut command = async_background_command("sh");
        command.args(["-c", script]).kill_on_drop(true);
        command.stderr(if wait { Stdio::piped() } else { Stdio::null() });
        let child = FutureChildOutput::new(command.spawn().unwrap(), wait);
        while exited && !child.task.is_finished() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        wait_for_child(child).await.err().map(|e| format!("{e:#}"))
    }

    #[tokio::test]
    async fn test_child_that_is_not_waited_for() {
        // Killed, as by an OOM killer.
        let killed = failure("kill -9 $$", false, true).await.unwrap();
        assert!(killed.contains("signal: 9"), "{killed}");
        let crashed = failure("exit 3", false, true).await.unwrap();
        assert!(crashed.contains("exit status: 3"), "{crashed}");

        assert_eq!(failure("true", false, true).await, None);
        // Still uploading: that is not waited for, and not a failure.
        assert_eq!(failure("sleep 600", false, false).await, None);
    }

    #[tokio::test]
    async fn test_child_that_is_waited_for() {
        let failed = failure("echo oops >&2; exit 3", true, false).await.unwrap();
        assert!(failed.contains("exit status: 3"), "{failed}");
        assert!(failed.contains("oops"), "{failed}");

        assert_eq!(failure("true", true, false).await, None);
    }
}
