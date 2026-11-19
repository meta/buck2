/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::path::Path;

use buck2_common::invocation_roots::InvocationRoots;
use buck2_common::legacy_configs::cells::BuckConfigBasedCells;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::name::CellName;
use buck2_core::cells::paths::CellRelativePath;
use buck2_core::cells::paths::CellRelativePathBuf;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_error::buck2_error;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::working_dir::AbsWorkingDir;

#[derive(Debug, Clone)]
pub(crate) struct SanitizedPath {
    given: String,
    abs_path: AbsNormPathBuf,
    cell_name: CellName,
}

impl SanitizedPath {
    pub(crate) fn abs_path(&self) -> &AbsNormPath {
        &self.abs_path
    }

    pub(crate) fn cell_name(&self) -> &CellName {
        &self.cell_name
    }

    pub(crate) fn given(&self) -> &str {
        &self.given
    }

    pub(crate) fn is_ready_for_next_dir(&self) -> bool {
        let is_root_dir = self.given.is_empty();
        let is_slash_terminated_dir = fs_util::metadata(&self.abs_path).is_ok_and(|m| m.is_dir())
            && self.given.ends_with('/');
        is_root_dir || is_slash_terminated_dir
    }
}

pub(crate) struct PathSanitizer {
    cell_resolver: CellResolver,
    alias_resolver: CellAliasResolver,
    cwd: AbsWorkingDir,
    cwd_roots: InvocationRoots,
}

impl PathSanitizer {
    pub(crate) async fn new(
        cell_configs: &BuckConfigBasedCells,
        cwd: &AbsWorkingDir,
        cwd_roots: &InvocationRoots,
    ) -> buck2_error::Result<Self> {
        let cell_resolver = cell_configs.cell_resolver.clone();
        let alias_resolver = cell_configs
            .get_cell_alias_resolver_for_cwd_fast(
                &cwd_roots.project_root,
                &cwd_roots.project_root.relativize(cwd.path())?,
            )
            .await?;
        Ok(Self {
            cell_resolver,
            alias_resolver,
            cwd: cwd.to_owned(),
            cwd_roots: cwd_roots.clone(),
        })
    }

