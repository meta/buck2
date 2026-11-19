/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use buck2_fs::paths::abs_norm_path::AbsNormPath;

use crate::cells::CellAliasResolver;
use crate::cells::CellResolver;
use crate::cells::cell_path::CellPath;
use crate::fs::project::ProjectRoot;
use crate::pattern::parse_package::maybe_parse_cell_path;

pub fn parse_query_file_literal(
    literal: &str,
    cell_alias_resolver: &CellAliasResolver,
    cell_resolver: &CellResolver,
    working_dir_abs: &AbsNormPath,
    project_root: &ProjectRoot,
) -> buck2_error::Result<CellPath> {
    match maybe_parse_cell_path(literal, cell_alias_resolver)? {
        Some(cell_path) => Ok(cell_path),
        None => {
            let path = Path::new(literal);
            // Note if the path is absolute, this `join` is a no-op.
            let path_abs = working_dir_abs.as_abs_path().join(path);
            let project_path = project_root.relativize_any(path_abs)?;
            Ok(cell_resolver.get_cell_path(&project_path))
        }
    }
}
