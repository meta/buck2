/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::OnceCell;

use allocative::Allocative;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_error::BuckErrorOptionContext;
use buck2_error::conversion::from_any_with_tag;
use starlark::StarlarkPagable;
use starlark::StarlarkPagablePanic;
use starlark::any::ProvidesStaticType;
use starlark::environment::FrozenModule;
use starlark::environment::Module;
use starlark::values::Freeze;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::OwnedFrozen;
use starlark::values::Trace;
use starlark::values::ValueTyped;
use starlark::values::any_complex::StarlarkAnyComplex;

use crate::analysis::registry::AnalysisValueStorage;
use crate::analysis::registry::FrozenAnalysisValueStorage;

#[derive(
    Default,
    Debug,
    ProvidesStaticType,
    Allocative,
    Trace,
    StarlarkPagablePanic
)]
pub struct AnalysisExtraValue<'v> {
    pub analysis_value_storage:
        OnceCell<ValueTyped<'v, StarlarkAnyComplex<AnalysisValueStorage<'v>>>>,
    /// The execution platform resolution of the target whose rule is being analyzed. Unset for
    /// anon targets and outside rule analysis.
    #[trace(static)]
    pub execution_platform_resolution: OnceCell<ExecutionPlatformResolution>,
}

#[derive(Debug, ProvidesStaticType, Allocative, StarlarkPagable)]
pub struct FrozenAnalysisExtraValue<'fv> {
    pub(crate) analysis_value_storage:
        Option<ValueTyped<'fv, StarlarkAnyComplex<FrozenAnalysisValueStorage<'fv>>>>,
}

starlark::register_starlark_any_complex!(AnalysisExtraValue<'_>, frozen FrozenAnalysisExtraValue<'_>);

impl<'v> Freeze<'v> for AnalysisExtraValue<'v> {
    type Frozen<'fv> = FrozenAnalysisExtraValue<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let AnalysisExtraValue {
            analysis_value_storage,
            execution_platform_resolution: _,
        } = self;
        Ok(FrozenAnalysisExtraValue {
            analysis_value_storage: Freeze::freeze(analysis_value_storage, freezer)?,
        })
    }
}

impl<'v> AnalysisExtraValue<'v> {
    pub fn get(module: &Module<'v>) -> buck2_error::Result<Option<&'v AnalysisExtraValue<'v>>> {
        let Some(extra) = module.extra_value() else {
            return Ok(None);
        };
        Ok(Some(
            &extra
                .downcast_ref_err::<StarlarkAnyComplex<AnalysisExtraValue>>()?
                .value,
        ))
    }

    /// The execution platform resolution of the rule analysis that `module` belongs to. `None`
    /// outside rule analysis, including modules whose extra value is not an analysis one.
    pub fn execution_platform_resolution(
        module: &Module<'v>,
    ) -> Option<&'v ExecutionPlatformResolution> {
        module
            .extra_value()?
            .downcast_ref::<StarlarkAnyComplex<AnalysisExtraValue>>()?
            .value
            .execution_platform_resolution
            .get()
    }

    pub fn get_or_init(module: &Module<'v>) -> buck2_error::Result<&'v AnalysisExtraValue<'v>> {
        if let Some(extra) = Self::get(module)? {
            return Ok(extra);
        }
        module
            .set_extra_value_no_overwrite(
                module
                    .heap()
                    .alloc_representation(StarlarkAnyComplex::new(AnalysisExtraValue::default()))
                    .to_value(),
            )
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))?;
        Self::get(module)?.internal_error("extra_value must be set")
    }
}

/// A frozen module's extra value, kept alive by that module's heap.
pub type OwnedFrozenAnalysisExtraValue =
    OwnedFrozen<ValueTyped<'static, StarlarkAnyComplex<FrozenAnalysisExtraValue<'static>>>>;

/// The [`AnalysisValueStorage`] written into a frozen module, kept alive by that module's heap.
pub type OwnedFrozenAnalysisValueStorage =
    OwnedFrozen<ValueTyped<'static, StarlarkAnyComplex<FrozenAnalysisValueStorage<'static>>>>;

impl FrozenAnalysisExtraValue<'_> {
    pub fn get(module: &FrozenModule) -> buck2_error::Result<OwnedFrozenAnalysisExtraValue> {
        module
            .extra_value()
            .internal_error("extra_value not set")?
            .maybe_map::<ValueTyped<'static, StarlarkAnyComplex<FrozenAnalysisExtraValue>>, _>(
                |v| ValueTyped::new(v),
            )
            .internal_error("extra_value has the wrong type")
    }

    pub fn analysis_value_storage(
        module: &FrozenModule,
    ) -> buck2_error::Result<OwnedFrozenAnalysisValueStorage> {
        Self::get(module)?.try_map(|v| {
            v.as_ref()
                .value
                .analysis_value_storage
                .internal_error("analysis_value_storage not set")
        })
    }
}
