/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::interpreter::rule_defs::provider::builtin::local_resource_info::LocalResourceInfo;
use buck2_build_api::interpreter::rule_defs::provider::builtin::local_resource_info::OwnedLocalResourceInfo;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_error::ErrorTag;
use buck2_error::internal_error;
use buck2_hash::BuckMutMap;
use buck2_test_api::data::RequiredLocalResources;
use buck2_test_api::data::TestStage;
use dice::DiceComputations;
use itertools::Itertools;

pub(crate) enum TestStageSimple {
    Listing,
    Testing,
}

impl From<&TestStage> for TestStageSimple {
    fn from(value: &TestStage) -> Self {
        match value {
            TestStage::Listing { .. } => TestStageSimple::Listing,
            TestStage::Testing { .. } => TestStageSimple::Testing,
        }
    }
}

pub(crate) async fn required_providers<'v>(
    dice: &mut DiceComputations<'_>,
    available_resources: BuckMutMap<&'v str, Option<&'v ConfiguredProvidersLabel>>,
    rule_required_resource_names: Vec<&'v str>,
    required_local_resources: &'v RequiredLocalResources,
) -> buck2_error::Result<Vec<(&'v ConfiguredTargetLabel, OwnedLocalResourceInfo)>> {
    let targets = required_local_resources
        .resources
        .iter()
        .map(|resource_type| &resource_type.name as &'v str)
        .chain(rule_required_resource_names)
        .unique()
        .map(|type_name| {
            available_resources.get(type_name).copied().ok_or_else(|| {
                buck2_error::buck2_error!(
                    ErrorTag::Input,
                    "Required local resource of type `{type_name}` not found.",
                )
            })
        })
        .filter_map(|r| match r {
            Ok(Some(x)) => Some(Ok(x)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        })
        .collect::<Result<Vec<_>, buck2_error::Error>>()?;
    // Several resource types may name the same provider; it is set up once.
    let targets = targets.into_iter().unique().collect::<Vec<_>>();

    dice.compute_join(targets, async |dice, target| {
        get_local_resource_info(dice, target).await
    })
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
}

async fn get_local_resource_info<'v>(
    dice: &mut DiceComputations<'_>,
    target: &'v ConfiguredProvidersLabel,
) -> buck2_error::Result<(&'v ConfiguredTargetLabel, OwnedLocalResourceInfo)> {
    let local_resource_info = dice
        .get_providers(target)
        .await?
        .require_compatible()?
        .builtin_provider_value::<LocalResourceInfo>()?
        .ok_or_else(|| {
            internal_error!("Target `{target}` expected to contain `LocalResourceInfo` provider")
        })?;
    Ok((target.target(), local_resource_info))
}
