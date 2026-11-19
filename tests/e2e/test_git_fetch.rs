/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use buck2_e2e::lsp::file_uri;
use buck2_e2e::prelude::*;

fn git(args: &[&str], cwd: &Path) -> anyhow::Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .context("Running git")?;
    anyhow::ensure!(
        output.status.success(),
        "`git {}` failed ({}):\n{}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// Create the repository to fetch from, and return the commit to pin.
///
/// It sits next to the project rather than inside it, so that buck neither sees it as
/// source nor watches it.
fn init_repo(cwd: &Path) -> anyhow::Result<String> {
    let repo = std::path::absolute(
        cwd.parent()
            .context("The project has a parent")?
            .join("remote"),
    )?;
    fs::create_dir(&repo)?;
    git(&["init"], &repo)?;
    git(&["config", "user.name", "notarealuser"], &repo)?;
    git(&["config", "user.email", "notarealuser@fb.com"], &repo)?;
    // git_fetch asks for a commit by hash, which a repository only serves when told to.
    // The hosts it is aimed at allow that; a freshly created repository does not.
    git(&["config", "uploadpack.allowAnySHA1InWant", "true"], &repo)?;

    fs::write(repo.join("hello.txt"), "hello\n")?;
    fs::create_dir(repo.join("subdir"))?;
    fs::write(repo.join("subdir").join("nested.txt"), "nested\n")?;
    git(&["add", "."], &repo)?;
    git(&["commit", "-m", "Commit name"], &repo)?;
    let rev = git(&["log", "--format=format:%H", "-1"], &repo)?;

    let mut buckconfig = fs::OpenOptions::new()
        .append(true)
        .open(cwd.join(".buckconfig"))?;
    write!(
        buckconfig,
        "\n[test_git_fetch]\n  repo = {}\n  rev = {rev}\n",
        file_uri(&repo)
    )?;
    Ok(rev)
}

async fn fetched(buck: &Buck, target: &str) -> anyhow::Result<PathBuf> {
    let res = buck
        .build_without_report(&[target, "--show-full-simple-output"])
        .await?;
    Ok(PathBuf::from(res.stdout.trim()))
}

#[buck_test(inplace = false)]
async fn test_fetches_the_work_tree(buck: &Buck) -> anyhow::Result<()> {
    init_repo(buck.cwd())?;

    let out = fetched(buck, "root//:fetch.git").await?;
    assert_eq!(fs::read_to_string(out.join("hello.txt"))?, "hello\n");
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_requested_paths_are_sub_targets(buck: &Buck) -> anyhow::Result<()> {
    init_repo(buck.cwd())?;

    let out = fetched(buck, "root//:fetch.git[subdir]").await?;
    assert_eq!(fs::read_to_string(out.join("nested.txt"))?, "nested\n");
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_git_dir_sub_target_is_a_repository(buck: &Buck) -> anyhow::Result<()> {
    let rev = init_repo(buck.cwd())?;

    let git_dir = fetched(buck, "root//:fetch.git[.git]").await?;

    // Validate this is a proper .git tree
    assert_eq!(
        git(
            &[
                "--git-dir",
                &git_dir.to_string_lossy(),
                "cat-file",
                "-t",
                &rev
            ],
            buck.cwd()
        )?,
        "commit"
    );
    Ok(())
}
