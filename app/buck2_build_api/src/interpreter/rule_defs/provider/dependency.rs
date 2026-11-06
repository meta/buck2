/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Display;
use std::hash::Hash;

use allocative::Allocative;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProviderName;
use buck2_error::BuckErrorContext;
use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_complex_value;
use starlark::typing::Ty;
use starlark::values::Freeze;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;
use starlark_map::StarlarkHasher;

use crate::interpreter::rule_defs::provider::ValueAsProviderLike;
use crate::interpreter::rule_defs::provider::collection::ProviderCollection;
use crate::interpreter::rule_defs::provider::execution_platform::StarlarkExecutionPlatformResolution;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum DependencyError {
    #[error("Unknown subtarget, could not find `{0}`")]
    UnknownSubtarget(String),
}

/// Wraps a dependency's `ProvidersLabel` and the result of analysis together for users' rule implementation functions
///
/// From Starlark, the label is accessible with `.label`, and providers from the underlying
/// `ProviderCollection` are available via `[]` (`get()`)
#[derive(
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[repr(C)]
pub struct Dependency<'v> {
    label: ValueTyped<'v, StarlarkConfiguredProvidersLabel>,
    provider_collection: ValueTyped<'v, ProviderCollection<'v>>,
    execution_platform: Option<ValueTyped<'v, StarlarkExecutionPlatformResolution>>,
}

starlark_complex_value!(pub Dependency);

impl<'v> Display for Dependency<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<dependency ")?;
        Display::fmt(&self.label, f)?;
        write!(f, ">")
    }
}

impl<'v> Dependency<'v> {
    pub fn label(&self) -> &'v StarlarkConfiguredProvidersLabel {
        self.label.as_ref()
    }

    fn collection(&self) -> &ProviderCollection<'v> {
        self.provider_collection.as_ref()
    }

    pub fn new(
        heap: Heap<'v>,
        label: ConfiguredProvidersLabel,
        provider_collection: ValueTyped<'v, ProviderCollection<'v>>,
        execution_platform: Option<&ExecutionPlatformResolution>,
    ) -> Self {
        Dependency {
            label: heap.alloc_typed(StarlarkConfiguredProvidersLabel::new(label)),
            provider_collection,
            execution_platform: execution_platform
                .map(|e| heap.alloc_typed(StarlarkExecutionPlatformResolution(e.clone()))),
        }
    }

    pub fn execution_platform(&self) -> Option<&'v ExecutionPlatformResolution> {
        self.execution_platform.map(|e| &e.as_ref().0)
    }
}

starlark::methods_static!(DEPENDENCY_METHODS = dependency_methods);

#[starlark_value(type = "Dependency")]
impl<'v> StarlarkValue<'v> for Dependency<'v> {
    fn get_type_starlark_repr() -> Ty {
        Ty::starlark_value::<Dependency>()
    }

    fn get_methods() -> Option<&'static Methods> {
        Some(DEPENDENCY_METHODS.methods())
    }

    fn at(&self, index: Value<'v>, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        self.provider_collection
            .to_value()
            .at(index, heap)
            .with_buck_error_context(|| format!("Error accessing dependencies of `{}`", self.label))
            .map_err(Into::into)
    }

    fn is_in(&self, other: Value<'v>) -> starlark::Result<bool> {
        self.provider_collection.to_value().is_in(other)
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        let other = match Dependency::from_value(other) {
            Some(other) => other.label(),
            None => return Ok(false),
        };
        Ok(self.label().inner() == other.inner())
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()> {
        self.label().inner().hash(hasher);
        Ok(())
    }
}

