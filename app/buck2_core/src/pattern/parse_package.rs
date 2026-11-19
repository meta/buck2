/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_fs::paths::forward_rel_path::ForwardRelativePath;

use crate::cells::CellAliasResolver;
use crate::cells::cell_path::CellPath;
use crate::cells::paths::CellRelativePath;
use crate::package::PackageLabel;
use crate::pattern::ascii_pattern::AsciiChar;
use crate::pattern::ascii_pattern::AsciiStr2;
use crate::pattern::ascii_pattern::split1_opt_ascii;
use crate::pattern::ascii_pattern::trim_prefix_ascii;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum ParseCellPathError {
    #[error("Expected `//` in cell path: `{0}`")]
    NoSlashSlash(String),
}

/// Parse a `cell//path` string into a `CellPath`.
/// Supports optional `@` prefix on the cell alias (e.g. `@cell//path`).
pub fn parse_cell_path(
    path: &str,
    cell_alias_resolver: &CellAliasResolver,
) -> buck2_error::Result<CellPath> {
    maybe_parse_cell_path(path, cell_alias_resolver)?
        .ok_or_else(|| ParseCellPathError::NoSlashSlash(path.to_owned()).into())
}

/// Like `parse_cell_path`, but returns `None` if `//` is not present
/// instead of returning an error.
pub fn maybe_parse_cell_path(
    path: &str,
    cell_alias_resolver: &CellAliasResolver,
) -> buck2_error::Result<Option<CellPath>> {
    let Some((cell_alias, cell_relative)) = split1_opt_ascii(path, AsciiStr2::new("//")) else {
        return Ok(None);
    };

    let cell_alias = trim_prefix_ascii(cell_alias, AsciiChar::new('@'));
    let cell = cell_alias_resolver.resolve(cell_alias)?;
    let cell_relative = ForwardRelativePath::new(cell_relative)?;
    let cell_relative = CellRelativePath::new(cell_relative);

    Ok(Some(CellPath::new(cell, cell_relative)))
}

/// Parse a `cell//path` string into a `PackageLabel`.
pub fn parse_package(
    package: &str,
    cell_alias_resolver: &CellAliasResolver,
) -> buck2_error::Result<PackageLabel> {
    let cell_path = parse_cell_path(package, cell_alias_resolver)?;
    PackageLabel::new(cell_path.cell(), cell_path.path())
}

#[cfg(test)]
mod tests {

    use crate::cells::CellAliasResolver;
    use crate::cells::alias::NonEmptyCellAlias;
    use crate::cells::name::CellName;
    use crate::pattern::parse_package::parse_cell_path;
    use crate::pattern::parse_package::parse_package;

    fn test_resolver() -> CellAliasResolver {
        CellAliasResolver::new(
            CellName::testing_new("oof"),
            [(
                NonEmptyCellAlias::new("foo".to_owned()).unwrap(),
                CellName::testing_new("oof"),
            )]
            .into_iter()
            .collect(),
        )
        .unwrap()
    }

    #[test]
    fn test_parse_package() {
        let resolver = test_resolver();
        let package = parse_package("foo//bar/baz", &resolver).unwrap();
        assert_eq!("oof", package.cell_name().as_str());
        assert_eq!("bar/baz", package.cell_relative_path().as_str());
    }

    #[test]
    fn test_parse_cell_path() {
        let resolver = test_resolver();
        let cell_path = parse_cell_path("foo//bar/baz", &resolver).unwrap();
        assert_eq!("oof", cell_path.cell().as_str());
        assert_eq!("bar/baz", cell_path.path().as_str());
    }

    #[test]
    fn test_parse_cell_path_with_at_prefix() {
        let resolver = test_resolver();
        let cell_path = parse_cell_path("@foo//bar/baz", &resolver).unwrap();
        assert_eq!("oof", cell_path.cell().as_str());
        assert_eq!("bar/baz", cell_path.path().as_str());
    }
}
