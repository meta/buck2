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
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgUnusedOptions;
use buck2_error::BuckErrorContext;

use crate::AuditSubcommand;

/// Unlike the other audit subcommands this one runs on the client: settings
/// are resolved there, from files and flags the daemon never sees, and the
/// daemon only receives the result.
#[derive(Debug, clap::Parser, serde::Serialize, serde::Deserialize)]
#[clap(
    name = "audit-settings",
    about = "Show the Buck settings in effect and where each value comes from"
)]
pub struct AuditSettingsCommand {
    /// Output the report as JSON.
    #[clap(long)]
    json: bool,

    /// Also show each setting's documentation, type, defaults and override
    /// policy.
    #[clap(long)]
    verbose: bool,

    /// Command doesn't need these flags, but they are used in mode files, so we need to keep them.
    #[clap(flatten)]
    _target_cfg: TargetCfgUnusedOptions,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,
}

impl AuditSettingsCommand {
    pub fn exec(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
    ) -> buck2_error::Result<()> {
        let report = ctx.settings_report()?;
        let text = if self.json {
            serde_json::to_string_pretty(&report)
                .map_err(|e| {
                    buck2_error::conversion::from_any_with_tag(e, buck2_error::ErrorTag::Tier0)
                })
                .buck_error_context("Serializing the settings report")?
        } else {
            report.render(self.verbose)
        };
        buck2_client_ctx::println!("{}", text.trim_end())?;
        Ok(())
    }
}

#[async_trait]
impl AuditSubcommand for AuditSettingsCommand {
    fn common_opts(&self) -> &CommonCommandOptions {
        &self.common_opts
    }
}
