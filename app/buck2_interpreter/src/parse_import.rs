/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Parses imports for load_file() calls in build files.

use buck2_core::bzl::ImportPath;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::build_file_cell::BuildFileCell;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::cell_path_with_allowed_relative_dir::CellPathWithAllowedRelativeDir;
use buck2_core::pattern::parse_package::parse_cell_path;
use buck2_fs::paths::RelativePath;
use buck2_fs::paths::file_name::FileName;

#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
enum ImportParseError {
    #[error(
        "Unable to parse import spec. Expected format `(@<cell>)//package/name:filename.bzl` or `:filename.bzl`. Got `{0}`"
    )]
    MatchFailed(String),
    #[error(
        "Unable to parse import spec. Expected format `(@<cell>)//package/name:filename.bzl` or `:filename.bzl`, but got an empty filename. Got `{0}`"
    )]
    EmptyFileName(String),
    #[error("Unexpected relative import spec. Got `{0}`")]
    ProhibitedRelativeImport(String),
    #[error(
        "Cell-qualified load path `{import}` must begin with `@` to reference another cell (did you mean `{suggestion}`?). A bare cell alias like `cell//...` is not accepted in `load()`."
    )]
    CellAliasMissingAtSymbol { import: String, suggestion: String },
    #[error(
        "Unable to parse import spec. Expected format `(@<cell>)//package/name:filename.bzl` or `:filename.bzl`, but got a path. Got `{0}`"
    )]
    NotAFileName(String),
}

pub enum RelativeImports<'a> {
    Allow {
        current_dir_with_allowed_relative: &'a CellPathWithAllowedRelativeDir,
    },
    Disallow,
}

/// Extra options for parsing a load() or load-like path into a `BuckPath`
pub struct ParseImportOptions<'a> {
    /// Whether '@' is required at the beginning of the import.
    pub allow_missing_at_symbol: bool,
    /// Whether relative imports (':bar.bzl') are allowed.
    pub relative_import_option: RelativeImports<'a>,
}

/// Returns true if the path has a cell alias prefix (e.g. `@cell//path` or `//path`),
/// respecting the `allow_missing_at_symbol` option.
fn has_cell_alias_prefix(path: &str, allow_missing_at_symbol: bool) -> bool {
    let Some((alias, _)) = path.split_once("//") else {
        return false;
    };
    alias.is_empty() || alias.starts_with('@') || allow_missing_at_symbol
}

/// `cell//...` with a non-empty alias and no leading `@`.
fn has_bare_cell_alias_prefix(path: &str) -> bool {
    path.split_once("//")
        .is_some_and(|(alias, _)| !alias.is_empty() && !alias.starts_with('@'))
}

pub fn parse_import(
    cell_resolver: &CellAliasResolver,
    relative_import_option: RelativeImports,
    import: &str,
) -> buck2_error::Result<CellPath> {
    let opts: ParseImportOptions = ParseImportOptions {
        allow_missing_at_symbol: false,
        relative_import_option,
    };
    parse_import_with_config(cell_resolver, import, &opts)
}

/// Parse import string into a BuckPath, but potentially be more or less flexible with what is
/// accepted.
///
/// Common use case is e.g. allowing "cell//foo:bar.bzl" to be passed on the command line
/// and letting that be turned into an ImportPath eventually, or disallowing relative imports
/// from command line arguments.
///
/// Strings for the `load()` statement in starlark files should use [`parse_import`]
pub fn parse_import_with_config(
    cell_resolver: &CellAliasResolver,
    import: &str,
    opts: &ParseImportOptions,
) -> buck2_error::Result<CellPath> {
    match import.split_once(':') {
        None => {
            // import without `:`, so just try to parse the cell and cell relative paths

            if has_cell_alias_prefix(import, opts.allow_missing_at_symbol) {
                Ok(parse_cell_path(import, cell_resolver)?)
            } else if has_bare_cell_alias_prefix(import) {
                // `cell//pkg/file` looks cell-qualified, but the alias lacks the
                // required leading `@`. Give a targeted hint rather than treating
                // it as a relative import that fails later with an opaque error.
                Err(ImportParseError::CellAliasMissingAtSymbol {
                    import: import.to_owned(),
                    suggestion: format!("@{import}"),
                }
                .into())
            } else if let RelativeImports::Allow {
                current_dir_with_allowed_relative,
            } = opts.relative_import_option
            {
                current_dir_with_allowed_relative
                    .join_normalized(RelativePath::unchecked_new(import))
            } else {
                Err(ImportParseError::ProhibitedRelativeImport(import.to_owned()).into())
            }
        }
        Some((path, filename)) => {
            if filename.is_empty() {
                return Err(ImportParseError::EmptyFileName(import.to_owned()).into());
            }

            let filename = FileName::new(filename)
                .map_err(|_| ImportParseError::NotAFileName(import.to_owned()))?;

            if path.is_empty() {
                if let RelativeImports::Allow {
                    current_dir_with_allowed_relative,
                } = opts.relative_import_option
                {
                    Ok(current_dir_with_allowed_relative.join(filename))
                } else {
                    Err(ImportParseError::ProhibitedRelativeImport(import.to_owned()).into())
                }
            } else if has_cell_alias_prefix(path, opts.allow_missing_at_symbol) {
                let cell_path = parse_cell_path(path, cell_resolver)?;
                Ok(cell_path.join(filename))
            } else {
                Err(ImportParseError::MatchFailed(import.to_owned()).into())
            }
        }
    }
}

