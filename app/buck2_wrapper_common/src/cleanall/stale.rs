/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::process::Stdio;

use buck2_error::BuckErrorContext;
use futures::StreamExt;
use futures::stream;
use tokio::fs;
use tokio::process::Command;

use crate::BUCK2_CLEANALL_ENV_VAR;
use crate::async_background_command;
use crate::cleanall::discovery::CleanallTarget;
use crate::cleanall::discovery::discover_cleanall_targets;

const MAX_CONCURRENT_CLEANS: usize = 8;

/// Env var overriding how many `clean --stale` subprocesses run concurrently.
///
/// Exists so tests can force serialized execution (e.g. `=1`) and verify that
/// a failing target does not prevent later targets from running.
const MAX_CONCURRENT_CLEANS_ENV_VAR: &str = "BUCK2_CLEANALL_MAX_CONCURRENT_CLEANS";

fn max_concurrent_cleans() -> usize {
    std::env::var(MAX_CONCURRENT_CLEANS_ENV_VAR)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|parsed| *parsed > 0)
        .unwrap_or(MAX_CONCURRENT_CLEANS)
}

#[derive(Debug, Eq, PartialEq)]
enum CleanStaleOutcome {
    Cleaned,
    TargetRemoved,
}

impl CleanallTarget {
    fn command(&self) -> Command {
        // Invoking buck2 in subprocess so that different repos with different buck2 releases
        // use corresponding buck2 client and daemon that match their releases.
        let mut command = async_background_command("buck2");
        command
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(&self.project_root)
            .env(BUCK2_CLEANALL_ENV_VAR, "1")
            .arg("--isolation-dir")
            .arg(&self.isolation_dir)
            .args(["clean", "--stale"]);
        command
    }

    /// Whether `buck-out/<isolation-dir>` still exists.
    ///
    /// Callers check before spawning to avoid unnecessary spawns, and re-check
    /// after any clean-stale failure: `buck-out` can be removed concurrently
    /// while clean-stale is running, in which case the failure is mapped to
    /// `TargetRemoved` instead of being reported.
    async fn is_valid(&self) -> bool {
        // Treat any I/O error as also invalid instead of propagating the error
        fs::try_exists(self.project_root.join("buck-out").join(&self.isolation_dir))
            .await
            .unwrap_or(false)
    }

    async fn clean_stale(self) -> buck2_error::Result<CleanStaleOutcome> {
        match self.clean_stale_inner().await {
            Err(error) => {
                // `buck-out` may have been removed concurrently (see `is_valid`).
                if !self.is_valid().await {
                    return Ok(CleanStaleOutcome::TargetRemoved);
                }
                Err(error)
            }
            ok => ok,
        }
    }

    async fn clean_stale_inner(&self) -> buck2_error::Result<CleanStaleOutcome> {
        let child = self.command().spawn().with_buck_error_context(|| {
            format!(
                "Failed to start `buck2 --isolation-dir {} clean --stale` in `{}`",
                self.isolation_dir,
                self.project_root.display(),
            )
        })?;
        let child_output = child.wait_with_output().await.with_buck_error_context(|| {
            format!(
                "Failed to wait for `buck2 --isolation-dir {} clean --stale` in `{}`",
                self.isolation_dir,
                self.project_root.display(),
            )
        })?;
        if child_output.status.success() {
            return Ok(CleanStaleOutcome::Cleaned);
        }

        let stdout = String::from_utf8_lossy(&child_output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&child_output.stderr).into_owned();
        let output = ChildOutput::new(
            format!("{}:{}", self.project_root.display(), self.isolation_dir),
            stdout,
            stderr,
        );

        Err(CleanallError::CleanFailed {
            project_root: self.project_root.clone(),
            isolation_dir: self.isolation_dir.clone(),
            status: child_output.status,
            output,
        }
        .into())
    }
}

#[derive(Debug)]
struct CleanallErrors(Vec<buck2_error::Error>);

#[derive(Debug)]
struct ChildOutput {
    prefix: String,
    stdout: String,
    stderr: String,
}

impl ChildOutput {
    fn new(prefix: String, stdout: String, stderr: String) -> Self {
        Self {
            prefix,
            stdout,
            stderr,
        }
    }

    fn format_stream(&self, name: &str, output: &str) -> String {
        std::iter::once(format!("  [{}] {name}:", self.prefix))
            .chain(
                output
                    .lines()
                    .map(|line| self.format_line(line))
                    .chain(output.is_empty().then(|| self.format_line(""))),
            )
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn format_line(&self, line: &str) -> String {
        if line.is_empty() {
            format!("  [{}]", self.prefix)
        } else {
            format!("  [{}] {line}", self.prefix)
        }
    }
}

impl fmt::Display for ChildOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}\n{}",
            self.format_stream("stdout", &self.stdout),
            self.format_stream("stderr", &self.stderr),
        )
    }
}

impl fmt::Display for CleanallErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "Failed to clean stale Buck2 state:")?;
        for error in &self.0 {
            writeln!(formatter, "- {error:#}")?;
        }
        Ok(())
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Environment)]
enum CleanallError {
    #[error(
        "`buck2 --isolation-dir {isolation_dir} clean --stale` failed in `{}` with status {status}\n{output}",
        project_root.display()
    )]
    CleanFailed {
        project_root: PathBuf,
        isolation_dir: String,
        status: ExitStatus,
        output: ChildOutput,
    },
    #[error("No home directory found for the current user")]
    HomeDirectoryNotFound,
    #[error("{0}")]
    Multiple(CleanallErrors),
}

