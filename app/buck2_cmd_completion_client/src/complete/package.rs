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
use buck2_fs::working_dir::AbsWorkingDir;

use super::path_completer::PathCompleter;
use super::path_sanitizer::PathSanitizer;
use super::results::CompletionResults;

pub(crate) struct PackageCompleter<'a> {
    cwd: AbsWorkingDir,
    roots: &'a InvocationRoots,
    cell_configs: Arc<BuckConfigBasedCells>,
    path_sanitizer: PathSanitizer,
    results: CompletionResults<'a>,
}

impl<'a> PackageCompleter<'a> {
    pub(crate) async fn new(
        cwd: &AbsWorkingDir,
        roots: &'a InvocationRoots,
    ) -> CommandOutcome<Self> {
        let cell_configs =
            Arc::new(BuckConfigBasedCells::parse_with_config_args(&roots.project_root, &[]).await?);

        let path_sanitizer = PathSanitizer::new(&cell_configs, cwd, roots).await?;
        let results = CompletionResults::new(roots, cell_configs.clone());
        CommandOutcome::Success(Self {
            cwd: cwd.to_owned(),
            roots,
            cell_configs,
            path_sanitizer,
            results,
        })
    }

    /// Complete the package portion of a partial target.
    ///
    /// Returns a collection of possible completions, each generally including the
    /// partial target. The partial target might not be returned as-is when
    /// completion logic is able to unambiguously normalize the partial target,
    /// such as partials which cross cell boundaries. In this case, normalized
    /// completion(s) are returned.
    pub(crate) async fn complete(mut self, given_path: &str) -> CommandOutcome<Vec<String>> {
        let cwd_cell_name = self
            .cell_configs
            .cell_resolver
            .get_cell_path(&self.roots.cwd)
            .cell();
        let cwd_cell_root = self.cell_configs.cell_resolver.get(cwd_cell_name)?.path();
        let cwd_cell_root = self.roots.project_root.resolve(cwd_cell_root);
        match given_path {
            "" => {
                self.results.insert_dir(&cwd_cell_root, "//").await;
                self.results
                    .insert_package_colon_if_buildfile_exists(self.cwd.path(), given_path)
                    .await;
                self.complete_partial_cells(given_path).await?;
                self.complete_partial_path(given_path).await?;
            }
            "/" => {
                self.results.insert_dir(&cwd_cell_root, "//").await;
            }
            "//" => {
                self.results
                    .insert_package_colon_if_buildfile_exists(&cwd_cell_root, given_path)
                    .await;
                self.complete_partial_path(given_path).await?;
            }
            _ => {
                self.complete_partial_cells(given_path).await?;
                self.complete_partial_path(given_path).await?;
            }
        }
        CommandOutcome::Success(self.results.into())
    }

    async fn complete_partial_cells(&mut self, given_path: &str) -> buck2_error::Result<()> {
        let cell_resolver = &self.cell_configs.cell_resolver;
        let alias_resolver = self
            .cell_configs
            .get_cell_alias_resolver_for_cwd_fast(&self.roots.project_root, &self.roots.cwd)
            .await?;
        for (cell_alias, cell_name) in alias_resolver.mappings() {
            let canonical_cell_root = format!("{cell_alias}//");
            if canonical_cell_root.starts_with(given_path) {
                let cell = cell_resolver.get(cell_name)?;
                let cell_abs_path = self
                    .roots
                    .project_root
                    .root()
                    .join(cell.path().as_project_relative_path());
                if canonical_cell_root == given_path {
                    // "cell1//" -> "cell1//:"
                    self.results
                        .insert_package_colon_if_buildfile_exists(
                            &cell_abs_path,
                            &canonical_cell_root,
                        )
                        .await;
                } else {
                    // "cell1" -> ["cell1//", "cell1//:"]
                    // "cell"  -> ["cell1//", "cell1//:", "cell2//"]
                    self.results
                        .insert_dir(&cell_abs_path, &canonical_cell_root)
                        .await;
                }
            }
        }
        Ok(())
    }

