/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::marker::PhantomData;

use allocative::Allocative;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_util::late_binding::LateBinding;
use starlark::any::AnyLifetime;
use starlark::values::DynStarlark;
use starlark::values::HeapSendable;
use starlark::values::Trace;
use starlark::values::Value;

use crate::analysis::anon_promises_dyn::AnonPromisesDyn;
use crate::artifact_groups::promise::PromiseArtifact;

pub static ANON_TARGET_REGISTRY_NEW: LateBinding<
    for<'v> fn(
        PhantomData<Value<'v>>,
        ExecutionPlatformResolution,
    ) -> Box<DynStarlark<'v, dyn AnonTargetsRegistryDyn<'v> + 'v>>,
> = LateBinding::new("ANON_TARGET_REGISTRY_NEW");

pub trait AnonTargetsRegistryDyn<'v>:
    Debug + Allocative + Trace<'v> + HeapSendable<'v> + AnyLifetime<'v> + 'v
{
    fn as_any_mut(&mut self) -> &mut dyn AnyLifetime<'v>;
    fn take_promises(&mut self) -> Option<Box<dyn AnonPromisesDyn<'v>>>;
    fn consumer_analysis_artifacts(&self) -> Vec<PromiseArtifact>;
    fn assert_no_promises(&self) -> buck2_error::Result<()>;
}
