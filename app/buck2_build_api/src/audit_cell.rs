/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_hash::BuckMutInsertionMap;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;
use futures::future::BoxFuture;

pub static AUDIT_CELL: LateBinding<
    for<'v> fn(
        ctx: &'v mut DiceComputations<'_>,
        aliases_to_resolve: &'v Vec<String>,
        aliases: bool,
        cwd: &'v ProjectRelativePath,
        fs: &'v ProjectRoot,
    )
        -> BoxFuture<'v, buck2_error::Result<BuckMutInsertionMap<String, AbsNormPathBuf>>>,
> = LateBinding::new("AUDIT_CELL");

pub fn audit_cell<'v>(
    ctx: &'v mut DiceComputations<'_>,
    aliases_to_resolve: &'v Vec<String>,
    aliases: bool,
    cwd: &'v ProjectRelativePath,
    fs: &'v ProjectRoot,
) -> buck2_error::Result<
    BoxFuture<'v, buck2_error::Result<BuckMutInsertionMap<String, AbsNormPathBuf>>>,
> {
    Ok((AUDIT_CELL.get()?)(
        ctx,
        aliases_to_resolve,
        aliases,
        cwd,
        fs,
    ))
}
