/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! File watcher tests of a directory that takes the path of another. Some file watchers report
//! the rename and nothing for the entries that came with the directory, EdenFS for one; buck
//! must read everything below the path again all the same.

use std::cell::Cell;
use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use buck2_e2e::Buck;
use buck2_e2e::ExpectFailure;
use buck2_e2e::utils::json_get;
use serde_json::Value;
use serde_json::json;

use crate::cmd::SL_USER;
use crate::cmd::run_checked;
use crate::file_watcher_tests::setup_file_watcher_test;
use crate::utils::get_files;

/// A package that a test writes, `root//probe/<name>`, whose build file prints what buck reads
/// below it.
struct Probe<'a> {
    buck: &'a Buck,
    name: &'static str,
    /// How often a build file was written: each differs from the one before, so that buck
    /// evaluates it whatever else it takes for changed.
    written: Cell<usize>,
}

impl<'a> Probe<'a> {
    fn new(buck: &'a Buck, name: &'static str) -> anyhow::Result<Self> {
        let probe = Self {
            buck,
            name,
            written: Cell::new(0),
        };
        fs::create_dir_all(probe.path(""))?;
        Ok(probe)
    }

    fn path(&self, path: &str) -> PathBuf {
        self.buck.cwd().join("probe").join(self.name).join(path)
    }

    /// The label of `file`, a `dir:name` below the package, for a `load`.
    fn load(&self, file: &str, symbol: &str) -> String {
        format!("load(\"//probe/{}/{file}\", {symbol})\n", self.name)
    }

    fn write(&self, path: &str, contents: &str) -> anyhow::Result<()> {
        let path = self.path(path);
        fs::create_dir_all(path.parent().context("a file has a parent")?)?;
        Ok(fs::write(path, contents)?)
    }

    fn rename(&self, from: &str, to: &str) -> anyhow::Result<()> {
        Ok(fs::rename(self.path(from), self.path(to))?)
    }

    /// Moves `d` out of the way and `incoming` to where it was.
    fn replace_d_by_incoming(&self) -> anyhow::Result<()> {
        self.rename("d", "old")?;
        self.rename("incoming", "d")
    }

    fn write_build_file(&self, package: &str, loads: &str, expression: &str) -> anyhow::Result<()> {
        self.written.set(self.written.get() + 1);
        self.write(
            &format!("{package}TARGETS.fixture"),
            &format!(
                "# {}\n{loads}print(\"Probe: \" + json.encode({expression}))  # buildifier: disable=print\n",
                self.written.get()
            ),
        )
    }

    /// What the build file of `package`, a directory below the probe that ends in `/` or is
    /// empty, prints.
    async fn printed(&self, package: &str) -> anyhow::Result<Value> {
        let target = format!("root//probe/{}/{package}", self.name);
        let target = format!("{}:", target.trim_end_matches('/'));
        let stderr = self.buck.targets(&[&target]).await?.stderr;
        let printed = stderr
            .lines()
            .find_map(|line| line.split_once("Probe: ").map(|(_, printed)| printed))
            .with_context(|| format!("Nothing printed: {stderr}"))?;
        Ok(serde_json::from_str(printed)?)
    }

    /// What `expression` is in a build file of the probe that starts with `loads`.
    async fn eval(&self, loads: &str, expression: &str) -> anyhow::Result<Value> {
        self.write_build_file("", loads, expression)?;
        self.printed("").await
    }

    /// The stderr of buck failing to evaluate a build file of the probe that starts with
    /// `loads`.
    async fn eval_fails(&self, loads: &str) -> anyhow::Result<String> {
        self.write_build_file("", loads, "None")?;
        let target = format!("root//probe/{}:", self.name);
        Ok(self.buck.targets(&[&target]).await.expect_failure().stderr)
    }

    /// The files below `dir`, as a glob finds them.
    async fn files(&self, dir: &str) -> anyhow::Result<Value> {
        self.eval("", &format!("glob([\"{dir}/**/*\"])")).await
    }
}

/// Renames a directory onto the path of one that buck has listed, and checks that buck lists the
/// one that is there now.
pub async fn run_replace_directory_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;

    let files = buck.cwd().join("files");
    fs::create_dir(files.join("incoming"))?;
    fs::write(files.join("incoming").join("new"), "")?;
    assert_eq!(
        get_files(buck).await?,
        ["files/abc", "files/d/empty", "files/incoming/new"]
    );

    fs::rename(files.join("d"), files.join("old"))?;
    fs::rename(files.join("incoming"), files.join("d"))?;
    assert_eq!(
        get_files(buck).await?,
        ["files/abc", "files/d/new", "files/old/empty"]
    );
    Ok(())
}

