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
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::provider::label::ProvidersName;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serialize;
use serde::Serializer;
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
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;

use crate::types::cell_path::StarlarkCellPath;
use crate::types::cell_root::CellRoot;
use crate::types::package_path::StarlarkPackagePath;
use crate::types::project_root::StarlarkProjectRoot;
use crate::types::target_label::StarlarkConfiguredTargetLabel;
use crate::types::target_label::StarlarkTargetLabel;

impl StarlarkConfiguredProvidersLabel {
    pub fn label(&self) -> &ConfiguredProvidersLabel {
        &self.label
    }
}

/// Container for `ConfiguredProvidersLabel` that gives users access to things like package, cell, etc. This can also be properly stringified by our forthcoming `CommandLine` object
#[derive(
    Clone,
    Debug,
    Display,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[display("{}", label)]
#[repr(C)]
pub struct StarlarkConfiguredProvidersLabel {
    #[freeze(identity)]
    #[starlark_pagable(pagable)]
    label: ConfiguredProvidersLabel,
}

starlark_simple_value!(StarlarkConfiguredProvidersLabel);

impl Serialize for StarlarkConfiguredProvidersLabel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.label.serialize(serializer)
    }
}

impl StarlarkConfiguredProvidersLabel {
    pub fn new(label: ConfiguredProvidersLabel) -> Self {
        StarlarkConfiguredProvidersLabel { label }
    }

    pub fn inner(&self) -> &ConfiguredProvidersLabel {
        &self.label
    }
}

starlark::methods_static!(CONFIGURED_PROVIDERS_LABEL_METHODS = configured_label_methods);

#[starlark_value(type = "Label")]
impl<'v> StarlarkValue<'v> for StarlarkConfiguredProvidersLabel
where
    Self: ProvidesStaticType<'v>,
{
    fn get_methods() -> Option<&'static Methods> {
        Some(CONFIGURED_PROVIDERS_LABEL_METHODS.methods())
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        Ok(match StarlarkConfiguredProvidersLabel::from_value(other) {
            Some(other) => self.label == other.label,
            None => false,
        })
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()> {
        self.label.hash(hasher);
        Ok(())
    }
}

/// A label is used to represent a configured target.
#[starlark_module]
fn configured_label_methods(builder: &mut MethodsBuilder) {
    /// For the label `fbcode//buck2/hello:world (ovr_config//platform/linux:x86_64-fbcode-46b26edb4b80a905)` this gives back `buck2/hello`
    #[starlark(attribute)]
    fn package<'v>(
        this: &'v StarlarkConfiguredProvidersLabel,
        heap: Heap<'v>,
    ) -> starlark::Result<StringValue<'v>> {
        Ok(heap.alloc_str_intern(this.label.target().pkg().cell_relative_path().as_str()))
    }

    /// For the label `fbcode//buck2/hello:world (ovr_config//platform/linux:x86_64-fbcode-46b26edb4b80a905)` this gives back `world`
    #[starlark(attribute)]
    fn name<'v>(this: &'v StarlarkConfiguredProvidersLabel) -> starlark::Result<&'v str> {
        Ok(this.label.target().name().as_str())
    }

    #[starlark(attribute)]
    fn sub_target<'v>(
        this: &'v StarlarkConfiguredProvidersLabel,
    ) -> starlark::Result<NoneOr<Vec<&'v str>>> {
        Ok(match this.label.name() {
            ProvidersName::Default => NoneOr::None,
            ProvidersName::NonDefault(flavor) => match flavor.as_ref() {
                NonDefaultProvidersName::Named(names) => {
                    NoneOr::Other(names.iter().map(|p| p.as_str()).collect())
                }
                NonDefaultProvidersName::UnrecognizedFlavor(_) => {
                    unreachable!(
                        "This should have been an error when looking up the corresponding analysis (`{}`)",
                        this.label
                    )
                }
            },
        })
    }

    /// For the label `fbcode//buck2/hello:world (ovr_config//platform/linux:x86_64-fbcode-46b26edb4b80a905)` this gives back `fbcode//buck2/hello`
    #[starlark(attribute)]
    fn path<'v>(
        this: &StarlarkConfiguredProvidersLabel,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkCellPath>> {
        Ok(ValueOf::alloc(
            heap,
            StarlarkCellPath(this.label.target().pkg().to_cell_path()),
        ))
    }

    /// For the label `fbcode//buck2/hello:world (ovr_config//platform/linux:x86_64-fbcode-46b26edb4b80a905)` this gives back `fbcode`
    #[starlark(attribute)]
    fn cell<'v>(this: &'v StarlarkConfiguredProvidersLabel) -> starlark::Result<&'v str> {
        Ok(this.label.target().pkg().cell_name().as_str())
    }

    /// Returns the PackagePath for this configured providers label.
    #[starlark(attribute)]
    fn package_path<'v>(
        this: &StarlarkConfiguredProvidersLabel,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkPackagePath>> {
        Ok(ValueOf::alloc(
            heap,
            StarlarkPackagePath::new(this.label.target().pkg().dupe()),
        ))
    }

    /// Obtain a reference to this target label's cell root. This can be used as if it were an
    /// artifact in places that expect one, such as `cmd_args().relative_to`.
    #[starlark(attribute)]
    fn cell_root<'v>(
        this: &StarlarkConfiguredProvidersLabel,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, CellRoot>> {
        Ok(ValueOf::alloc(
            heap,
            CellRoot::new(this.label.target().pkg().cell_name()),
        ))
    }

    /// Obtain a reference to the project's root. This can be used as if it were an artifact in
    /// places that expect one, such as `cmd_args().relative_to`.
    #[starlark(attribute)]
    fn project_root<'v>(
        this: &StarlarkConfiguredProvidersLabel,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkProjectRoot>> {
        Ok(ValueOf::alloc(heap, StarlarkProjectRoot))
    }

    /// For the label `fbcode//buck2/hello:world (ovr_config//platform/linux:x86_64-fbcode-46b26edb4b80a905)` this returns the unconfigured underlying target label (`fbcode//buck2/hello:world`)
    fn raw_target<'v>(
        this: &StarlarkConfiguredProvidersLabel,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkTargetLabel>> {
        Ok(ValueOf::alloc(
            heap,
            StarlarkTargetLabel::new((*this.label.target().unconfigured()).dupe()),
        ))
    }

    /// Returns the underlying configured target label, dropping the sub target
    fn configured_target<'v>(
        this: &StarlarkConfiguredProvidersLabel,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkConfiguredTargetLabel>> {
        Ok(ValueOf::alloc(
            heap,
            StarlarkConfiguredTargetLabel::new((*this.label.target()).dupe()),
        ))
    }
}

