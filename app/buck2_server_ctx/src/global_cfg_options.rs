/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_cli_proto::TargetCfg;
use buck2_common::dice::cells::HasCellResolver;
use buck2_core::cells::CellResolver;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::pattern::pattern::ParsedPattern;
use dice::DiceComputations;

use crate::ctx::ServerCommandContextTrait;

/// Extract target configuration components.
pub async fn global_cfg_options_from_client_context(
    target_cfg: &TargetCfg,
    server_ctx: &dyn ServerCommandContextTrait,
    dice_ctx: &mut DiceComputations<'_>,
) -> buck2_error::Result<GlobalCfgOptions> {
    let cell_resolver: &CellResolver = dice_ctx.get_cell_resolver().await?;
    let working_dir: &ProjectRelativePath = server_ctx.working_dir();
    let cwd = cell_resolver.get_cell_path(working_dir);
    let cell_alias_resolver = dice_ctx.get_cell_alias_resolver(cwd.cell()).await?;
    let target_platform = &target_cfg.target_platform;
    let target_platform_label = if !target_platform.is_empty() {
        Some(
            ParsedPattern::parse_precise(
                target_platform,
                cwd.cell(),
                cell_resolver,
                &cell_alias_resolver,
            )?
            .as_target_label(target_platform)?,
        )
    } else {
        None
    };

    Ok(GlobalCfgOptions {
        target_platform: target_platform_label,
        cli_modifiers: target_cfg.cli_modifiers.clone().into(),
    })
}
