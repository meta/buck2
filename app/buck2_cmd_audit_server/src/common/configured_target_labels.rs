/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_client_ctx::common::target_cfg::TargetCfgWithUniverseOptions;
use buck2_core::pattern::pattern_type::ConfiguredTargetPatternExtra;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::pattern_parse_and_resolve::parse_and_resolve_patterns_to_targets_from_cli_args;
use dice::DiceComputations;

use crate::common::target_resolution_config::audit_command_target_resolution_config;

pub(crate) async fn audit_command_configured_target_labels(
    ctx: &mut DiceComputations<'_>,
    patterns: &[String],
    target_cfg: &TargetCfgWithUniverseOptions,
    server_ctx: &dyn ServerCommandContextTrait,
) -> buck2_error::Result<Vec<ConfiguredTargetLabel>> {
    let targets =
        parse_and_resolve_patterns_to_targets_from_cli_args::<ConfiguredTargetPatternExtra>(
            ctx,
            patterns,
            server_ctx.working_dir(),
        )
        .await?;

    let target_resolution_config =
        audit_command_target_resolution_config(ctx, target_cfg, server_ctx).await?;

    let mut configured_target_labels: Vec<ConfiguredTargetLabel> = Vec::new();
    for target in targets {
        configured_target_labels.extend(
            target_resolution_config
                .get_configured_targets_for_configured_target_literals(ctx, &target)
                .await?,
        );
    }

    Ok(configured_target_labels)
}