impl StarlarkProvidersLabel {
    pub fn label(&self) -> &ProvidersLabel {
        &self.label
    }
}

/// Container for `ProvidersLabel` that gives users access to things like package, cell, etc.
#[derive(
    Clone,
    Debug,
    Display,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    Serialize,
    Pagable,
    StarlarkPagable
)]
#[display("{}", label)]
#[repr(C)]
#[serde(transparent)]
pub struct StarlarkProvidersLabel {
    #[freeze(identity)]
    #[starlark_pagable(pagable)]
    label: ProvidersLabel,
}

starlark_simple_value!(StarlarkProvidersLabel);

impl StarlarkProvidersLabel {
    pub fn new(label: ProvidersLabel) -> Self {
        StarlarkProvidersLabel { label }
    }
}

starlark::methods_static!(PROVIDERS_LABEL_METHODS = label_methods);

#[starlark_value(type = "ProvidersLabel")]
impl<'v> StarlarkValue<'v> for StarlarkProvidersLabel
where
    Self: ProvidesStaticType<'v>,
{
    fn get_methods() -> Option<&'static Methods> {
        Some(PROVIDERS_LABEL_METHODS.methods())
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        if let Some(other) = StarlarkProvidersLabel::from_value(other) {
            Ok(self.label == other.label)
        } else {
            Ok(false)
        }
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()> {
        self.label.hash(hasher);
        Ok(())
    }
}

