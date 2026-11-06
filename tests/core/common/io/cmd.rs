/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Running the commands the tests drive the checkout with, like `sl`, as Python's `subprocess`
//! did. Their output goes to the test's.

use std::path::Path;
use std::process::ExitStatus;

use anyhow::Context;

/// Runs `args` in `cwd` and gives its exit status, whatever it is, as `subprocess.run` without
/// `check`.
pub async fn run(args: &[&str], cwd: &Path) -> anyhow::Result<ExitStatus> {
    eprintln!("Running {}", args.join(" "));
    tokio::process::Command::new(args[0])
        .args(&args[1..])
        .current_dir(cwd)
        .status()
        .await
        .with_context(|| format!("Running `{}`", args.join(" ")))
}

/// Runs `args` in `cwd`, failing unless it succeeds, as `subprocess.run` with `check=True`.
pub async fn run_checked(args: &[&str], cwd: &Path) -> anyhow::Result<()> {
    let status = run(args, cwd).await?;
    anyhow::ensure!(status.success(), "`{}` failed: {status}", args.join(" "));
    Ok(())
}

/// The stdout of `args` run in `cwd`, failing unless it succeeds, as `subprocess.check_output`.
pub async fn output(args: &[&str], cwd: &Path) -> anyhow::Result<String> {
    let output = tokio::process::Command::new(args[0])
        .args(&args[1..])
        .current_dir(cwd)
        .stderr(std::process::Stdio::inherit())
        .output()
        .await
        .with_context(|| format!("Running `{}`", args.join(" ")))?;
    anyhow::ensure!(
        output.status.success(),
        "`{}` failed: {}",
        args.join(" "),
        output.status
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_gives_any_exit_status() -> anyhow::Result<()> {
        let status = run(&["sh", "-c", "exit 3"], Path::new("/")).await?;
        assert_eq!(status.code(), Some(3));
        Ok(())
    }

    #[tokio::test]
    async fn test_run_checked_fails_on_an_unsuccessful_exit() -> anyhow::Result<()> {
        run_checked(&["true"], Path::new("/")).await?;
        assert!(run_checked(&["false"], Path::new("/")).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_output_is_stdout_in_the_directory() -> anyhow::Result<()> {
        let dir = std::env::temp_dir();
        let out = output(&["pwd"], &dir).await?;
        assert_eq!(
            std::fs::canonicalize(out.trim())?,
            std::fs::canonicalize(&dir)?
        );
        Ok(())
    }
}