/// A directory below the replaced one is listed again.
pub async fn run_replace_directory_nested_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    nested(buck).await
}

async fn nested(buck: &Buck) -> anyhow::Result<()> {
    let probe = Probe::new(buck, "nested")?;
    probe.write("d/sub/before", "")?;
    probe.write("incoming/sub/after", "")?;
    assert_eq!(probe.files("d").await?, json!(["d/sub/before"]));

    probe.replace_d_by_incoming()?;
    assert_eq!(probe.files("d").await?, json!(["d/sub/after"]));
    Ok(())
}

/// A file that has the name of one in the directory that was there before is read again.
pub async fn run_replace_directory_same_name_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    same_name(buck).await
}

async fn same_name(buck: &Buck) -> anyhow::Result<()> {
    let probe = Probe::new(buck, "same_name")?;
    probe.write("d/defs.bzl", "VALUE = \"before\"\n")?;
    probe.write("incoming/defs.bzl", "VALUE = \"after\"\n")?;
    let load = probe.load("d:defs.bzl", "\"VALUE\"");
    assert_eq!(probe.eval(&load, "VALUE").await?, json!("before"));

    probe.replace_d_by_incoming()?;
    assert_eq!(probe.eval(&load, "VALUE").await?, json!("after"));
    Ok(())
}

/// A file that left with the directory that was there before is not found any more.
pub async fn run_replace_directory_gone_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    gone(buck).await
}

async fn gone(buck: &Buck) -> anyhow::Result<()> {
    let probe = Probe::new(buck, "gone")?;
    probe.write("d/gone.bzl", "VALUE = \"before\"\n")?;
    probe.write("incoming/other", "")?;
    let load = probe.load("d:gone.bzl", "\"VALUE\"");
    assert_eq!(probe.eval(&load, "VALUE").await?, json!("before"));

    probe.replace_d_by_incoming()?;
    let stderr = probe.eval_fails(&load).await?;
    assert!(
        stderr.contains("probe/gone/d/gone.bzl` does not exist"),
        "{stderr}"
    );
    Ok(())
}

/// A `PACKAGE` file that comes with the directory applies to the packages below it.
pub async fn run_replace_directory_package_file_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    package_file(buck).await
}

async fn package_file(buck: &Buck) -> anyhow::Result<()> {
    let probe = Probe::new(buck, "package_file")?;
    let value = "read_package_value(\"probe.value\")";
    probe.write_build_file("d/pkg/", "", value)?;
    probe.write(
        "incoming/PACKAGE",
        "write_package_value(\"probe.value\", \"after\")\n",
    )?;
    probe.write("incoming/pkg/file", "")?;
    assert_eq!(probe.printed("d/pkg/").await?, json!(null));

    probe.replace_d_by_incoming()?;
    probe.write_build_file("d/pkg/", "", value)?;
    assert_eq!(probe.printed("d/pkg/").await?, json!("after"));
    Ok(())
}

/// A directory that comes with the directory, where the one that was there before had a file of
/// that name, is listed.
pub async fn run_replace_directory_file_to_directory_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    file_to_directory(buck).await
}

async fn file_to_directory(buck: &Buck) -> anyhow::Result<()> {
    let probe = Probe::new(buck, "file_to_directory")?;
    probe.write("d/entry.bzl", "VALUE = \"before\"\n")?;
    probe.write("incoming/entry.bzl/inside", "")?;
    let load = probe.load("d:entry.bzl", "\"VALUE\"");
    assert_eq!(probe.eval(&load, "VALUE").await?, json!("before"));

    probe.replace_d_by_incoming()?;
    assert_eq!(probe.files("d").await?, json!(["d/entry.bzl/inside"]));
    Ok(())
}

/// A package that comes with the directory, where buck found no directory before, is loaded.
pub async fn run_replace_directory_missing_to_directory_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    missing_to_directory(buck).await
}

async fn missing_to_directory(buck: &Buck) -> anyhow::Result<()> {
    let probe = Probe::new(buck, "missing_to_directory")?;
    probe.write("d/file", "")?;
    probe.write_build_file("incoming/pkg/", "", "\"after\"")?;
    buck.targets(&["root//probe/missing_to_directory/d/pkg:"])
        .await
        .expect_failure();

    probe.replace_d_by_incoming()?;
    assert_eq!(probe.printed("d/pkg/").await?, json!("after"));
    Ok(())
}

