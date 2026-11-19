/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_client_ctx::command_outcome::CommandOutcome;
use buck2_common::invocation_roots::InvocationRoots;
use buck2_common::legacy_configs::cells::BuckConfigBasedCells;
use buck2_fs::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::working_dir::AbsWorkingDir;

use crate::complete::path_sanitizer::PathSanitizer;
use crate::complete::path_sanitizer::SanitizedPath;

/// Companion files that live alongside flagfiles under `mode/` directories but
/// are not themselves usable as `@flagfile` / `--flagfile` arguments: buckconfig
/// includes pulled in via `--config-file`, Starlark (`.bzl` and `PACKAGE`),
/// generators, docs, and metadata.
const NON_FLAGFILE_EXTENSIONS: &[&str] = &[
    "buckconfig",
    "inc",
    "bcfg",
    "py",
    "pyc",
    "md",
    "owner",
    "json",
    "bzl",
];
const NON_FLAGFILE_NAMES: &[&str] = &["BUCK", "TARGETS", "PACKAGE"];

/// Completes the argument to a buck2 flagfile, i.e. an `@cell//path/to/mode/file`
/// or `--flagfile cell//path/to/mode/file` argument.
///
/// Unlike target completion this never needs the daemon: flagfiles are plain
/// files on disk, so completion is a single-level directory listing rooted at
/// the cell the partial resolves to. Directories are offered with a trailing `/`
/// (so completion continues into them); files are offered only when they look
/// like a flagfile rather than a companion (see `is_flagfile`).
pub(crate) struct FlagfileCompleter {
    path_sanitizer: PathSanitizer,
}

impl FlagfileCompleter {
    pub(crate) async fn new(cwd: &AbsWorkingDir, roots: &InvocationRoots) -> CommandOutcome<Self> {
        let cell_configs =
            Arc::new(BuckConfigBasedCells::parse_with_config_args(&roots.project_root, &[]).await?);
        let path_sanitizer = PathSanitizer::new(&cell_configs, cwd, roots).await?;
        CommandOutcome::Success(Self { path_sanitizer })
    }

    pub(crate) async fn complete(&self, given: &str) -> CommandOutcome<Vec<String>> {
        // A flagfile argument may be prefixed with `@` (the `@flagfile` form) or
        // bare (the `--flagfile <path>` form). Preserve whichever the user typed.
        let (at, body) = match given.strip_prefix('@') {
            Some(rest) => ("@", rest),
            None => ("", given),
        };

        let sanitized = self.path_sanitizer.sanitize(body)?;

        let mut results = Vec::new();
        if sanitized.is_ready_for_next_dir() {
            self.list_children(&sanitized, at, &mut results)?;
        } else {
            self.list_fragment_matches(&sanitized, at, &mut results)?;
        }

        results.sort();
        results.dedup();
        CommandOutcome::Success(results)
    }

    /// The partial names a directory (empty or slash-terminated): list its
    /// immediate children.
    fn list_children(
        &self,
        partial: &SanitizedPath,
        at: &str,
        results: &mut Vec<String>,
    ) -> CommandOutcome<()> {
        let given_dir = partial.given();
        for entry in fs_util::read_dir(partial.abs_path()).categorize_input()? {
            let entry = entry?;
            let name = file_name_string(&entry);
            self.push_candidate(
                &format!("{given_dir}{name}"),
                &name,
                entry.path().is_dir(),
                at,
                results,
            )?;
        }
        CommandOutcome::Success(())
    }

    /// The partial names an incomplete final component: list the entries of its
    /// parent directory whose names extend that component.
    fn list_fragment_matches(
        &self,
        partial: &SanitizedPath,
        at: &str,
        results: &mut Vec<String>,
    ) -> CommandOutcome<()> {
        let abs_path = partial.abs_path();
        let Some(base) = abs_path.file_name().and_then(|n| n.to_str()) else {
            return CommandOutcome::Success(());
        };
        let Some(scan_dir) = abs_path.parent() else {
            return CommandOutcome::Success(());
        };
        // Derive the completion prefix from what the user actually typed, by
        // splitting on the last `/`, rather than subtracting the resolved
        // base-name length: the sanitizer can normalize the trailing component,
        // and byte subtraction would then slice at the wrong offset (or panic on
        // an underflow / mid-UTF-8-character boundary).
        let given = partial.given();
        let given_dir = match given.rfind('/') {
            Some(slash) => &given[..=slash],
            None => "",
        };

        for entry in fs_util::read_dir(scan_dir).categorize_input()? {
            let entry = entry?;
            let name = file_name_string(&entry);
            if name.starts_with(base) {
                self.push_candidate(
                    &format!("{given_dir}{name}"),
                    &name,
                    entry.path().is_dir(),
                    at,
                    results,
                )?;
            }
        }
        CommandOutcome::Success(())
    }

