/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::process::Stdio;

use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_util::process::async_background_command;
use tokio::sync::OnceCell;

enum RepoVcs {
    Hg,
    Git,
}

async fn get_vcs_revision(vcs: RepoVcs, repo_root: &AbsNormPath) -> Option<String> {
    let mut cmd = match vcs {
        RepoVcs::Hg => {
            let mut cmd = async_background_command("hg");
            cmd.arg("--cwd")
                .arg(repo_root.as_path())
                .args(["id", "-i", "--debug"]);
            cmd.env("HGPLAIN", "1");
            cmd
        }
        RepoVcs::Git => {
            let mut cmd = async_background_command("git");
            cmd.arg("-C").arg(repo_root.as_path()).args([
                "describe",
                "--always",
                "--dirty=+",
                "--abbrev=40",
            ]);
            cmd.env("GIT_OPTIONAL_LOCKS", "0");
            cmd
        }
    };
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(std::str::from_utf8(&output.stdout).ok()?.trim().to_owned())
}

async fn compute_revision(repo_root: &AbsNormPath) -> Option<String> {
    if let Some(revision) = get_vcs_revision(RepoVcs::Hg, repo_root).await {
        return Some(revision);
    }
    get_vcs_revision(RepoVcs::Git, repo_root).await
}

/// Lazily computed version-control revision for one repository root.
///
/// The first lookup starts an `hg` or `git` subprocess. Later lookups reuse the cached result.
pub struct LazyVcsRevision {
    repo_root: AbsNormPathBuf,
    value: OnceCell<Option<String>>,
}

impl LazyVcsRevision {
    /// Create an empty revision cache scoped to `repo_root`.
    pub fn new(repo_root: AbsNormPathBuf) -> Self {
        Self {
            repo_root,
            value: OnceCell::new(),
        }
    }

    /// Return the repository revision, computing it at most once for this instance.
    pub async fn get(&self) -> Option<&str> {
        self.value
            .get_or_init(|| compute_revision(&self.repo_root))
            .await
            .as_deref()
    }
}

// Runs `git` in temporary repositories, which Miri cannot.
#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    async fn git(repo_root: &AbsNormPath, args: &[&str]) -> String {
        let output = async_background_command("git")
            .arg("-C")
            .arg(repo_root.as_path())
            .args(args)
            .output()
            .await
            .expect("running git in the test repository should succeed");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("git stdout should be UTF-8")
    }

    async fn init_git_repo(contents: &str) -> (tempfile::TempDir, AbsNormPathBuf) {
        let temp_dir = tempfile::tempdir().expect("creating a temporary directory should succeed");
        let repo_root = AbsNormPathBuf::try_from(temp_dir.path().to_path_buf())
            .expect("the temporary directory should be absolute");
        // An hg repository no hg can open: it stops hg from discovering an enclosing
        // checkout when TMPDIR is inside one, and makes the lookup fall through to git.
        // An empty `.hg` is not enough, Mercurial reads it as a valid repository at the
        // null revision.
        let hg_dir = repo_root.as_path().join(".hg");
        tokio::fs::create_dir(&hg_dir)
            .await
            .expect("creating the hg metadata directory should succeed");
        tokio::fs::write(
            hg_dir.join("requires"),
            "buck2-test-unopenable-repository\n",
        )
        .await
        .expect("writing the hg requirements should succeed");
        git(&repo_root, &["init", "-q"]).await;
        tokio::fs::write(repo_root.as_path().join("file.txt"), contents)
            .await
            .expect("writing the test file should succeed");
        git(&repo_root, &["add", "file.txt"]).await;
        git(
            &repo_root,
            &[
                "-c",
                "user.name=Buck Test",
                "-c",
                "user.email=buck-test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "initial",
            ],
        )
        .await;
        (temp_dir, repo_root)
    }

    #[tokio::test]
    async fn revisions_are_scoped_to_repo_root() {
        let (_first_dir, first_root) = init_git_repo("first").await;
        let (_second_dir, second_root) = init_git_repo("second").await;

        let first = LazyVcsRevision::new(first_root);
        let second = LazyVcsRevision::new(second_root);
        let first_revision = first.get().await;
        let second_revision = second.get().await;

        assert!(first_revision.is_some());
        assert!(second_revision.is_some());
        assert_ne!(first_revision, second_revision);
    }

    #[tokio::test]
    async fn revision_is_cached_per_instance() {
        let (_temp_dir, repo_root) = init_git_repo("initial").await;
        let revision = LazyVcsRevision::new(repo_root.clone());
        let clean_revision = revision.get().await.map(str::to_owned);

        tokio::fs::write(repo_root.as_path().join("file.txt"), "changed")
            .await
            .expect("updating the test file should succeed");

        assert_eq!(revision.get().await, clean_revision.as_deref());
        assert!(
            LazyVcsRevision::new(repo_root)
                .get()
                .await
                .is_some_and(|revision| revision.ends_with('+'))
        );
    }

    #[tokio::test]
    async fn revision_searches_parents_and_falls_back_to_git() {
        let (_temp_dir, repo_root) = init_git_repo("initial").await;
        let nested_root = repo_root.as_path().join("nested");
        tokio::fs::create_dir(&nested_root)
            .await
            .expect("creating a nested project root should succeed");
        let nested_root = AbsNormPathBuf::try_from(nested_root)
            .expect("the nested project root should be absolute");

        assert!(LazyVcsRevision::new(nested_root).get().await.is_some());
    }
}
