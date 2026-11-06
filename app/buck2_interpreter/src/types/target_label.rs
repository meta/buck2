/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;

use allocative::Allocative;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::NonDefaultProvidersName;
use buck2_core::provider::label::ProviderName;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::provider::label::ProvidersName;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::BuckErrorContext;
use derive_more::Display;
use derive_more::From;
use dupe::Dupe;
use serde::Serialize;
use starlark::any::ProvidesStaticType;
use starlark::collections::StarlarkHasher;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::Freeze;
use starlark::values::Heap;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueError;
use starlark::values::ValueOf;
use starlark::values::list::ListType;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::types::cell_path::StarlarkCellPath;
use crate::types::configuration::StarlarkConfiguration;
use crate::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use crate::types::configured_providers_label::StarlarkProvidersLabel;
use crate::types::package_path::StarlarkPackagePath;

#[derive(
    Clone,
    Dupe,
    Debug,
    Hash,
    Display,
    PartialEq,
    Eq,
    From,
    Freeze,
    ProvidesStaticType,
    Serialize,
    Allocative,
    StarlarkPagable
)]
#[serde(transparent)]
pub struct StarlarkTargetLabel {
    #[freeze(identity)]
    #[starlark_pagable(pagable)]
    label: TargetLabel,
}

starlark_simple_value!(StarlarkTargetLabel);

impl StarlarkTargetLabel {
    pub fn label(&self) -> &TargetLabel {
        &self.label
    }

    pub fn new(label: TargetLabel) -> Self {
        StarlarkTargetLabel { label }
    }
}

starlark::methods_static!(TARGET_LABEL_METHODS = label_methods);

#[starlark_value(type = "TargetLabel")]
impl<'v> StarlarkValue<'v> for StarlarkTargetLabel {
    fn get_methods() -> Option<&'static Methods> {
        Some(TARGET_LABEL_METHODS.methods())
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()> {
        self.hash(hasher);
        Ok(())
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        if let Some(other) = other.downcast_ref::<Self>() {
            Ok(self.label == other.label)
        } else {
            Ok(false)
        }
    }

    fn compare(&self, other: Value<'v>) -> starlark::Result<std::cmp::Ordering> {
        if let Some(other) = other.downcast_ref::<Self>() {
            Ok(self.label.cmp(&other.label))
        } else {
            ValueError::unsupported_with(self, "compare", other)
        }
    }
}

#[starlark_module]
fn label_methods(builder: &mut MethodsBuilder) {
    #[starlark(attribute)]
    fn package<'v>(
        this: &StarlarkTargetLabel,
        heap: Heap<'v>,
    ) -> starlark::Result<StringValue<'v>> {
        Ok(heap.alloc_str_intern(this.label.pkg().cell_relative_path().as_str()))
    }

    #[starlark(attribute)]
    fn name<'v>(this: &'v StarlarkTargetLabel) -> starlark::Result<&'v str> {
        Ok(this.label.name().as_str())
    }

    #[starlark(attribute)]
    fn cell<'v>(this: &'v StarlarkTargetLabel) -> starlark::Result<&'v str> {
        Ok(this.label.pkg().cell_name().as_str())
    }

    #[starlark(attribute)]
    fn path<'v>(this: &StarlarkTargetLabel) -> starlark::Result<StarlarkCellPath> {
        Ok(StarlarkCellPath(this.label.pkg().to_cell_path()))
    }

    /// Returns the PackagePath for this target label.
    #[starlark(attribute)]
    fn package_path<'v>(this: &StarlarkTargetLabel) -> starlark::Result<StarlarkPackagePath> {
        Ok(StarlarkPackagePath::new(this.label.pkg().dupe()))
    }

    /// Converts a `TargetLabel` into its corresponding `ProvidersLabel` given the subtarget names,
    /// which is a list for each layer of subtarget
    ///
    /// Sample usage:
    /// ```text
    /// def _impl_sub_target(ctx):
    ///     owners = ctx.uquery().owner("bin/TARGETS.fixture")
    ///     for owner in owners:
    ///         unconfigured_label = owner.label
    ///         ctx.output.print(unconfigured_label.with_sub_target())
    ///         ctx.output.print(unconfigured_label.with_sub_target("subtarget1"))
    ///         ctx.output.print(unconfigured_label.with_sub_target(["subtarget1", "subtarget2"]))
    /// ```
    fn with_sub_target<'v>(
        this: &StarlarkTargetLabel,
        // TODO(nga): must be either positional or named.
        #[starlark(default = SubtargetNameArg::List(ValueOf::empty_list()))]
        subtarget_name: SubtargetNameArg<'v>,
    ) -> starlark::Result<StarlarkProvidersLabel> {
        let providers_name = value_to_providers_name(subtarget_name)?;

        Ok(StarlarkProvidersLabel::new(ProvidersLabel::new(
            this.label().dupe(),
            providers_name,
        )))
    }
}

