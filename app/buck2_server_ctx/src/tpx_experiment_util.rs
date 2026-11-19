/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::legacy_configs::dice::HasInjectedLegacyConfigs;
use buck2_core::fs::project::ProjectRoot;
use buck2_hash::BuckMutSet;
use dice::DiceTransaction;

use crate::experiment_util::get_experiment_tags;

/// Get all TPX experiments from buckconfig
///
/// This function retrieves all experiments from buckconfig that start with "experiments.tpx_"
/// and returns them as a HashSet with the "experiments.tpx_" prefix removed.
pub async fn get_tpx_experiments(
    ctx: DiceTransaction,
    project_root: &ProjectRoot,
) -> buck2_error::Result<BuckMutSet<String>> {
    // Get all experiments from buckconfig
    if !ctx
        .ctx()
        .is_injected_external_buckconfig_data_key_set()
        .await?
    {
        return Ok(BuckMutSet::default());
    }

    let external_configs = ctx.ctx().get_injected_external_buckconfig_data().await?;
    let current_external_and_local_configs = external_configs
        .get_buckconfig_components(project_root)
        .await;
    let experiment_tags = get_experiment_tags(&current_external_and_local_configs);
    // Filter experiments that start with "experiments.tpx_"
    let tpx_experiments = experiment_tags
        .into_iter()
        .filter(|tag| tag.starts_with("experiments.tpx_"))
        .map(|tag| tag.replace("experiments.tpx_", ""))
        .collect::<BuckMutSet<String>>();

    Ok(tpx_experiments)
}
