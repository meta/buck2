/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_build_api::dynamic_value::DynamicValue;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_output_artifact::FrozenStarlarkOutputArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifact;
use buck2_build_api::interpreter::rule_defs::plugins::AnalysisPlugins;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use mini_vec::MiniBoxSlice;
use starlark::StarlarkPagable;
use starlark::any::ProvidesStaticType;
use starlark::values::Freeze;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::FrozenValueTyped;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::structs::StructRef;
use starlark::values::typing::StarlarkCallable;

use crate::attrs::DynamicAttrValues;
use crate::dynamic_actions_callable::FrozenStarlarkDynamicActionsCallable;

#[derive(Allocative, Debug, StarlarkPagable)]
pub struct DynamicLambdaStaticFields {
    /// Input artifacts required to be materialized by the lambda.
    pub artifact_values: MiniBoxSlice<Artifact>,
    /// Dynamic values I depend on.
    pub dynamic_values: MiniBoxSlice<DynamicValue>,
    /// Execution platform inherited from the owner to use for actionsfbcode/buck2/app/buck2_action_impl/src/dynamic/deferred.rs
    #[starlark_pagable(pagable)]
    pub execution_platform: ExecutionPlatformResolution,
}

#[derive(Allocative, Trace, Debug, ProvidesStaticType)]
pub struct DynamicLambdaParams<'v> {
    pub attributes: Option<ValueOf<'v, StructRef<'v>>>,
    pub plugins: Option<ValueTyped<'v, AnalysisPlugins<'v>>>,
    pub lambda: StarlarkCallable<'v>,
    pub attr_values: Option<(
        DynamicAttrValues<'v>,
        FrozenValueTyped<'v, FrozenStarlarkDynamicActionsCallable<'v>>,
    )>,
    pub outputs: MiniBoxSlice<ValueTyped<'v, StarlarkOutputArtifact<'v>>>,
    pub static_fields: DynamicLambdaStaticFields,
}

#[derive(Allocative, Debug, ProvidesStaticType, StarlarkPagable)]
pub struct FrozenDynamicLambdaParams<'fv> {
    attributes: Option<ValueOf<'fv, StructRef<'fv>>>,
    plugins: Option<ValueTyped<'fv, AnalysisPlugins<'fv>>>,
    lambda: StarlarkCallable<'fv>,
    pub attr_values: Option<(
        DynamicAttrValues<'fv>,
        FrozenValueTyped<'fv, FrozenStarlarkDynamicActionsCallable<'fv>>,
    )>,
    pub outputs: MiniBoxSlice<ValueTyped<'fv, FrozenStarlarkOutputArtifact<'fv>>>,
    pub static_fields: DynamicLambdaStaticFields,
}

impl<'fv> FrozenDynamicLambdaParams<'fv> {
    pub fn attributes(&self) -> Option<ValueOf<'fv, StructRef<'fv>>> {
        self.attributes
    }

    pub fn plugins(&self) -> Option<ValueTyped<'fv, AnalysisPlugins<'fv>>> {
        self.plugins
    }

    pub fn lambda(&self) -> Value<'fv> {
        self.lambda.0
    }
}

impl<'v> Freeze<'v> for DynamicLambdaParams<'v> {
    type Frozen<'fv> = FrozenDynamicLambdaParams<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        Ok(FrozenDynamicLambdaParams {
            attributes: self.attributes.freeze(freezer)?,
            plugins: self.plugins.freeze(freezer)?,
            lambda: self.lambda.freeze(freezer)?,
            attr_values: self.attr_values.freeze(freezer)?,
            outputs: self.outputs.freeze(freezer)?,
            static_fields: self.static_fields,
        })
    }
}