#[derive(
    Clone,
    Dupe,
    Debug,
    Hash,
    Display,
    PartialEq,
    Eq,
    From,
    ProvidesStaticType,
    Serialize,
    Allocative,
    StarlarkPagable
)]
#[serde(transparent)]
pub struct StarlarkConfiguredTargetLabel {
    #[starlark_pagable(pagable)]
    label: ConfiguredTargetLabel,
}

starlark_simple_value!(StarlarkConfiguredTargetLabel);

impl StarlarkConfiguredTargetLabel {
    pub fn label(&self) -> &ConfiguredTargetLabel {
        &self.label
    }

    pub fn new(label: ConfiguredTargetLabel) -> Self {
        StarlarkConfiguredTargetLabel { label }
    }
}

starlark::methods_static!(CONFIGURED_TARGET_LABEL_METHODS = configured_label_methods);

#[starlark_value(type = "ConfiguredTargetLabel")]
impl<'v> StarlarkValue<'v> for StarlarkConfiguredTargetLabel {
    fn get_methods() -> Option<&'static Methods> {
        Some(CONFIGURED_TARGET_LABEL_METHODS.methods())
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()> {
        self.hash(hasher);
        Ok(())
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        if let Some(other) = other.downcast_ref::<Self>() {
            Ok(self.label == other.label)
        } else {
            Ok(false)
        }
    }

    fn compare(&self, other: Value<'v>) -> starlark::Result<std::cmp::Ordering> {
        if let Some(other) = other.downcast_ref::<Self>() {
            Ok(self.label.cmp(&other.label))
        } else {
            ValueError::unsupported_with(self, "compare", other)
        }
    }
}

