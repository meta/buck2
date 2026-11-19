/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgUnusedOptions;

use crate::AuditSubcommand;

/// Inspect package values, visibility, within_view, visibility_cap, within_view_cap and
/// visibility_intersection.
///
/// Package values are set with `write_package_value` function from `PACKAGE` files.
/// Visibility and within_view come from the `package()` function in `PACKAGE` files.
/// Visibility_cap (legacy collapsed shape, without exemptions) and
/// visibility_intersection (per-layer shape) come from `enforce_visibility_intersection()` and within_view_cap from
/// `enforce_within_view_intersection()` in `PACKAGE` files.
#[derive(Debug, clap::Parser, serde::Serialize, serde::Deserialize)]
#[clap(name = "package-values")]
pub struct PackageValuesCommand {
    /// Package names to inspect (like `//foo/bar`, no trailing colon).
    pub packages: Vec<String>,

    /// Command doesn't need these flags, but they are used in mode files, so we need to keep them.
    #[clap(flatten)]
    _target_cfg: TargetCfgUnusedOptions,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,
}

impl AuditSubcommand for PackageValuesCommand {
    fn common_opts(&self) -> &CommonCommandOptions {
        &self.common_opts
    }
}