#[starlark_module]
fn label_methods(builder: &mut MethodsBuilder) {
    #[starlark(attribute)]
    fn name<'v>(this: &'v StarlarkProvidersLabel) -> starlark::Result<&'v str> {
        Ok(this.label.target().name().as_str())
    }

    #[starlark(attribute)]
    fn sub_target<'v>(this: &'v StarlarkProvidersLabel) -> starlark::Result<NoneOr<Vec<&'v str>>> {
        Ok(match this.label.name() {
            ProvidersName::Default => NoneOr::None,
            ProvidersName::NonDefault(flavor) => match flavor.as_ref() {
                NonDefaultProvidersName::Named(names) => {
                    NoneOr::Other(names.iter().map(|p| p.as_str()).collect())
                }
                NonDefaultProvidersName::UnrecognizedFlavor(_) => {
                    unreachable!(
                        "This should have been an error when looking up the corresponding analysis (`{}`)",
                        this.label
                    )
                }
            },
        })
    }

    #[starlark(attribute)]
    fn path<'v>(
        this: &StarlarkProvidersLabel,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkCellPath>> {
        Ok(ValueOf::alloc(
            heap,
            StarlarkCellPath(this.label.target().pkg().to_cell_path()),
        ))
    }

    #[starlark(attribute)]
    fn cell<'v>(this: &'v StarlarkProvidersLabel) -> starlark::Result<&'v str> {
        let cell = this.label.target().pkg().cell_name().as_str();
        Ok(cell)
    }

    #[starlark(attribute)]
    fn package<'v>(
        this: &'v StarlarkProvidersLabel,
        heap: Heap<'v>,
    ) -> starlark::Result<StringValue<'v>> {
        Ok(heap.alloc_str_intern(this.label.target().pkg().cell_relative_path().as_str()))
    }

    /// Returns the PackagePath for this providers label.
    #[starlark(attribute)]
    fn package_path<'v>(
        this: &StarlarkProvidersLabel,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkPackagePath>> {
        Ok(ValueOf::alloc(
            heap,
            StarlarkPackagePath::new(this.label.target().pkg().dupe()),
        ))
    }

    /// Returns the unconfigured underlying target label.
    fn raw_target<'v>(
        this: &StarlarkProvidersLabel,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkTargetLabel>> {
        Ok(ValueOf::alloc(
            heap,
            StarlarkTargetLabel::new((*this.label.target()).dupe()),
        ))
    }
}

// TODO(nga): remove the `Label` alias. (T264813434)
#[starlark_module]
#[starlark_types(
    StarlarkConfiguredProvidersLabel as Label,
    StarlarkProvidersLabel as ProvidersLabel,
    StarlarkConfiguredProvidersLabel as ConfiguredProvidersLabel
)]
pub fn register_providers_label(globals: &mut GlobalsBuilder) {}

#[cfg(test)]
mod tests {
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::provider::label::ConfiguredProvidersLabel;
    use buck2_core::provider::label::NonDefaultProvidersName;
    use buck2_core::provider::label::ProviderName;
    use buck2_core::provider::label::ProvidersLabel;
    use buck2_core::provider::label::ProvidersName;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
    use buck2_core::target::label::label::TargetLabel;
    use buck2_util::arc_str::ArcSlice;
    use starlark::assert::Assert;
    use starlark::environment::GlobalsBuilder;
    use starlark::starlark_module;
    use starlark::values::Heap;
    use starlark::values::ValueOf;

    use crate::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
    use crate::types::configured_providers_label::StarlarkProvidersLabel;

    #[starlark_module]
    fn register_test_providers_label(globals: &mut GlobalsBuilder) {
        fn configured_providers_label<'v>(
            heap: Heap<'v>,
        ) -> starlark::Result<ValueOf<'v, StarlarkConfiguredProvidersLabel>> {
            let label = StarlarkConfiguredProvidersLabel {
                label: ConfiguredProvidersLabel::new(
                    ConfiguredTargetLabel::testing_parse(
                        "foo//bar:baz",
                        ConfigurationData::testing_new(),
                    ),
                    ProvidersName::NonDefault(triomphe::Arc::new(NonDefaultProvidersName::Named(
                        ArcSlice::new([
                            ProviderName::new("qux".to_owned())?,
                            ProviderName::new("quux".to_owned())?,
                        ]),
                    ))),
                ),
            };
            Ok(ValueOf::alloc(heap, label))
        }

        fn providers_label<'v>(
            heap: Heap<'v>,
        ) -> starlark::Result<ValueOf<'v, StarlarkProvidersLabel>> {
            let label = StarlarkProvidersLabel {
                label: ProvidersLabel::new(
                    TargetLabel::testing_parse("foo//bar:baz"),
                    ProvidersName::NonDefault(triomphe::Arc::new(NonDefaultProvidersName::Named(
                        ArcSlice::new([
                            ProviderName::new("qux".to_owned())?,
                            ProviderName::new("quux".to_owned())?,
                        ]),
                    ))),
                ),
            };
            Ok(ValueOf::alloc(heap, label))
        }
    }

    #[test]
    fn test_configured_providers_label_to_json() {
        let mut a = Assert::new();
        a.globals_add(register_test_providers_label);
        a.eq(
            &"'\"foo//bar:baz[qux][quux] (<CFG>)\"'"
                .replace("<CFG>", &ConfigurationData::testing_new().to_string()),
            "json.encode(configured_providers_label())",
        );
    }

    #[test]
    fn test_providers_label_to_json() {
        let mut a = Assert::new();
        a.globals_add(register_test_providers_label);
        a.eq(
            "'\"foo//bar:baz[qux][quux]\"'",
            "json.encode(providers_label())",
        );
    }
}