/// Represents a dependency in a build rule. When you declare a dependency attribute using
/// `attrs.dep()` in your rule definition, accessing that attribute gives you a Dependency object
/// that provides access to the dependency's providers and metadata.
///
/// Key operations:
/// - Index with `dep[ProviderType]` to access a provider (errors if absent)
/// - Use `dep.get(ProviderType)` to optionally access a provider (returns None if absent)
/// - Access the dependency's label with `dep.label`
/// - Get subtargets with `dep.sub_target("name")`
///
/// Example usage in a rule:
/// ```python
/// my_library = rule(
///     impl = my_library_impl,
///     attrs = {
///         "deps": attrs.list(attrs.dep()),
///     },
/// )
///
/// def my_library_impl(ctx):
///     # Iterate over dependencies
///     for dep in ctx.attrs.deps:
///         # Access providers
///         if dep.get(CxxLibraryInfo):
///             libs = dep[CxxLibraryInfo].libraries
///
///         # Access outputs
///         outputs = dep[DefaultInfo].default_outputs
///
///         # Get the label
///         dep_target = dep.label.raw_target()
/// ```
#[starlark_module]
fn dependency_methods(builder: &mut MethodsBuilder) {
    /// The label of this dependency.
    #[starlark(attribute)]
    fn label<'v>(
        this: &Dependency<'v>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkConfiguredProvidersLabel>> {
        Ok(this.label)
    }

    /// Returns a list of all providers available from this dependency.
    // TODO(nga): should return provider collection.
    #[starlark(attribute)]
    fn providers<'v>(this: &Dependency<'v>) -> starlark::Result<Vec<Value<'v>>> {
        Ok(this
            .collection()
            .iter_providers()
            .map(|entry| entry.map(|(_, v)| v))
            .collect::<buck2_error::Result<Vec<_>>>()?)
    }

    /// Returns a `Dependency` object of the subtarget of this target.
    ///
    /// In most cases, you can also use `dep[DefaultInfo].sub_targets["foo"]` to access subtarget
    /// providers directly. This method is useful when you need a real `Dependency` object, such
    /// as when passing to `ctx.actions.anon_target()`.
    ///
    /// Example:
    /// ```python
    /// def _impl(ctx):
    ///     for dep in ctx.attrs.deps:
    ///         # Get the dependency for a subtarget named "shared"
    ///         shared_dep = dep.sub_target("shared")
    ///         # Now shared_dep is a Dependency you can pass to other APIs
    ///         # that require a Dependency object
    ///         ctx.actions.anon_target(my_rule, {"dep": shared_dep})
    /// ```
    fn sub_target<'v>(
        this: &Dependency<'v>,
        #[starlark(require = pos)] subtarget: &str,
        heap: Heap<'v>,
    ) -> starlark::Result<Dependency<'v>> {
        let providers = this
            .collection()
            .default_info()?
            .get_sub_target_providers(subtarget)?
            .ok_or_else(|| {
                buck2_error::Error::from(DependencyError::UnknownSubtarget(subtarget.to_owned()))
            })?;
        let lbl = this.label.as_ref().inner();
        let lbl = ConfiguredProvidersLabel::new(
            lbl.target().dupe(),
            lbl.name().push(ProviderName::new(subtarget.to_owned())?),
        );
        Ok(Dependency::new(heap, lbl, providers, None))
    }

    /// Gets a specific provider from this dependency by provider type. Returns None if the
    /// provider is not present. This is the same as using indexing syntax `dep[ProviderType]`,
    /// but returns None instead of raising an error when the provider is absent.
    ///
    /// Example:
    /// ```python
    /// FooInfo = provider(fields=["bar"])
    ///
    /// def _impl(ctx):
    ///     for dep in ctx.attrs.deps:
    ///         # Try to get FooInfo provider, returns None if absent
    ///         foo_info = dep.get(FooInfo)
    ///         if foo_info:
    ///             # Provider exists, use it
    ///             value = foo_info.bar
    ///         else:
    ///             # Provider not available from this dependency
    ///             pass
    ///
    ///         # Compare with indexing (raises error if absent):
    ///         # foo_info = dep[FooInfo]  # Errors if FooInfo not provided
    /// ```
    fn get<'v>(
        this: &Dependency<'v>,
        index: Value<'v>,
    ) -> starlark::Result<NoneOr<ValueOf<'v, ValueAsProviderLike<'v>>>> {
        Ok(this.collection().get(index).with_buck_error_context(|| {
            format!("Error accessing dependencies of `{}`", this.label)
        })?)
    }
}

#[starlark_module]
#[starlark_types(
    Dependency<'static> as Dependency
)]
pub(crate) fn register_dependency(globals: &mut GlobalsBuilder) {}