pub fn parse_bzl_path_with_config(
    cell_resolver: &CellAliasResolver,
    import: &str,
    opts: &ParseImportOptions,
    build_cell_path: BuildFileCell,
) -> buck2_error::Result<ImportPath> {
    let path = parse_import_with_config(cell_resolver, import, opts)?;
    ImportPath::new_with_build_file_cells(path, build_cell_path)
}

#[cfg(test)]
mod tests {
    use buck2_core::cells::alias::NonEmptyCellAlias;
    use buck2_core::cells::name::CellName;
    use buck2_hash::IntentionallyStdHashMap;

    use super::*;

    fn resolver() -> CellAliasResolver {
        let mut m = IntentionallyStdHashMap::new();
        m.insert(
            NonEmptyCellAlias::new("cell1".to_owned()).unwrap(),
            CellName::testing_new("cell1"),
        );
        m.insert(
            NonEmptyCellAlias::new("alias2".to_owned()).unwrap(),
            CellName::testing_new("cell2"),
        );
        CellAliasResolver::new(CellName::testing_new("root"), m).expect("valid resolver")
    }

    fn path(path: &str) -> CellPath {
        CellPath::testing_new(path)
    }

    #[test]
    fn root_package() -> buck2_error::Result<()> {
        assert_eq!(
            path("root//package/path/import.bzl"),
            parse_import(
                &resolver(),
                RelativeImports::Allow {
                    current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                        CellPath::testing_new("passport//"),
                        None,
                    ),
                },
                "//package/path:import.bzl"
            )?
        );
        Ok(())
    }

    #[test]
    fn cell_package() -> buck2_error::Result<()> {
        assert_eq!(
            path("cell1//package/path/import.bzl"),
            parse_import(
                &resolver(),
                RelativeImports::Allow {
                    current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                        CellPath::testing_new("root//"),
                        None,
                    ),
                },
                "@cell1//package/path:import.bzl"
            )?
        );
        Ok(())
    }

    #[test]
    fn package_relative() -> buck2_error::Result<()> {
        assert_eq!(
            path("cell1//package/path/import.bzl"),
            parse_import(
                &resolver(),
                RelativeImports::Allow {
                    current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                        CellPath::testing_new("cell1//package/path"),
                        None,
                    ),
                },
                ":import.bzl"
            )?
        );
        Ok(())
    }

    #[test]
    fn missing_colon() -> buck2_error::Result<()> {
        let import = "//package/path/import.bzl".to_owned();
        assert_eq!(
            parse_import(
                &resolver(),
                RelativeImports::Allow {
                    current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                        CellPath::testing_new("lighter//"),
                        None,
                    ),
                },
                &import
            )?,
            path("root//package/path/import.bzl")
        );
        Ok(())
    }

    #[test]
    fn empty_filename() -> buck2_error::Result<()> {
        let path = "//package/path:".to_owned();
        match parse_import(
            &resolver(),
            RelativeImports::Allow {
                current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                    CellPath::testing_new("root//"),
                    None,
                ),
            },
            &path,
        ) {
            Ok(import) => panic!("Expected parse failure for {path}, got result {import}"),
            Err(e) => {
                assert_eq!(
                    format!("{e:#}"),
                    ImportParseError::EmptyFileName(path.to_owned()).to_string()
                );
            }
        }
        Ok(())
    }

    #[test]
    fn bad_alias() -> buck2_error::Result<()> {
        let path = "bad_alias//package/path:".to_owned();
        match parse_import(
            &resolver(),
            RelativeImports::Allow {
                current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                    CellPath::testing_new("root//"),
                    None,
                ),
            },
            &path,
        ) {
            Ok(import) => panic!("Expected parse failure for {path}, got result {import}"),
            Err(_) => {
                // TODO: should we verify the contents of the error?
            }
        }
        Ok(())
    }

    #[test]
    fn file_relative_import_given_relative_paths_allowed() -> buck2_error::Result<()> {
        assert_eq!(
            path("cell1//package/path/bar.bzl"),
            parse_import(
                &resolver(),
                RelativeImports::Allow {
                    current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                        CellPath::testing_new("cell1//package/path"),
                        None,
                    ),
                },
                "bar.bzl",
            )?
        );
        assert_eq!(
            path("cell1//package/path/foo/bar.bzl"),
            parse_import(
                &resolver(),
                RelativeImports::Allow {
                    current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                        CellPath::testing_new("cell1//package/path"),
                        None,
                    ),
                },
                "foo/bar.bzl",
            )?
        );
        Ok(())
    }

    #[test]
    fn malformed_relative_import_with_allowed_dir() -> buck2_error::Result<()> {
        let without_allowed_dir =
            CellPathWithAllowedRelativeDir::new(CellPath::testing_new("cell1//package/path"), None);
        let with_allowed_dir = CellPathWithAllowedRelativeDir::new(
            CellPath::testing_new("cell1//package/path"),
            Some(CellPath::testing_new("cell1//package")),
        );
        // Each of these is rejected whether or not an allowed relative dir is set.
        let cases = [
            "/bar.bzl",
            "bar.bzl/",
            "foo//bar.bzl",
            "alias2//bar.bzl",
            "..//sibling.bzl",
            "/../sibling.bzl",
        ];
        for import in cases {
            for current_dir in [&without_allowed_dir, &with_allowed_dir] {
                assert!(
                    parse_import(
                        &resolver(),
                        RelativeImports::Allow {
                            current_dir_with_allowed_relative: current_dir,
                        },
                        import,
                    )
                    .is_err(),
                    "{import}"
                );
            }
        }
        // A well-formed relative import still resolves under the allowed dir.
        assert_eq!(
            path("cell1//package/sibling.bzl"),
            parse_import(
                &resolver(),
                RelativeImports::Allow {
                    current_dir_with_allowed_relative: &with_allowed_dir,
                },
                "../sibling.bzl",
            )?
        );
        Ok(())
    }

    #[test]
    fn cell_relative_import_given_relative_paths_allowed() -> buck2_error::Result<()> {
        let importer = CellPath::testing_new("cell1//package/path");
        let importee = "foo/bar.bzl";

        assert_eq!(
            parse_import(
                &resolver(),
                RelativeImports::Allow {
                    current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                        importer, None,
                    ),
                },
                importee
            )?,
            path("cell1//package/path/foo/bar.bzl")
        );
        Ok(())
    }

    #[test]
    fn regular_import_given_relative_paths_allowed() -> buck2_error::Result<()> {
        assert_eq!(
            path("cell1//package/path/import.bzl"),
            parse_import(
                &resolver(),
                RelativeImports::Allow {
                    current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                        CellPath::testing_new("root//foo/bar"),
                        None,
                    ),
                },
                "@cell1//package/path:import.bzl",
            )?
        );
        Ok(())
    }

    #[test]
    fn allows_non_at_symbols() -> buck2_error::Result<()> {
        assert_eq!(
            path("cell1//package/path/import.bzl"),
            parse_import_with_config(
                &resolver(),
                "cell1//package/path:import.bzl",
                &ParseImportOptions {
                    allow_missing_at_symbol: true,
                    relative_import_option: RelativeImports::Allow {
                        current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                            CellPath::testing_new("root//"),
                            None,
                        ),
                    },
                }
            )?,
        );
        Ok(())
    }

    #[test]
    fn cell_alias_without_at_symbol_is_rejected_with_hint() -> buck2_error::Result<()> {
        // A bare `cell//...` (no leading `@`, no colon) must not be silently
        // treated as a relative import and fail later with an opaque
        // forward-relative-path error. It should report the missing `@`.
        let import = "other//pkg/lib.lock";
        let res = parse_import(
            &resolver(),
            RelativeImports::Allow {
                current_dir_with_allowed_relative: &CellPathWithAllowedRelativeDir::new(
                    CellPath::testing_new("root//pkg"),
                    None,
                ),
            },
            import,
        );
        match res {
            Ok(res) => panic!("Expected parse failure for {import}, got result {res}"),
            Err(e) => {
                assert_eq!(
                    format!("{e:#}"),
                    ImportParseError::CellAliasMissingAtSymbol {
                        import: import.to_owned(),
                        suggestion: format!("@{import}"),
                    }
                    .to_string()
                );
            }
        };
        Ok(())
    }

    #[test]
    fn fails_relative_import_if_disallowed() -> buck2_error::Result<()> {
        let imported_file = ":bar.bzl";
        let res = parse_import_with_config(
            &resolver(),
            imported_file,
            &ParseImportOptions {
                allow_missing_at_symbol: false,
                relative_import_option: RelativeImports::Disallow,
            },
        );
        match res {
            Ok(res) => panic!("Expected parse failure for {imported_file}, got result {res}"),
            Err(e) => {
                assert_eq!(
                    format!("{e:#}"),
                    ImportParseError::ProhibitedRelativeImport(imported_file.to_owned())
                        .to_string()
                );
            }
        };
        Ok(())
    }
}
