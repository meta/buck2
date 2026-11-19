/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgUnusedOptions;

use crate::AuditSubcommand;

#[derive(Debug, clap::Parser, serde::Serialize, serde::Deserialize)]
#[clap(
    name = "audit-file-package",
    about = "Map file paths to fully qualified package names."
)]
pub struct AuditFilePackageCommand {
    #[clap(long = "json", help = "Output in JSON format")]
    pub json: bool,

    #[clap(
        name = "PATHS",
        help = "File paths to resolve to package names (build files, directories, or any source files)"
    )]
    pub paths: Vec<String>,

    /// Command doesn't need these flags, but they are used in mode files, so we need to keep them.
    #[clap(flatten)]
    _target_cfg: TargetCfgUnusedOptions,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,
}

#[async_trait]
impl AuditSubcommand for AuditFilePackageCommand {
    fn common_opts(&self) -> &CommonCommandOptions {
        &self.common_opts
    }
}