    async fn complete_partial_path(&mut self, given_path: &str) -> CommandOutcome<()> {
        let completer = PathCompleter::new(
            &self.cwd,
            self.roots,
            &self.path_sanitizer,
            &mut self.results,
        )?;
        completer.complete(given_path).await
    }
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

    fn in_dir(d: &str) -> CommandOutcome<(InvocationRoots, AbsWorkingDir)> {
        let cwd = AbsNormPathBuf::new(std::env::current_dir().unwrap())?;

        for path in paths_to_test_data() {
            let candidate = cwd.join_normalized(path)?.join_normalized(d)?;
            if candidate.exists() {
                let candidate = AbsWorkingDir::unchecked_new(candidate);
                return CommandOutcome::Success((find_invocation_roots(&candidate)?, candidate));
            }
        }

        CommandOutcome::Failure(ExitResult::bail("test_data directory not found"))
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

    fn is_err<T>(outcome: CommandOutcome<T>) -> bool {
        match outcome {
            CommandOutcome::Success(_) => false,
            CommandOutcome::Failure(_) => true,
        }
    }

    type TestResult = Result<(), ExitResult>;

    #[tokio::test]
    async fn test_expands_top_level_directory() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("bare").await?;

        assert_eq!(actual, vec!["baredir0/"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_expands_subdirectory() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("baredir0/bare").await?;

        assert_eq!(actual, vec!["baredir0/baredir0a/"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_expands_subdirectory_with_buck_targets() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("baredir0/buck").await?;

        assert_eq!(actual, vec!["baredir0/buckdir0b/", "baredir0/buckdir0b:",]);
        Ok(())
    }

    #[tokio::test]
    async fn test_provides_subdirectory_and_target_alternatives() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("baredir0/buckdir0b").await?;

        assert_eq!(actual, vec!["baredir0/buckdir0b/", "baredir0/buckdir0b:",]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_partial_paths_and_matched_target_dirs() -> TestResult {
        let (roots, cwd) = in_dir("baredir0")?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("b").await?;

        assert_eq!(actual, vec!["baredir0a/", "buckdir0b/", "buckdir0b:",]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_cell_name_when_given_cell_root() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cel").await?;

        assert_eq!(
            actual,
            vec![
                "cell1//",
                "cell1//:",
                "cell2//",
                "cell3a//",
                "cell3a//:",
                "cell3b//"
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_cell_name_when_given_cell_root_as_directory() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell1/").await?;

        assert_eq!(actual, vec!["cell1//", "cell1//:"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_identifies_non_local_cell_name() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell3a").await?;

        assert_eq!(actual, vec!["cell3a//", "cell3a//:"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_non_local_cell_name_when_provided_with_a_single_slash() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell3b/").await?;

        assert_eq!(actual, vec!["cell3b//"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_non_local_cell_name() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell3").await?;

        assert_eq!(actual, vec!["cell3a//", "cell3a//:", "cell3b//"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_mix_of_non_local_cell_name_and_local_dirs() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell").await?;

        assert_eq!(
            actual,
            vec![
                "cell1//",
                "cell1//:",
                "cell2//",
                "cell3a//",
                "cell3a//:",
                "cell3b//"
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_multiple_partial_dirs() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("baredir0/b").await?;

        assert_eq!(
            actual,
            vec![
                "baredir0/baredir0a/",
                "baredir0/buckdir0b/",
                "baredir0/buckdir0b:"
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_as_both_directory_and_target_root() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("baredir0/buckdir0b").await?;

        assert_eq!(actual, vec!["baredir0/buckdir0b/", "baredir0/buckdir0b:",]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_subdirs_as_well_as_target_colon_for_fully_qualified_cell() -> TestResult
    {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell1//").await?;

        assert_eq!(actual, vec!["cell1//:", "cell1//buck2/", "cell1//buck2:"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_cell_correctly_from_a_different_cell() -> TestResult {
        let (roots, cwd) = in_dir("cell2")?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell1//").await?;

        assert_eq!(actual, vec!["cell1//:", "cell1//buck2/", "cell1//buck2:"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_directory_in_different_cell_to_canonical_name() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell1/buck2").await?;

        assert_eq!(actual, vec!["cell1//buck2"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_partial_dir_in_different_cell_to_canonical_name() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell1/b").await?;

        assert_eq!(actual, vec!["cell1//b"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_canonical_path_in_other_cell_to_with_slash_or_colon() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell1//buck2").await?;

        assert_eq!(actual, vec!["cell1//buck2/", "cell1//buck2:"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_canonical_path_to_other_cell_from_subdirectory_in_this_cell()
    -> TestResult {
        let (roots, cwd) = in_dir("baredir0")?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell1//buck2").await?;

        assert_eq!(actual, vec!["cell1//buck2/", "cell1//buck2:"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_in_subdirectory_can_complete_subdirs_of_project_root_with_canonical_cell()
    -> TestResult {
        let (roots, cwd) = in_dir("baredir0")?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("root//").await?;

        assert_eq!(actual, vec!["root//baredir0/", "root//dir3/",]);
        Ok(())
    }

    #[tokio::test]
    async fn test_backwards_path_to_different_cell_root_directory_completes_to_canonical_cell_name()
    -> TestResult {
        let (roots, cwd) = in_dir("baredir0")?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("../cell1").await?;

        assert_eq!(actual, vec!["cell1//"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_partial_subdirectory_name_expands_to_all_matches() -> TestResult {
        let (roots, cwd) = in_dir("baredir0")?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("b").await?;

        assert_eq!(actual, vec!["baredir0a/", "buckdir0b/", "buckdir0b:",]);
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_string_completes_both_cells_and_dirs() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("").await?;

        assert_eq!(
            actual,
            vec![
                "//",
                "baredir0/",
                "cell1//",
                "cell1//:",
                "cell2//",
                "cell3a//",
                "cell3a//:",
                "cell3b//",
                "dir3/",
                "prelude//",
                "root//"
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_string_also_completes_colon_if_in_target_dir() -> TestResult {
        let (roots, cwd) = in_dir("cell1")?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("").await?;

        assert_eq!(
            actual,
            vec![
                "//",
                "//:",
                ":",
                "buck2/",
                "buck2:",
                "cell1//",
                "cell1//:",
                "cell2//",
                "cell3a//",
                "cell3a//:",
                "cell3b//",
                "prelude//",
                "root//"
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_single_slash_to_double_slash_in_root() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("/").await?;

        assert_eq!(actual, vec!["//"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_root_children_of_double_slash() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("//").await?;

        assert_eq!(actual, vec!["//baredir0/", "//dir3/"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_single_slash_to_double_slash_in_sub_cell() -> TestResult {
        let (roots, cwd) = in_dir("cell1")?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("/").await?;

        assert_eq!(actual, vec!["//", "//:"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_cell_children_of_double_slash() -> TestResult {
        let (roots, cwd) = in_dir("cell1")?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("//").await?;

        assert_eq!(actual, vec!["//:", "//buck2/", "//buck2:"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_normalizes_absolute_paths() -> TestResult {
        let (roots, cwd) = in_root()?;
        let absolute_partial = cwd.path().as_path().join("bare");
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete(absolute_partial.to_str().unwrap()).await?;

        assert_eq!(actual, vec!["root//bare"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_bails_on_nonexistent_path() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("nonexistent").await?;

        assert_eq!(actual.len(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_bails_on_nonexistent_path_in_subcell() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell1//nonexistent").await?;

        assert_eq!(actual.len(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_package_completion_completes_packages_with_cell_aliases_as_aliases() -> TestResult
    {
        let (roots, cwd) = in_dir("cell1/buck2/prelude")?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell1_alias//b").await?;

        assert_eq!(actual, vec!["cell1_alias//buck2/", "cell1_alias//buck2:"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_package_completion_only_uses_aliases_in_cells_definining_them() -> TestResult {
        let (roots, cwd) = in_dir("cell1/buck2")?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual_result = uut.complete("cell1_alias//b").await;

        assert!(is_err(actual_result));
        Ok(())
    }

    #[tokio::test]
    async fn test_completes_cell_aliases_alongside_cells() -> TestResult {
        let (roots, cwd) = in_dir("cell1/buck2/prelude")?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("cell1").await?;

        assert_eq!(
            actual,
            vec!["cell1//", "cell1//:", "cell1_alias//", "cell1_alias//:"]
        );
        Ok(())
    }

    /// With the cwd in a package below a cell root that is not a package, `""` offers `:` for
    /// the cwd and no `//:`.
    #[tokio::test]
    async fn test_empty_string_in_package_subdir_completes_cwd_colon() -> TestResult {
        let (roots, cwd) = in_dir("baredir0/buckdir0b")?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("").await?;

        assert_eq!(
            actual,
            vec![
                "//",
                ":",
                "cell1//",
                "cell1//:",
                "cell2//",
                "cell3a//",
                "cell3a//:",
                "cell3b//",
                "prelude//",
                "root//"
            ]
        );
        Ok(())
    }

    /// A partial whose last part is `.` or `..` has nothing to complete and must not panic.
    #[tokio::test]
    async fn test_dot_partial_completes_nothing() -> TestResult {
        for given in ["//.", "root//.", "//baredir0/.."] {
            let (roots, cwd) = in_root()?;
            let uut = PackageCompleter::new(&cwd, &roots).await?;

            let actual = uut.complete(given).await?;

            assert!(actual.is_empty(), "given {given}: {actual:?}");
        }
        Ok(())
    }

    /// A `/` after a directory that does not exist, or a `.` component, completes to nothing
    /// rather than to directories that do not exist.
    #[tokio::test]
    async fn test_fragment_after_dot_or_missing_dir_completes_nothing() -> TestResult {
        for given in ["//baredir0/.", "cell1//bu/", "//baredir0/bare/"] {
            let (roots, cwd) = in_root()?;
            let uut = PackageCompleter::new(&cwd, &roots).await?;

            let actual = uut.complete(given).await?;

            assert!(actual.is_empty(), "given {given}: {actual:?}");
        }
        Ok(())
    }

    /// `baredir0/` names an existing directory of the cwd cell, so its children are offered.
    #[tokio::test]
    async fn test_trailing_slash_on_existing_dir_lists_its_children() -> TestResult {
        let (roots, cwd) = in_root()?;
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("baredir0/").await?;

        assert_eq!(
            actual,
            vec![
                "baredir0/baredir0a/",
                "baredir0/buckdir0b/",
                "baredir0/buckdir0b:"
            ]
        );
        Ok(())
    }

    /// A throwaway project with a `buck-out` next to a real directory.
    fn project_with_buck_out() -> (std::path::PathBuf, InvocationRoots, AbsWorkingDir) {
        let dir = std::env::temp_dir().join(format!("completion_buck_out_{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(dir.join("buck-out/v2")).unwrap();
        std::fs::create_dir_all(dir.join("bar")).unwrap();
        std::fs::write(dir.join(".buckroot"), "").unwrap();
        std::fs::write(dir.join(".buckconfig"), "[cells]\n  root = .\n").unwrap();
        let cwd = AbsWorkingDir::unchecked_new(AbsNormPathBuf::new(dir.clone()).unwrap());
        let roots = find_invocation_roots(&cwd).unwrap();
        (dir, roots, cwd)
    }

    /// `buck-out` holds no packages and is never offered.
    #[tokio::test]
    async fn test_skips_buck_out() -> TestResult {
        let (dir, roots, cwd) = project_with_buck_out();
        let uut = PackageCompleter::new(&cwd, &roots).await?;

        let actual = uut.complete("b").await?;

        drop(std::fs::remove_dir_all(&dir));
        assert_eq!(actual, vec!["bar/"]);
        Ok(())
    }
}