/// A directory renamed onto an empty one, which is the one rename onto a directory that there
/// is, is read.
pub async fn run_replace_empty_directory_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    empty_directory(buck).await
}

async fn empty_directory(buck: &Buck) -> anyhow::Result<()> {
    let probe = Probe::new(buck, "empty_directory")?;
    fs::create_dir(probe.path("d"))?;
    probe.write("incoming/sub/after", "")?;
    assert_eq!(probe.files("d").await?, json!([]));

    probe.rename("incoming", "d")?;
    assert_eq!(probe.files("d").await?, json!(["d/sub/after"]));
    Ok(())
}

/// Two directories that swap paths are both read again.
pub async fn run_swap_directories_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    swap(buck).await
}

async fn swap(buck: &Buck) -> anyhow::Result<()> {
    let probe = Probe::new(buck, "swap")?;
    for name in ["a", "b"] {
        probe.write(
            &format!("{name}/defs.bzl"),
            &format!("VALUE = \"{name}\"\n"),
        )?;
        probe.write(&format!("{name}/only_{name}"), "")?;
    }
    let loads =
        probe.load("a:defs.bzl", "A = \"VALUE\"") + &probe.load("b:defs.bzl", "B = \"VALUE\"");
    let read = "{\"a\": A, \"b\": B, \"files\": glob([\"a/*\", \"b/*\"])}";
    assert_eq!(
        probe.eval(&loads, read).await?,
        json!({
            "a": "a",
            "b": "b",
            "files": ["a/defs.bzl", "a/only_a", "b/defs.bzl", "b/only_b"],
        })
    );

    probe.rename("a", "tmp")?;
    probe.rename("b", "a")?;
    probe.rename("tmp", "b")?;
    assert_eq!(
        probe.eval(&loads, read).await?,
        json!({
            "a": "b",
            "b": "a",
            "files": ["a/defs.bzl", "a/only_b", "b/defs.bzl", "b/only_a"],
        })
    );
    Ok(())
}

/// Renames that buck learns of all at once: three directories that each take the path of the
/// next, and one that is changed under another name and renamed back.
pub async fn run_rename_directories_in_one_batch_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    one_batch(buck).await
}

async fn one_batch(buck: &Buck) -> anyhow::Result<()> {
    let probe = Probe::new(buck, "one_batch")?;
    let mut loads = String::new();
    for name in ["a", "b", "c", "back"] {
        probe.write(
            &format!("{name}/defs.bzl"),
            &format!("VALUE = \"{name}\"\n"),
        )?;
        loads += &probe.load(
            &format!("{name}:defs.bzl"),
            &format!("{} = \"VALUE\"", name.to_uppercase()),
        );
    }
    let read = "[A, B, C, BACK]";
    assert_eq!(
        probe.eval(&loads, read).await?,
        json!(["a", "b", "c", "back"])
    );

    probe.rename("a", "tmp")?;
    probe.rename("b", "a")?;
    probe.rename("c", "b")?;
    probe.rename("tmp", "c")?;
    probe.rename("back", "away")?;
    probe.write("away/defs.bzl", "VALUE = \"changed while away\"\n")?;
    probe.rename("away", "back")?;
    assert_eq!(
        probe.eval(&loads, read).await?,
        json!(["b", "c", "a", "changed while away"])
    );

    // And a change below a path that a directory was renamed to is seen like any other.
    probe.write("back/defs.bzl", "VALUE = \"changed later\"\n")?;
    assert_eq!(
        probe.eval(&loads, read).await?,
        json!(["b", "c", "a", "changed later"])
    );
    Ok(())
}

/// The build files that the last command evaluated.
async fn evaluated(buck: &Buck) -> anyhow::Result<Vec<String>> {
    let log = buck.log(&["show"]).await?.stdout;
    let mut evaluated = Vec::new();
    for line in log.lines() {
        let event: Value = serde_json::from_str(line)?;
        let load = json_get(&event, &["Event", "data", "SpanEnd", "data", "Load"]);
        if let Some(module) = load.and_then(|load| load["module_id"].as_str()) {
            evaluated.push(module.to_owned());
        }
    }
    evaluated.sort();
    Ok(evaluated)
}

