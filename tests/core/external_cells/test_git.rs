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
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use buck2_e2e::prelude::*;

fn repo(cwd: &Path) -> PathBuf {
    cwd.parent()
        .expect("the project directory has a parent")
        .join("external")
}

fn git(args: &[&str], cwd: &Path) -> anyhow::Result<String> {
    eprintln!("Running {}", args.join(" "));
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo(cwd))
        .output()
        .context("Running git")?;
    anyhow::ensure!(
        output.status.success(),
        "git {} failed: {}\n{}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let out = String::from_utf8(output.stdout)?.trim().to_owned();
    eprintln!("Git output: {out}");
    Ok(out)
}

fn git_commit(cwd: &Path) -> anyhow::Result<String> {
    git(&["add", "."], cwd)?;
    git(&["commit", "-m", "Commit name"], cwd)?;
    git(&["log", "--format=format:%H%n", "-1", "-r", "."], cwd)
}

fn set_revision(rev: &str, cwd: &Path) -> anyhow::Result<()> {
    let p = cwd.join(".buckconfig");
    let text = fs::read_to_string(&p)?;
    let lines: Vec<&str> = text.lines().collect();
    let mut data: Vec<String> = lines[..lines.len() - 2]
        .iter()
        .map(|l| (*l).to_owned())
        .collect();
    data.push(format!("  git_origin = file://{}", repo(cwd).display()));
    data.push(format!("  commit_hash = {rev}"));
    fs::write(&p, data.join("\n"))?;
    Ok(())
}

fn copy_tree(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

fn init_repo(cwd: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(repo(cwd))?;
    git(&["init"], cwd)?;
    git(&["config", "user.name", "notarealuser"], cwd)?;
    git(&["config", "user.email", "notarealuser@fb.com"], cwd)?;
    copy_tree(&cwd.join("template"), &repo(cwd))?;
    let rev = git_commit(cwd)?;
    set_revision(&rev, cwd)
}

fn read_output(stdout: &str) -> anyhow::Result<String> {
    Ok(fs::read_to_string(stdout.trim())?.trim().to_owned())
}

/// Deletes `path`, whose files git makes read-only, which Windows refuses to delete.
fn remove_tree(path: &Path) -> anyhow::Result<()> {
    fn make_writable(path: &Path) -> std::io::Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                make_writable(&entry.path())?;
            } else {
                let mut permissions = entry.metadata()?.permissions();
                #[expect(
                    clippy::permissions_set_readonly_false,
                    reason = "it only clears the read-only attribute on Windows"
                )]
                permissions.set_readonly(false);
                fs::set_permissions(entry.path(), permissions)?;
            }
        }
        Ok(())
    }
    if cfg!(windows) {
        make_writable(path)?;
    }
    fs::remove_dir_all(path)?;
    Ok(())
}

#[buck_test]
async fn test_expand_external(buck: &Buck) -> anyhow::Result<()> {
    init_repo(buck.cwd())?;
    buck.expand_external_cell(&["libfoo"]).await?;
    assert!(buck.cwd().join("libfoo").join("src.txt").exists());
    assert!(
        fs::read_to_string(buck.cwd().join("libfoo").join(".buckconfig"))?.contains("buildfile")
    );
    Ok(())
}

#[buck_test]
async fn test_non_master_ancestor(buck: &Buck) -> anyhow::Result<()> {
    init_repo(buck.cwd())?;

    git(&["switch", "-c", "other"], buck.cwd())?;
    fs::write(repo(buck.cwd()).join("src.txt"), "change")?;
    let rev = git_commit(buck.cwd())?;
    set_revision(&rev, buck.cwd())?;

    git(&["switch", "master"], buck.cwd())?;
    fs::write(repo(buck.cwd()).join("src.txt"), "change2")?;
    git_commit(buck.cwd())?;

    let res = buck
        .build_without_report(&["libfoo//:t", "--show-full-simple-output"])
        .await?;
    assert_eq!(read_output(&res.stdout)?, "change");
    Ok(())
}

#[buck_test]
async fn test_changing_commit(buck: &Buck) -> anyhow::Result<()> {
    init_repo(buck.cwd())?;

    let res = buck
        .build_without_report(&["libfoo//:t", "--show-full-simple-output"])
        .await?;
    assert_eq!(read_output(&res.stdout)?, "");

    fs::write(repo(buck.cwd()).join("src.txt"), "change")?;
    let rev = git_commit(buck.cwd())?;
    set_revision(&rev, buck.cwd())?;

    let res = buck
        .build_without_report(&["libfoo//:t", "--show-full-simple-output"])
        .await?;
    assert_eq!(read_output(&res.stdout)?, "change");
    Ok(())
}

#[buck_test]
async fn test_full_clean_cycle(buck: &Buck) -> anyhow::Result<()> {
    init_repo(buck.cwd())?;

    let res = buck
        .build_without_report(&["libfoo//:t[src]", "--show-full-simple-output"])
        .await?;
    let src_path = PathBuf::from(res.stdout.trim());

    buck.clean(&[]).await?;

    assert!(!src_path.exists());
    Ok(())
}

/// Changing the commit hash of a git external cell causes a full rebuild
/// even when the file contents are identical. This test uses a non-deterministic
/// action to detect whether a rebuild occurred: if the outputs differ, the
/// action was re-executed rather than cached.
///
/// When external cell caching is fixed to be content-based, this assertion
/// should flip to `output1 == output2`.
#[buck_test]
async fn test_noop_commit_change_causes_rebuild(buck: &Buck) -> anyhow::Result<()> {
    init_repo(buck.cwd())?;

    let res1 = buck
        .build_without_report(&[
            "libfoo//:nondeterministic",
            "--show-full-simple-output",
            "--local-only",
        ])
        .await?;
    let output1 = read_output(&res1.stdout)?;

    // New commit that adds an unrelated file (no build-relevant changes)
    fs::write(repo(buck.cwd()).join("README.md"), "unrelated file")?;
    let rev = git_commit(buck.cwd())?;
    set_revision(&rev, buck.cwd())?;

    let res2 = buck
        .build_without_report(&[
            "libfoo//:nondeterministic",
            "--show-full-simple-output",
            "--local-only",
        ])
        .await?;
    let output2 = read_output(&res2.stdout)?;

    // Currently rebuilds (different output). After fix: should be cached (same output).
    assert_ne!(output1, output2);
    Ok(())
}

#[buck_test]
async fn test_no_refetch_on_restart(buck: &Buck) -> anyhow::Result<()> {
    init_repo(buck.cwd())?;

    buck.build(&["libfoo//:t"]).await?;
    buck.kill().await?;

    remove_tree(&repo(buck.cwd()))?;
    buck.build(&["libfoo//:t"]).await?;
    Ok(())
}
