/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_error::BuckErrorContext;
use buck2_util::process::async_background_command;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Environment)]
enum SourceControlError {
    #[error("HG command failed with code '{0}' and error '{1}' ")]
    HgCommand(i32, String),
    #[error("Git command failed with code '{0}' and error '{1}' ")]
    GitCommand(i32, String),
}

enum CommandResult {
    Ok(String),
    RepoNotFound,
}

pub async fn get_info() -> buck2_error::Result<String> {
    let hg_info = get_hg_info().await;
    if let Ok(CommandResult::Ok(output)) = hg_info {
        return Ok(output);
    }
    let git_info = get_git_info().await;
    if let Ok(CommandResult::Ok(output)) = git_info {
        return Ok(output);
    }
    hg_info?;
    git_info?;
    Ok("Current directory is not inside a repository (tried hg and git)".to_owned())
}

async fn get_hg_info() -> buck2_error::Result<CommandResult> {
    let result = async_background_command("hg")
        .args(["snapshot", "create"])
        .env("HGPLAIN", "1")
        .output()
        .await?;
    if !result.status.success() {
        let error = from_utf8(result.stderr, "hg snapshot stderr")?;
        if error.contains("is not inside a repository") {
            return Ok(CommandResult::RepoNotFound);
        };
        // On Unix, `code()` will return `None` if the process was terminated by a signal.
        let code = result.status.code().unwrap_or(1);
        return Err(SourceControlError::HgCommand(code, error).into());
    };
    let snapshot = {
        let output = from_utf8(result.stdout, "hg snapshot stdout")?;
        format!("hg snapshot update {output}")
    };

    let result = async_background_command("hg")
        .arg("whereami")
        .output()
        .await?;
    if !result.status.success() {
        let error = from_utf8(result.stderr, "hg whereami stderr")?;
        // On Unix, `code()` will return `None` if the process was terminated by a signal.
        let code = result.status.code().unwrap_or(1);
        return Err(SourceControlError::HgCommand(code, error).into());
    };
    let revision = {
        let output = from_utf8(result.stdout, "hg whereami stdout")?;
        format!("hg revision: {output}")
    };

    Ok(CommandResult::Ok(format!("{revision}{snapshot}")))
}

async fn get_git_info() -> buck2_error::Result<CommandResult> {
    let commit_hash = async_background_command("git")
        .args(["log", "-1", "--format=%H"])
        .output()
        .await?;
    if !commit_hash.status.success() {
        let error = from_utf8(commit_hash.stderr, "git log stderr")?;
        if error.contains("not a git repository") {
            return Ok(CommandResult::RepoNotFound);
        };
        let code = commit_hash.status.code().unwrap_or(1);
        return Err(SourceControlError::GitCommand(code, error).into());
    };

    let status = async_background_command("git")
        .args(["status", "-sb"])
        .output()
        .await?;
    if !status.status.success() {
        let error = from_utf8(status.stderr, "git status stderr")?;
        let code = status.status.code().unwrap_or(1);
        return Err(SourceControlError::GitCommand(code, error).into());
    };
    Ok(CommandResult::Ok(format!(
        "Git base commit hash:\n{}\nGit status:\n{}",
        from_utf8(commit_hash.stdout, "git log stdout")?,
        from_utf8(status.stdout, "git status stdout")?
    )))
}

fn from_utf8(result: Vec<u8>, subject: &str) -> buck2_error::Result<String> {
    String::from_utf8(result).with_buck_error_context(|| format!("{subject} is not utf8"))
}