    pub(crate) fn sanitize(&self, given: &str) -> buck2_error::Result<SanitizedPath> {
        match given.split("//").collect::<Vec<_>>()[..] {
            [path] => self.sanitize_relative_path(given, path),
            [cell, path] => self.sanitize_cell_based_path(given, cell, path),
            _ => Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Poorly formatted BuckPath string"
            )),
        }
    }

    fn sanitize_relative_path(
        &self,
        given: &str,
        path_str: &str,
    ) -> Result<SanitizedPath, buck2_error::Error> {
        let path = Path::new(path_str);
        let abs_path = if path.is_absolute() {
            AbsNormPathBuf::new(path.to_owned())?
        } else {
            self.cwd.path().join_normalized(path_str)?
        };

        let cwd_cell_name = self.cell_resolver.find(&self.cwd_roots.cwd);

        let cell_name = self.resolve_cell(&abs_path)?;
        let cell_path = self.relative_to_cell(&abs_path)?;

        if cell_name != cwd_cell_name || !self.is_normalized_path_and_in_cell(given, &cell_path) {
            let fixed_given = if given.ends_with('/') && cell_path.as_str() != "" {
                format!("{cell_name}//{cell_path}/")
            } else {
                format!("{cell_name}//{cell_path}")
            };
            Ok(SanitizedPath {
                given: fixed_given,
                abs_path,
                cell_name,
            })
        } else {
            Ok(SanitizedPath {
                given: given.to_owned(),
                abs_path,
                cell_name,
            })
        }
    }

    fn sanitize_cell_based_path(
        &self,
        given: &str,
        given_cell_str: &str,
        cell_path: &str,
    ) -> Result<SanitizedPath, buck2_error::Error> {
        let given_cell = if given_cell_str.is_empty() {
            self.cell_resolver.find(&self.cwd_roots.cwd)
        } else {
            self.resolve_alias(given_cell_str)?
        };
        let abs_path = self.cell_abs_path(given_cell)?.join_normalized(cell_path)?;
        let actual_cell = self.resolve_cell(&abs_path)?;
        if given_cell == actual_cell {
            Ok(SanitizedPath {
                given: given.to_owned(),
                abs_path,
                cell_name: given_cell,
            })
        } else {
            // This is a bit ugly because it breaks expectations --
            // the "given" path is malformed, so it is corrected to
            // the closest equivalent path that actually works.
            let corrected_cell_root = self.cell_abs_path(actual_cell)?;
            let corrected_cell_path = abs_path.strip_prefix(corrected_cell_root)?;
            Ok(SanitizedPath {
                given: format!("{actual_cell}//{corrected_cell_path}"),
                abs_path,
                cell_name: actual_cell,
            })
        }
    }

    fn cell_abs_path(&self, cell: CellName) -> buck2_error::Result<AbsNormPathBuf> {
        let root_to_cell = self
            .cell_resolver
            .get(cell)?
            .path()
            .as_forward_relative_path();
        self.project_root_dir().join_normalized(root_to_cell)
    }

    /// Checks whether a given path str is properly a member of the given cell
    ///
    /// Both `given_fragment` and `proper_cell_path` refer to the same path,
    /// but `given_fragment` is user input and may be malformed in ways that
    /// buck will not tolerate. (absolute, cross-cell, absolute, ../, etc)
    ///
    /// This function returns true if `given_fragment` is acceptable to buck
    /// as a relative reference to a path in the cell that `proper_cell_path`
    /// is based on.
    fn is_normalized_path_and_in_cell(
        &self,
        given_fragment: &str,
        proper_cell_path: &CellRelativePath,
    ) -> bool {
        let path = Path::new(given_fragment);
        match ForwardRelativePath::new_trim_trailing_slashes(path) {
            Ok(forward_rel_path) => proper_cell_path.ends_with(forward_rel_path),
            Err(_) => false,
        }
    }

    fn project_root(&self) -> &ProjectRoot {
        &self.cwd_roots.project_root
    }

    fn project_root_dir(&self) -> &AbsNormPath {
        self.cwd_roots.project_root.root()
    }

    fn resolve_alias(&self, dir: &str) -> buck2_error::Result<CellName> {
        self.alias_resolver.resolve(dir)
    }

    fn resolve_cell(&self, path: &AbsNormPath) -> buck2_error::Result<CellName> {
        let project_relative = &self.relative_to_project(path)?;
        Ok(self
            .cell_resolver
            .find::<ProjectRelativePath>(project_relative))
    }

    fn relative_to_cell(&self, dir: &AbsNormPath) -> buck2_error::Result<CellRelativePathBuf> {
        Ok(self
            .cell_resolver
            .get_cell_path_from_abs_path(dir, self.project_root())?
            .path()
            .to_owned())
    }

    fn relative_to_project<'a>(
        &'a self,
        dir: &'a AbsNormPath,
    ) -> buck2_error::Result<Cow<'a, ProjectRelativePath>> {
        self.project_root().relativize(dir)
    }
}

#[cfg(test)]
mod tests {
    use buck2_common::invocation_roots::find_invocation_roots;
    use buck2_common::legacy_configs::cells::BuckConfigBasedCells;
    use paste::paste;

    use super::*;

