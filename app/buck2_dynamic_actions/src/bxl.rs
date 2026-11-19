/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::pin::Pin;

use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_build_api::analysis::registry::RecordedAnalysisValues;
use buck2_build_api::dynamic_value::DynamicValue;
use buck2_build_api::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use buck2_core::deferred::base_deferred_key::BaseDeferredKeyBxl;
use buck2_core::deferred::dynamic::DynamicLambdaResultsKey;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest_config::DigestConfig;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutMap;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;
use dice_futures::cancellation::CancellationObserver;
use futures::Future;
use starlark::values::OwnedFrozenRef;

use crate::deferred::InputArtifactsMaterialized;
use crate::params::FrozenDynamicLambdaParams;

pub static EVAL_BXL_FOR_DYNAMIC_OUTPUT: LateBinding<
    for<'v> fn(
        &'v BaseDeferredKeyBxl,
        DynamicLambdaResultsKey,
        OwnedFrozenRef<'v, &'static FrozenDynamicLambdaParams<'static>>,
        &'v mut DiceComputations,
        InputArtifactsMaterialized,
        &'v BuckMutInsertionMap<&Artifact, &ArtifactValue>,
        BuckMutMap<DynamicValue, FrozenProviderCollectionValue>,
        DigestConfig,
        CancellationObserver,
    ) -> Pin<
        Box<dyn Future<Output = buck2_error::Result<RecordedAnalysisValues>> + Send + 'v>,
    >,
> = LateBinding::new("EVAL_BXL_FOR_DYNAMIC_OUTPUT");

pub async fn eval_bxl_for_dynamic_output<'v>(
    base_deferred_key: &'v BaseDeferredKeyBxl,
    self_key: DynamicLambdaResultsKey,
    dynamic_lambda: OwnedFrozenRef<'_, &'static FrozenDynamicLambdaParams<'static>>,
    dice_ctx: &'v mut DiceComputations<'_>,
    input_artifacts_materialized: InputArtifactsMaterialized,
    ensured_artifacts: &'v BuckMutInsertionMap<&Artifact, &ArtifactValue>,
    resolved_dynamic_values: BuckMutMap<DynamicValue, FrozenProviderCollectionValue>,
    digest_config: DigestConfig,
    liveness: CancellationObserver,
) -> buck2_error::Result<RecordedAnalysisValues> {
    (EVAL_BXL_FOR_DYNAMIC_OUTPUT.get()?)(
        base_deferred_key,
        self_key,
        dynamic_lambda,
        dice_ctx,
        input_artifacts_materialized,
        ensured_artifacts,
        resolved_dynamic_values,
        digest_config,
        liveness,
    )
    .await
}
