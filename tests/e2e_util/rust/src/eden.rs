/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The EdenFS daemon of a test set up with `setup_eden`, which a test can mount more checkouts of
//! its project's repository on.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;

use crate::buck::Buck;

/// An EdenFS daemon of a test's own, with the empty repository its checkouts are of.
#[derive(Clone)]
pub struct Eden {
    dir: PathBuf,
    env: BTreeMap<OsString, OsString>,
}

impl Eden {
    pub(crate) fn new(dir: PathBuf, env: BTreeMap<OsString, OsString>) -> Self {
        Self { dir, env }
    }

    /// The daemon serving the project of `buck`, a test set up with `setup_eden`.
    pub fn of(buck: &Buck) -> anyhow::Result<Self> {
        let dir = buck
            .cwd()
            .parent()
            .context("the project has a parent")?
            .join("eden");
        anyhow::ensure!(
            dir.is_dir(),
            "no EdenFS daemon at {}: the test is not set up with `setup_eden`",
            dir.display()
        );
        Ok(Self::new(
            dir,
            buck.envs()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
        ))
    }

    /// Starts the daemon in `dir` and mounts the first checkout at `project_dir`.
    pub(crate) fn start(&self, project_dir: &Path) -> anyhow::Result<()> {
        for sub in ["config", "etc", "home"] {
            fs::create_dir_all(self.dir.join(sub))?;
        }
        run(&mut self.eden(["start"]))?;

        let repo = self.repo();
        run(self.command("hg").arg("init").arg(&repo))?;
        // The redirection is set up when the checkout is mounted. The number of APFS volumes macOS
        // can have is limited, and cleaning disk image redirections up is not trivial, so it is a
        // symlink there.
        let redirection = if cfg!(target_os = "macos") {
            "symlink"
        } else {
            "bind"
        };
        fs::write(
            repo.join(".eden-redirections"),
            format!("[redirections]\n\"buck-out\" = \"{redirection}\"\n"),
        )?;
        // Keeps the repository clean when EdenFS creates the redirection.
        fs::write(repo.join(".gitignore"), "/buck-out\n")?;
        run(self
            .command("hg")
            .args(["commit", "--addremove", "-m", "init"])
            .current_dir(&repo))?;
        self.clone_checkout(project_dir)
    }

    /// Mounts another checkout of the repository at `path`, which must not exist yet.
    pub fn clone_checkout(&self, path: &Path) -> anyhow::Result<()> {
        let mut clone = self.eden(["clone"]);
        clone
            .arg(self.repo())
            .arg(path)
            .args(["--allow-empty-repo", "--case-insensitive"]);
        if cfg!(windows) {
            clone.arg("--enable-windows-symlinks");
        }
        run(&mut clone)
    }

    /// Unmounts the checkout at `path` and deletes it.
    pub fn remove_checkout(&self, path: &Path) -> anyhow::Result<()> {
        let mut remove = self.eden(["remove"]);
        remove.arg(path).arg("-y");
        run(&mut remove)
    }

    /// Unmounts the checkout at `project_dir` and stops the daemon, as well as they go.
    pub(crate) fn shutdown(&self, project_dir: &Path) {
        let _ignored = self.remove_checkout(project_dir);
        let _ignored = self.eden(["shutdown"]).status();
    }

    fn repo(&self) -> PathBuf {
        self.dir.join("temp_repo")
    }

    fn command(&self, program: &str) -> std::process::Command {
        let mut command = std::process::Command::new(program);
        command
            .env_clear()
            .envs(&self.env)
            .stdout(std::io::stderr())
            .stderr(std::io::stderr());
        command
    }

    fn eden<'a>(&self, args: impl IntoIterator<Item = &'a str>) -> std::process::Command {
        let mut command = self.command("eden");
        command
            .arg("--config-dir")
            .arg(self.dir.join("config"))
            .arg("--home-dir")
            .arg(self.dir.join("home"))
            .arg("--etc-eden-dir")
            .arg(self.dir.join("etc"))
            .args(args);
        command
    }
}

pub(crate) fn run(command: &mut std::process::Command) -> anyhow::Result<()> {
    let status = command
        .status()
        .with_context(|| format!("Running {command:?}"))?;
    anyhow::ensure!(status.success(), "{command:?} failed: {status}");
    Ok(())
}