async fn run_stale_clean_commands(targets: Vec<CleanallTarget>) -> Vec<buck2_error::Error> {
    let total = targets.len();

    let errors: Vec<buck2_error::Error> = stream::iter(targets)
        .map(|target| async move {
            if target.is_valid().await {
                target.clean_stale().await
            } else {
                Ok(CleanStaleOutcome::TargetRemoved)
            }
        })
        .buffer_unordered(max_concurrent_cleans())
        .filter_map(|result| async move { result.err() })
        .collect()
        .await;

    let failed = errors.len();
    let succeeded = total - failed;
    eprintln!("Cleanall --stale: {succeeded} succeeded, {failed} failed");
    errors
}

/// Runs `buck2 clean --stale` for every persisted Buck2 project and isolation directory.
///
/// Returns every child-process failure after all clean commands finish.
pub async fn cleanall_stale() -> buck2_error::Result<()> {
    let Some(home) = crate::buck2_home_dir() else {
        return Err(CleanallError::HomeDirectoryNotFound.into());
    };
    let buckd_root = home.join(".buck").join("buckd");
    let targets = discover_cleanall_targets(&buckd_root).await;
    let errors = run_stale_clean_commands(targets).await;

    if errors.is_empty() {
        Ok(())
    } else {
        Err(CleanallError::Multiple(CleanallErrors(errors)).into())
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;
    use std::path::Path;
    use std::path::PathBuf;

    use super::*;

    #[cfg(unix)]
    const EXPECTED_CLEANALL_ERRORS: &str = concat!(
        "Failed to clean stale Buck2 state:\n",
        "- `buck2 --isolation-dir v2 clean --stale` failed in `/project` with status exit status: 42\n",
        "  [/project:v2] stdout:\n",
        "  [/project:v2] clean stdout\n",
        "  [/project:v2] second stdout line\n",
        "  [/project:v2] stderr:\n",
        "  [/project:v2]\n",
        "- No home directory found for the current user\n",
    );
    #[cfg(windows)]
    const EXPECTED_CLEANALL_ERRORS: &str = concat!(
        "Failed to clean stale Buck2 state:\n",
        "- `buck2 --isolation-dir v2 clean --stale` failed in `/project` with status exit code: 42\n",
        "  [/project:v2] stdout:\n",
        "  [/project:v2] clean stdout\n",
        "  [/project:v2] second stdout line\n",
        "  [/project:v2] stderr:\n",
        "  [/project:v2]\n",
        "- No home directory found for the current user\n",
    );

    #[cfg(unix)]
    fn failed_exit_status() -> ExitStatus {
        ExitStatus::from_raw(42 << 8)
    }

    #[cfg(windows)]
    fn failed_exit_status() -> ExitStatus {
        ExitStatus::from_raw(42)
    }

    #[test]
    fn formats_cleanall_errors() {
        let error: buck2_error::Error = CleanallError::Multiple(CleanallErrors(vec![
            CleanallError::CleanFailed {
                project_root: PathBuf::from("/project"),
                isolation_dir: String::from("v2"),
                status: failed_exit_status(),
                output: ChildOutput::new(
                    String::from("/project:v2"),
                    String::from("clean stdout\nsecond stdout line"),
                    String::new(),
                ),
            }
            .into(),
            CleanallError::HomeDirectoryNotFound.into(),
        ]))
        .into();

        assert_eq!(format!("{error:#}"), EXPECTED_CLEANALL_ERRORS);
    }

    #[test]
    fn builds_stale_clean_command() {
        let target = CleanallTarget {
            project_root: PathBuf::from("/project"),
            isolation_dir: String::from("v2"),
        };

        let command = target.command();
        let command = command.as_std();

        assert_eq!(command.get_program(), OsStr::new("buck2"));
        assert_eq!(command.get_current_dir(), Some(Path::new("/project")));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--isolation-dir", "v2", "clean", "--stale"].map(OsStr::new)
        );
        assert!(
            command.get_envs().any(|(key, value)| key == BUCK2_CLEANALL_ENV_VAR
                && value == Some(OsStr::new("1"))),
            "the spawned clean should be marked as coming from cleanall"
        );
    }

    #[tokio::test]
    async fn validates_buck_out_path() {
        let temp = tempfile::tempdir().expect("temporary directory should be created");
        let target = |project_root, isolation_dir: &str| CleanallTarget {
            project_root,
            isolation_dir: isolation_dir.to_owned(),
        };

        assert!(
            !target(temp.path().to_owned(), "v2").is_valid().await,
            "missing buck-out path should be invalid"
        );
        fs::create_dir_all(temp.path().join("buck-out").join("v2"))
            .await
            .expect("buck-out path should be created");
        assert!(
            target(temp.path().to_owned(), "v2").is_valid().await,
            "existing buck-out path should be valid"
        );
        assert!(
            !target(temp.path().to_owned(), "other").is_valid().await,
            "a different isolation directory should be invalid"
        );
        assert!(
            !target(PathBuf::from("\0"), "v2").is_valid().await,
            "buck-out path check errors should be invalid"
        );
    }

    #[tokio::test]
    async fn ignores_project_root_removed_before_spawn() {
        let temp = tempfile::tempdir().expect("temporary directory should be created");
        let project_root = temp.path().join("project");
        fs::create_dir_all(project_root.join("buck-out").join("v2"))
            .await
            .expect("buck-out path should be created");
        let target = CleanallTarget {
            project_root: project_root.clone(),
            isolation_dir: String::from("v2"),
        };

        assert!(
            target.is_valid().await,
            "project root should initially be valid"
        );
        fs::remove_dir_all(project_root)
            .await
            .expect("project root should be removed");

        let outcome = target
            .clean_stale()
            .await
            .expect("a removed project root should be ignored");
        assert_eq!(outcome, CleanStaleOutcome::TargetRemoved);
    }
}
