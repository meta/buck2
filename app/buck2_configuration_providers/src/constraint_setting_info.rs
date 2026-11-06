/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! ConstraintSettingInfo is the provider info returned from a constraint_setting() rule. Currently, all
//! it really does is indicate that the rule is actually a constraint_setting as a constraint_setting()
//! rule has no arguments.
//!
//! A constraint_value() rule will check that the referenced setting produces a ConstraintSettingInfo.

use std::fmt::Debug;

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use buck2_core::configuration::constraints::ConstraintKey;
use buck2_core::configuration::constraints::ConstraintValue;
use buck2_interpreter::types::configured_providers_label::StarlarkProvidersLabel;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::StarlarkPagable;
use starlark::values::Trace;
use starlark::values::ValueTyped;
use starlark::values::none::NoneOr;

/// Provider that signals that a target can be used as a constraint key. This is the only provider
/// returned by a `constraint_setting()` target.
#[internal_provider(constraint_info_creator)]
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
pub(crate) struct ConstraintSettingInfo<'v> {
    label: ValueTyped<'v, StarlarkTargetLabel>,
    // TODO(nero): Remove NoneOr when we migrate to unified constraint
    default: Option<ValueTyped<'v, StarlarkProvidersLabel>>,
}

impl<'v> ConstraintSettingInfo<'v> {
    pub(crate) fn label(&self) -> ValueTyped<'v, StarlarkTargetLabel> {
        self.label
    }

    pub(crate) fn default(&self) -> Option<ValueTyped<'v, StarlarkProvidersLabel>> {
        self.default
    }

    /// Convert to a ConstraintKey for use in configuration data.
    pub(crate) fn to_constraint_key(&self) -> ConstraintKey {
        let default_constraint_value = self
            .default()
            .map(|default| ConstraintValue(default.label().dupe()));
        ConstraintKey {
            key: self.label().label().dupe(),
            default: default_constraint_value,
        }
    }

    pub(crate) fn new(
        label: ValueTyped<'v, StarlarkTargetLabel>,
        default: Option<ValueTyped<'v, StarlarkProvidersLabel>>,
    ) -> ConstraintSettingInfo<'v> {
        ConstraintSettingInfo { label, default }
    }
}

#[starlark_module]
fn constraint_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = ConstraintSettingInfo<'static>)]
    fn ConstraintSettingInfo<'v>(
        #[starlark(require = named)] label: ValueTyped<'v, StarlarkTargetLabel>,
        #[starlark(require = named, default = NoneOr::None)] default: NoneOr<
            ValueTyped<'v, StarlarkProvidersLabel>,
        >,
    ) -> starlark::Result<ConstraintSettingInfo<'v>> {
        Ok(ConstraintSettingInfo::new(label, default.into_option()))
    }
}