    fn paths_to_test_data() -> &'static [&'static str] {
        &[
            "fbcode/buck2/app/buck2_cmd_completion_client/test_data",
            "app/buck2_cmd_completion_client/test_data",
            "test_data",
        ]
    }

    fn in_dir(d: &str) -> buck2_error::Result<AbsWorkingDir> {
        let cwd = AbsNormPathBuf::new(std::env::current_dir().unwrap())?;

        for path in paths_to_test_data() {
            let candidate = cwd.join_normalized(path)?.join_normalized(d)?;
            if candidate.exists() {
                return Ok(AbsWorkingDir::unchecked_new(candidate));
            }
        }

        Err(buck2_error!(
            buck2_error::ErrorTag::Tier0,
            "test_data directory not found"
        ))
    }

    fn in_root() -> buck2_error::Result<AbsWorkingDir> {
        let cwd = AbsNormPathBuf::new(std::env::current_dir().unwrap())?;

        for path in paths_to_test_data() {
            let candidate = cwd.join_normalized(path)?;
            if candidate.exists() {
                return Ok(AbsWorkingDir::unchecked_new(candidate));
            }
        }

        Err(buck2_error!(
            buck2_error::ErrorTag::Tier0,
            "test_data directory not found"
        ))
    }

    fn abs_path_from_root(relative: &str) -> buck2_error::Result<AbsNormPathBuf> {
        let root = in_root()?;
        root.into_abs_norm_path_buf().join_normalized(relative)
    }

    fn abs_str_from_root(relative: &str) -> buck2_error::Result<String> {
        let path = abs_path_from_root(relative)?;
        Ok(path.to_string())
    }

    fn cell_configs(cwd: &AbsWorkingDir) -> buck2_error::Result<BuckConfigBasedCells> {
        let cwd_roots = find_invocation_roots(cwd)?;
        futures::executor::block_on(BuckConfigBasedCells::parse_with_config_args(
            &cwd_roots.project_root,
            &[],
        ))
    }

    macro_rules! testy {
        ($test_name:ident($in_dir:expr, $partial:expr) -> {
            abs_path: from_root($abs_dir:literal),
            canonical: $canonical:literal,
            cell_name: $cell_name:literal,
            cell_path: $cell_path:literal,
            given: $given:literal,
            to_string: $to_string:literal,
        }) => {
            paste! {
                #[tokio::test]
                async fn [<test_ $test_name _verify_abs_path>]() -> buck2_error::Result<()> {
                    let cwd = $in_dir;
                    let uut = PathSanitizer::new(&cell_configs(&cwd)?, &cwd, &find_invocation_roots(&cwd)?).await?;

                    let actual = uut.sanitize($partial)?;

                    assert_eq!(actual.abs_path(), abs_path_from_root($abs_dir)?);

                    Ok(())
                }

                #[tokio::test]
                async fn [<test_ $test_name _verify_cell_name>]() -> buck2_error::Result<()> {
                    let cwd = $in_dir;
                    let uut = PathSanitizer::new(&cell_configs(&cwd)?, &cwd, &find_invocation_roots(&cwd)?).await?;

                    let actual = uut.sanitize($partial)?;

                    assert_eq!(actual.cell_name(), &CellName::testing_new($cell_name));

                    Ok(())
                }

                #[tokio::test]
                async fn [<test_ $test_name _verify_given>]() -> buck2_error::Result<()> {
                    let cwd = $in_dir;
                    let uut = PathSanitizer::new(&cell_configs(&cwd)?, &cwd, &find_invocation_roots(&cwd)?).await?;

                    let actual = uut.sanitize($partial)?;

                    assert_eq!(actual.given(), $given);

                    Ok(())
                }
            }
        };
    }

    #[tokio::test]
    async fn test_can_create_from_a_canonical_path() -> buck2_error::Result<()> {
        let cwd = in_root()?;
        let uut =
            PathSanitizer::new(&cell_configs(&cwd)?, &cwd, &find_invocation_roots(&cwd)?).await?;

        uut.sanitize("root//baredir0/buckdir0a")?;

        Ok(())
    }

    #[tokio::test]
    async fn test_can_create_from_a_str_relative_path() -> buck2_error::Result<()> {
        let cwd = in_root()?;
        let uut =
            PathSanitizer::new(&cell_configs(&cwd)?, &cwd, &find_invocation_roots(&cwd)?).await?;

        uut.sanitize("baredir0/buckdir0a")?;

        Ok(())
    }

    testy!(canonical_path_in_root_from_root(in_root()?, "root//baredir0/buckdir0a") -> {
        abs_path: from_root("baredir0/buckdir0a"),
        canonical: "root//baredir0/buckdir0a",
        cell_name: "root",
        cell_path: "baredir0/buckdir0a",
        given: "root//baredir0/buckdir0a",
        to_string: "root//baredir0/buckdir0a",
    });

    testy!(anonymous_cell_from_root(in_root()?, "//") -> {
        abs_path: from_root(""),
        canonical: "//",
        cell_name: "root",
        cell_path: "",
        given: "//",
        to_string: "//",
    });

    testy!(canonical_cell_path_from_root(in_root()?, "cell1//buck2") -> {
        abs_path: from_root("cell1/buck2"),
        canonical: "cell1//buck2",
        cell_name: "cell1",
        cell_path: "buck2",
        given: "cell1//buck2",
        to_string: "cell1//buck2",
    });

    testy!(relative_path_from_root(in_root()?, "baredir0/buckdir0a") -> {
        abs_path: from_root("baredir0/buckdir0a"),
        canonical: "root//baredir0/buckdir0a",
        cell_name: "root",
        cell_path: "baredir0/buckdir0a",
        given: "baredir0/buckdir0a",
        to_string: "baredir0/buckdir0a",
    });

    testy!(cross_cell_forward_path_from_root(in_root()?, "cell1/buck2") -> {
        abs_path: from_root("cell1/buck2"),
        canonical: "cell1//buck2",
        cell_name: "cell1",
        cell_path: "buck2",
        given: "cell1//buck2", // BuckPath is documented as correcting this to cell1//buck2
        to_string: "cell1//buck2",
    });

    testy!(corrects_malformed_cross_cell_forward_path_from_root(in_root()?, "root//cell1/buck2") -> {
        abs_path: from_root("cell1/buck2"),
        canonical: "cell1//buck2",
        cell_name: "cell1",
        cell_path: "buck2",
        given: "cell1//buck2",
        to_string: "cell1//buck2",
    });

    #[tokio::test]
    async fn test_root_dir_as_empty_string_is_ready_for_subdirs() -> buck2_error::Result<()> {
        let cwd = in_root()?;
        let uut =
            PathSanitizer::new(&cell_configs(&cwd)?, &cwd, &find_invocation_roots(&cwd)?).await?;

        let actual = uut.sanitize("")?;

        assert!(actual.is_ready_for_next_dir());

        Ok(())
    }

    #[tokio::test]
    async fn test_slash_terminated_dir_is_ready_for_subdirs() -> buck2_error::Result<()> {
        let cwd = in_root()?;
        let uut =
            PathSanitizer::new(&cell_configs(&cwd)?, &cwd, &find_invocation_roots(&cwd)?).await?;

        let actual = uut.sanitize("baredir0/")?;

        assert!(actual.is_ready_for_next_dir());

        Ok(())
    }

    #[tokio::test]
    async fn test_partial_with_no_slash_is_not_ready_for_subdirs() -> buck2_error::Result<()> {
        let cwd = in_root()?;
        let uut =
            PathSanitizer::new(&cell_configs(&cwd)?, &cwd, &find_invocation_roots(&cwd)?).await?;

        let actual = uut.sanitize("baredir0")?;

        assert!(!actual.is_ready_for_next_dir());

        Ok(())
    }

    #[tokio::test]
    async fn test_fully_qualified_cell_is_ready_for_subdirs() -> buck2_error::Result<()> {
        let cwd = in_root()?;
        let uut =
            PathSanitizer::new(&cell_configs(&cwd)?, &cwd, &find_invocation_roots(&cwd)?).await?;

        let actual = uut.sanitize("cell1//")?;

        assert!(actual.is_ready_for_next_dir());

        Ok(())
    }

    #[tokio::test]
    async fn test_bails_on_nonexistent_cell() -> buck2_error::Result<()> {
        let cwd = in_root()?;
        let uut =
            PathSanitizer::new(&cell_configs(&cwd)?, &cwd, &find_invocation_roots(&cwd)?).await?;

        assert!(uut.sanitize("boguscell//").is_err());

        Ok(())
    }

    testy!(absolute_path_from_root(in_root()?, &abs_str_from_root("baredir0")?) -> {
        abs_path: from_root("baredir0"),
        canonical: "root//baredir0",
        cell_name: "root",
        cell_path: "baredir0",
        given: "root//baredir0",
        to_string: "root//baredir0",
    });

    testy!(absolute_path_in_subcell(in_dir("cell1")?, &abs_str_from_root("cell1/buck2")?) -> {
        abs_path: from_root("cell1/buck2"),
        canonical: "cell1//buck2",
        cell_name: "cell1",
        cell_path: "buck2",
        given: "cell1//buck2",
        to_string: "cell1//buck2",
    });

    testy!(aliased_cell(in_dir("cell1/buck2/prelude")?, "cell1_alias//buck2") -> {
        abs_path: from_root("cell1/buck2"),
        canonical: "cell1//buck2",
        cell_name: "cell1",
        cell_path: "buck2",
        given: "cell1_alias//buck2",
        to_string: "cell1_alias//buck2",
    });

    #[tokio::test]
    async fn test_creation_returns_error_on_non_local_alias() -> buck2_error::Result<()> {
        let cwd = in_dir("cell1/buck2")?;
        let uut =
            PathSanitizer::new(&cell_configs(&cwd)?, &cwd, &find_invocation_roots(&cwd)?).await?;

        assert!(uut.sanitize("cell1_alias//buck2").is_err());

        Ok(())
    }
}