/// A file or a directory that is created, changed, deleted or renamed has buck evaluate the
/// package it is in again, and no other: not the package beside it, not the one above, and not
/// the one below, whose build file would be read again if buck took the directory above it for
/// another.
pub async fn run_changes_evaluate_their_package_only_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    let probe = Probe::new(buck, "changes")?;
    let files = "glob([\"**/*\"])";
    for package in ["", "a/", "a/below/", "b/"] {
        probe.write_build_file(package, "", files)?;
        probe.write(&format!("{package}dir/file"), "")?;
    }
    let all = ["root//probe/changes/..."];
    buck.targets(&all).await?;
    assert_eq!(evaluated(buck).await?.len(), 4);
    buck.targets(&all).await?;
    assert_eq!(evaluated(buck).await?, Vec::<String>::new());

    let a = ["root//probe/changes/a:TARGETS.fixture"];
    for created in ["a/created", "a/dir/created"] {
        probe.write(created, "")?;
        buck.targets(&all).await?;
        assert_eq!(evaluated(buck).await?, a, "{created} created");

        probe.write(created, "changed")?;
        buck.targets(&all).await?;
        assert_eq!(
            evaluated(buck).await?,
            Vec::<String>::new(),
            "{created} changed"
        );

        fs::remove_file(probe.path(created))?;
        buck.targets(&all).await?;
        assert_eq!(evaluated(buck).await?, a, "{created} deleted");
    }

    probe.write("a/created_dir/file", "")?;
    buck.targets(&all).await?;
    assert_eq!(evaluated(buck).await?, a);

    fs::remove_dir_all(probe.path("a/created_dir"))?;
    buck.targets(&all).await?;
    assert_eq!(evaluated(buck).await?, a);

    probe.rename("a/dir", "a/renamed")?;
    buck.targets(&all).await?;
    assert_eq!(evaluated(buck).await?, a, "a/dir renamed");
    Ok(())
}

/// The first commit to track a file below a directory adds the directory to the manifest, and
/// going back to the commit before removes it again. Neither changes what is on disk and not
/// tracked, so buck evaluates no package again.
pub async fn run_commit_evaluates_no_untracked_package_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    // The commit is made on another bookmark: with the mergebase, buck would start over.
    run_checked(&["sl", "bookmark", "work"], buck.cwd()).await?;
    let probe = Probe::new(buck, "commit")?;
    for package in ["d/a/", "d/b/"] {
        probe.write_build_file(package, "", "1")?;
    }
    probe.write("d/tracked", "")?;
    let all = ["root//probe/commit/d/..."];
    buck.targets(&all).await?;
    assert_eq!(evaluated(buck).await?.len(), 2);
    buck.targets(&all).await?;
    assert_eq!(evaluated(buck).await?, Vec::<String>::new());

    let tracked = "probe/commit/d/tracked";
    run_checked(&["sl", "add", tracked], buck.cwd()).await?;
    let commit = ["sl", "commit", "-m", "track", "--config", SL_USER, tracked];
    run_checked(&commit, buck.cwd()).await?;
    buck.targets(&all).await?;
    assert_eq!(evaluated(buck).await?, Vec::<String>::new(), "committed");

    run_checked(&["sl", "goto", "main"], buck.cwd()).await?;
    assert!(!probe.path("d/tracked").exists());
    buck.targets(&all).await?;
    assert_eq!(evaluated(buck).await?, Vec::<String>::new(), "back");
    Ok(())
}

/// A directory that a commit adds to the manifest, and that another directory is renamed onto
/// before the next command, is reported as added twice: by source control, which reports the
/// files the commit adds with it, and by EdenFS, which reports nothing for what came with the
/// rename.
pub async fn run_commit_and_replace_directory_test(buck: &Buck) -> anyhow::Result<()> {
    setup_file_watcher_test(buck).await?;
    // The commit is made on another bookmark: with the mergebase, buck would start over.
    run_checked(&["sl", "bookmark", "work"], buck.cwd()).await?;
    let probe = Probe::new(buck, "commit_and_replace")?;
    probe.write("d/tracked", "")?;
    probe.write("incoming/defs.bzl", "VALUE = \"after\"\n")?;
    let load = probe.load("d:defs.bzl", "\"VALUE\"");
    let stderr = probe.eval_fails(&load).await?;
    assert!(
        stderr.contains("probe/commit_and_replace/d/defs.bzl` does not exist"),
        "{stderr}"
    );

    let tracked = "probe/commit_and_replace/d/tracked";
    run_checked(&["sl", "add", tracked], buck.cwd()).await?;
    let commit = ["sl", "commit", "-m", "track", "--config", SL_USER, tracked];
    run_checked(&commit, buck.cwd()).await?;
    fs::remove_file(probe.path("d/tracked"))?;
    probe.rename("incoming", "d")?;
    assert_eq!(probe.eval(&load, "VALUE").await?, json!("after"));
    Ok(())
}
