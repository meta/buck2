/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! `ConstraintValueInfo` is the provider info returned from a `constraint_value()` rule.

use std::fmt::Debug;

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use buck2_core::configuration::constraints::ConstraintKey;
use buck2_core::configuration::constraints::ConstraintValue;
use buck2_interpreter::types::configured_providers_label::StarlarkProvidersLabel;
use buck2_interpreter::types::target_label::LabelArg;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::Heap;
use starlark::values::StarlarkPagable;
use starlark::values::Trace;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;

use crate::constraint_setting_info::ConstraintSettingInfo;

/// Provider that signals that a target can be used as a constraint key. This is the only provider
/// returned by a `constraint_value()` target.
#[internal_provider(constraint_value_info_creator)]
#[derive(
    Clone,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(C)]
pub struct ConstraintValueInfo<'v> {
    setting: ValueOf<'v, ConstraintSettingInfo<'v>>,
    label: ValueOf<'v, StarlarkProvidersLabel>,
}

impl<'v> ConstraintValueInfo<'v> {
    pub(crate) fn setting(&self) -> ValueOf<'v, ConstraintSettingInfo<'v>> {
        self.setting
    }

    pub(crate) fn label(&self) -> ValueOf<'v, StarlarkProvidersLabel> {
        self.label
    }

    /// Convert to a ConstraintValue for use in configuration data.
    fn to_constraint_value(&self) -> ConstraintValue {
        ConstraintValue(self.label().label().dupe())
    }

    /// Get the ConstraintKey and ConstraintValue pair for use in configuration data.
    pub fn to_constraint_key_value(&self) -> (ConstraintKey, ConstraintValue) {
        let constraint_key = self.setting().to_constraint_key();
        let constraint_value = self.to_constraint_value();
        (constraint_key, constraint_value)
    }

    pub(crate) fn new(
        setting: ValueOf<'v, ConstraintSettingInfo<'v>>,
        label: ValueOf<'v, StarlarkProvidersLabel>,
    ) -> ConstraintValueInfo<'v> {
        ConstraintValueInfo { setting, label }
    }

    pub(crate) fn default_from_constraint_setting(
        setting: ValueOf<'v, ConstraintSettingInfo<'v>>,
    ) -> Option<ConstraintValueInfo<'v>> {
        setting
            .default()
            .map(|default| ConstraintValueInfo::new(setting, default))
    }
}

#[starlark_module]
fn constraint_value_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = ConstraintValueInfo<'static>)]
    fn ConstraintValueInfo<'v>(
        #[starlark(require = named)] setting: ValueOf<'v, ConstraintSettingInfo<'v>>,
        #[starlark(require = named)] label: LabelArg<'v>,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueTyped<'v, ConstraintValueInfo<'v>>> {
        let label = ValueOf::alloc(heap, label.to_provider_label());
        Ok(heap.alloc_representation(ConstraintValueInfo::new(setting, label)))
    }
}
