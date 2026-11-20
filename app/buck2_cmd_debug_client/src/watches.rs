/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Report project directories the daemon holds no inotify watch for.
//!
//! A directory that never got a watch makes every later change under it invisible for the life of
//! the daemon: no `File changed:` line, no invalidation, and a build that quietly uses stale
//! contents. Nothing inside buck2 can see that state today, so this reads the daemon's watch
//! descriptors out of `/proc` and compares them against the tree.
//!
//! Debugging aid, Linux only. Directories excluded by `[project] ignore` are reported too, since
//! resolving cell ignores needs the daemon; only the top of each unwatched subtree is listed, so
//! they show up as one line each rather than as a flood.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;

use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_error::BuckErrorContext;
use buck2_hash::BuckMutSet;

/// List project directories the running daemon has no inotify watch for.
#[derive(Debug, clap::Parser)]
pub struct WatchesCommand {
    /// List every unwatched directory, not just the top of each unwatched subtree.
    #[clap(long)]
    all: bool,
}

/// The inodes the daemon holds inotify watches on, from every inotify fd it has open.
fn watched_inodes(pid: u64) -> buck2_error::Result<BuckMutSet<u64>> {
    let mut inodes = BuckMutSet::default();
    let fdinfo = PathBuf::from(format!("/proc/{pid}/fdinfo"));
    for entry in fs::read_dir(&fdinfo)
        .with_buck_error_context(|| format!("Reading `{}`", fdinfo.display()))?
    {
        let Ok(contents) = fs::read_to_string(entry?.path()) else {
            continue; // fds come and go while we read them
        };
        for line in contents.lines() {
            if let Some(rest) = line.strip_prefix("inotify wd:") {
                for field in rest.split_whitespace() {
                    if let Some(hex) = field.strip_prefix("ino:")
                        && let Ok(ino) = u64::from_str_radix(hex, 16)
                    {
                        inodes.insert(ino);
                    }
                }
            }
        }
    }
    Ok(inodes)
}

/// Walk the project, collecting directories with no watch. Descends into an unwatched directory
/// only when asked, so an ignored subtree costs one line.
fn unwatched(root: &Path, watched: &BuckMutSet<u64>, all: bool) -> Vec<PathBuf> {
    let mut holes = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            if path
                .file_name()
                .is_some_and(|name| name == "buck-out" || name == ".git")
            {
                continue;
            }
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_symlink() {
                continue;
            }
            if watched.contains(&metadata.ino()) {
                pending.push(path);
            } else {
                holes.push(path.clone());
                if all {
                    pending.push(path);
                }
            }
        }
    }
    holes.sort();
    holes
}

impl WatchesCommand {
    pub fn exec(self, _matches: BuckArgMatches<'_>, ctx: ClientCommandContext<'_>) -> ExitResult {
        let paths = ctx.paths()?;
        let info = paths.daemon_dir()?.buckd_info();
        let daemon: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&info)
                .with_buck_error_context(|| format!("Reading `{}`", info.display()))?,
        )?;
        let Some(pid) = daemon["pid"].as_u64() else {
            return ExitResult::bail("No pid in buckd.info; is a daemon running?");
        };

        let root = paths.project_root().root();
        let watched = watched_inodes(pid)?;
        let holes = unwatched(root.as_path(), &watched, self.all);

        buck2_client_ctx::println!("daemon {} holds {} watches", pid, watched.len())?;
        for hole in &holes {
            buck2_client_ctx::println!(
                "unwatched: {}",
                hole.strip_prefix(root.as_path()).unwrap_or(hole).display()
            )?;
        }
        buck2_client_ctx::println!(
            "{} unwatched {} (ignored directories are expected here)",
            holes.len(),
            if self.all { "directories" } else { "subtrees" },
        )?;
        ExitResult::success()
    }
}