    fn push_candidate(
        &self,
        candidate: &str,
        name: &str,
        is_dir: bool,
        at: &str,
        results: &mut Vec<String>,
    ) -> buck2_error::Result<()> {
        if is_dir {
            let normalized = self.path_sanitizer.sanitize(candidate)?;
            results.push(format!("{at}{}/", normalized.given()));
        } else if is_flagfile(name) {
            let normalized = self.path_sanitizer.sanitize(candidate)?;
            results.push(format!("{at}{}", normalized.given()));
        }
        Ok(())
    }
}

fn is_flagfile(name: &str) -> bool {
    if NON_FLAGFILE_NAMES.contains(&name) {
        return false;
    }
    match name.rsplit_once('.') {
        Some((_, extension)) => !NON_FLAGFILE_EXTENSIONS.contains(&extension),
        None => true,
    }
}

fn file_name_string(entry: &std::fs::DirEntry) -> String {
    // Non-UTF-8 directory entries are legal (esp. on Linux); degrade lossily
    // instead of panicking the completion process on an unusual filename.
    entry.file_name().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use buck2_client_ctx::exit_result::ExitResult;
    use buck2_common::invocation_roots::find_invocation_roots;
    use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;

    use super::*;

    fn paths_to_test_data() -> &'static [&'static str] {
        &[
            "fbcode/buck2/app/buck2_cmd_completion_client/test_data",
            "app/buck2_cmd_completion_client/test_data",
            "test_data",
        ]
    }

    fn in_root() -> CommandOutcome<(InvocationRoots, AbsWorkingDir)> {
        let cwd = AbsNormPathBuf::new(std::env::current_dir().unwrap())?;
        for path in paths_to_test_data() {
            let candidate = cwd.join_normalized(path)?;
            if candidate.exists() {
                let candidate = AbsWorkingDir::unchecked_new(candidate);
                return CommandOutcome::Success((find_invocation_roots(&candidate)?, candidate));
            }
        }
        CommandOutcome::Failure(ExitResult::bail("test_data directory not found"))
    }

    type TestResult = Result<(), ExitResult>;

    #[test]
    fn test_is_flagfile_accepts_extensionless_files() {
        assert!(
            is_flagfile("development"),
            "extensionless mode files are flagfiles"
        );
        assert!(is_flagfile("dev-asan"), "dashes are not extensions");
    }

    #[test]
    fn test_is_flagfile_rejects_companions_and_build_files() {
        assert!(!is_flagfile("BUCK"));
        assert!(!is_flagfile("TARGETS"));
        assert!(!is_flagfile("PACKAGE"));
        assert!(!is_flagfile("modes.bzl"));
        assert!(!is_flagfile("dev.buckconfig"));
        assert!(!is_flagfile("common.buckconfig.inc"));
        assert!(!is_flagfile("gen-modes.py"));
        assert!(!is_flagfile("migration.md"));
        assert!(!is_flagfile(".owner"));
    }

    #[tokio::test]
    async fn test_completes_directory_fragment_including_subdirs() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = FlagfileCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("baredir0/bare").await?;

        // Directories under the fragment are offered with a trailing slash.
        assert!(
            actual.iter().any(|s| s == "baredir0/baredir0a/"),
            "expected baredir0/baredir0a/ in {actual:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_at_prefix_is_preserved() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = FlagfileCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("@baredir0/bare").await?;

        assert!(!actual.is_empty(), "expected at least one completion");
        assert!(
            actual.iter().all(|s| s.starts_with('@')),
            "every completion should keep the @ prefix: {actual:?}"
        );
        Ok(())
    }
}