#[starlark_module]
fn configured_label_methods(builder: &mut MethodsBuilder) {
    #[starlark(attribute)]
    fn package<'v>(
        this: &StarlarkConfiguredTargetLabel,
        heap: Heap<'v>,
    ) -> starlark::Result<StringValue<'v>> {
        Ok(heap.alloc_str_intern(this.label.pkg().cell_relative_path().as_str()))
    }

    #[starlark(attribute)]
    fn name<'v>(this: &'v StarlarkConfiguredTargetLabel) -> starlark::Result<&'v str> {
        Ok(this.label.name().as_str())
    }

    #[starlark(attribute)]
    fn cell<'v>(this: &'v StarlarkConfiguredTargetLabel) -> starlark::Result<&'v str> {
        Ok(this.label.pkg().cell_name().as_str())
    }

    #[starlark(attribute)]
    fn path<'v>(this: &StarlarkConfiguredTargetLabel) -> starlark::Result<StarlarkCellPath> {
        Ok(StarlarkCellPath(this.label.pkg().to_cell_path()))
    }

    /// Returns the PackagePath for this configured target label.
    #[starlark(attribute)]
    fn package_path<'v>(
        this: &StarlarkConfiguredTargetLabel,
    ) -> starlark::Result<StarlarkPackagePath> {
        Ok(StarlarkPackagePath::new(this.label.pkg().dupe()))
    }

    fn config<'v>(this: &StarlarkConfiguredTargetLabel) -> starlark::Result<StarlarkConfiguration> {
        Ok(StarlarkConfiguration((this.label.cfg()).dupe()))
    }

    /// Returns the unconfigured underlying target label.
    fn raw_target(this: &StarlarkConfiguredTargetLabel) -> starlark::Result<StarlarkTargetLabel> {
        Ok(StarlarkTargetLabel::new(
            (*this.label.unconfigured()).dupe(),
        ))
    }

    /// Converts a `ConfiguredTargetLabel` into its corresponding `Label` given the subtarget name
    /// which is a list for each layer of subtarget
    ///
    /// Sample usage:
    /// ```text
    /// def _impl_sub_target(ctx):
    ///     owners = ctx.cquery().owner("bin/TARGETS.fixture")
    ///     for owner in owners:
    ///         configured_label = owner.label
    ///         ctx.output.print(configured_label.with_sub_target())
    ///         ctx.output.print(configured_label.with_sub_target("subtarget1"))
    ///         ctx.output.print(configured_label.with_sub_target(["subtarget1", "subtarget2"]))
    /// ```
    fn with_sub_target<'v>(
        this: &'v StarlarkConfiguredTargetLabel,
        // TODO(nga): must be either positional or named.
        #[starlark(default = SubtargetNameArg::List(ValueOf::empty_list()))]
        subtarget_name: SubtargetNameArg<'v>,
    ) -> starlark::Result<StarlarkConfiguredProvidersLabel> {
        let providers_name = value_to_providers_name(subtarget_name)?;

        Ok(StarlarkConfiguredProvidersLabel::new(
            ConfiguredProvidersLabel::new(this.label().dupe(), providers_name),
        ))
    }
}

#[derive(StarlarkTypeRepr, UnpackValue)]
enum SubtargetNameArg<'v> {
    List(ValueOf<'v, ListType<String>>),
    Str(&'v str),
}

fn value_to_providers_name(subtarget_name: SubtargetNameArg) -> buck2_error::Result<ProvidersName> {
    let subtarget = match subtarget_name {
        SubtargetNameArg::List(list) => list
            .try_items()
            .map(|name| {
                ProviderName::new(name?).buck_error_context("for parameter `subtarget_name`")
            })
            .collect::<buck2_error::Result<Vec<_>>>()?,
        SubtargetNameArg::Str(str) => {
            vec![
                ProviderName::new(str.to_owned())
                    .buck_error_context("for parameter `subtarget_name`")?,
            ]
        }
    };

    Ok(if subtarget.is_empty() {
        ProvidersName::Default
    } else {
        ProvidersName::NonDefault(triomphe::Arc::new(NonDefaultProvidersName::Named(
            buck2_util::arc_str::ArcSlice::from_iter(subtarget),
        )))
    })
}

#[derive(StarlarkTypeRepr, UnpackValue)]
pub enum LabelArg<'v> {
    Target(&'v StarlarkTargetLabel),
    Providers(&'v StarlarkProvidersLabel),
}

impl<'v> LabelArg<'v> {
    pub fn to_provider_label(&self) -> StarlarkProvidersLabel {
        match self {
            LabelArg::Target(t) => StarlarkProvidersLabel::new(ProvidersLabel::new(
                t.label().dupe(),
                ProvidersName::Default,
            )),
            LabelArg::Providers(t) => StarlarkProvidersLabel::new(t.label().dupe()),
        }
    }
}

#[starlark_module]
#[starlark_types(
    StarlarkTargetLabel as TargetLabel,
    StarlarkConfiguredTargetLabel as ConfiguredTargetLabel
)]
pub fn register_target_label(globals: &mut GlobalsBuilder) {}
