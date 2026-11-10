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

use allocative::Allocative;
use buck2_build_api::analysis::ProvidersLookup;
use buck2_build_api::interpreter::rule_defs::provider::collection::ProviderCollection;
use buck2_build_api::interpreter::rule_defs::provider::dependency::Dependency;
use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::FrozenValueTyped;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::starlark_value;

#[derive(
    ProvidesStaticType,
    Debug,
    NoSerialize,
    Allocative,
    starlark::values::StarlarkPagable
)]
pub(crate) struct StarlarkAnalysisResult {
    lookup: ProvidersLookup,
}

impl fmt::Display for StarlarkAnalysisResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AnalysisResult(")?;
        fmt::Display::fmt(self.lookup.providers().provider_collection(), f)?;
        write!(f, ")")
    }
}

impl StarlarkAnalysisResult {
    pub(crate) fn new(lookup: ProvidersLookup) -> Self {
        Self { lookup }
    }
}

starlark_simple_value!(StarlarkAnalysisResult);

starlark::methods_static!(STARLARK_ANALYSIS_RESULT_METHODS = starlark_analysis_result_methods);

#[starlark_value(type = "bxl.AnalysisResult")]
impl<'v> StarlarkValue<'v> for StarlarkAnalysisResult {
    fn get_methods() -> Option<&'static Methods> {
        Some(STARLARK_ANALYSIS_RESULT_METHODS.methods())
    }
}

/// The result of running an analysis in bxl.
#[starlark_module]
fn starlark_analysis_result_methods(builder: &mut MethodsBuilder) {
    /// Access the providers of the rule. Returns a `ProviderCollection` the same as accessing
    /// providers of dependencies within a rule implementation.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_providers(ctx):
    ///     node = ctx.configured_targets("root//bin:the_binary")
    ///     providers = ctx.analysis(node).providers()
    ///     ctx.output.print(providers[FooInfo])
    ///     providers = ctx.analysis("//:bin").providers()
    ///     ctx.output.print(providers[FooInfo])
    /// ```
    fn providers<'v>(
        this: &'v StarlarkAnalysisResult,
        heap: Heap<'v>,
    ) -> starlark::Result<FrozenValueTyped<'v, ProviderCollection<'v>>> {
        Ok(this.lookup.providers().add_heap_ref(heap))
    }

    /// Returns a list of structs describing each provider in the collection.
    /// Each struct has three fields:
    /// - `name`: The provider type name (e.g., `"DefaultInfo"`, `"FooInfo"`)
    /// - `path`: The cell path of the `.bzl` file where the provider was defined
    ///   (e.g., `"fbcode//pkg/defs.bzl"`), or `None` for built-in providers
    /// - `value`: The provider instance itself
    ///
    /// This enables BXL scripts to enumerate all providers on a target without
    /// knowing the provider types in advance.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_providers_info(ctx):
    ///     node = ctx.configured_targets("root//bin:the_binary")
    ///     result = ctx.analysis(node)
    ///     for info in result.providers_info():
    ///         ctx.output.print(info.name)
    ///         ctx.output.print(info.path)
    ///         ctx.output.print(info.value)
    /// ```
    fn providers_info<'v>(
        this: &'v StarlarkAnalysisResult,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        let heap = eval.heap();
        let collection = this.lookup.providers().add_heap_ref(heap);
        let mut result = Vec::new();
        for entry in collection.as_ref().iter_providers() {
            let (id, value) = entry?;
            let name = heap.alloc(id.name.as_str());
            let path = match &id.path {
                Some(p) => heap.alloc(p.to_string()),
                None => Value::new_none(),
            };
            let info = heap.alloc(starlark::values::structs::AllocStruct([
                ("name", name),
                ("path", path),
                ("value", value),
            ]));
            result.push(info);
        }
        Ok(heap.alloc(result))
    }

    /// Gets the configured providers label for this analysis result.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_label(ctx):
    ///     actions = ctx.aquery().all_actions("//target")
    ///     for node in actions:
    ///         analysis = node.analysis()
    ///         if analysis:
    ///             ctx.output.print(analysis.label())
    /// ```
    fn label<'v>(
        this: &StarlarkAnalysisResult,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkConfiguredProvidersLabel>> {
        Ok(ValueOf::alloc(
            heap,
            StarlarkConfiguredProvidersLabel::new(this.lookup.label().dupe()),
        ))
    }

    /// Converts the analysis result into a `Dependency`. Currently, you can only get a `Dependency` without any
    /// transitions. This means that you cannot create an exec dep or toolchain from an analysis result.
    ///
    /// We may support other dependency transition types in the future.
    ///
    /// This is useful for passing in the results of `ctx.analysis()` into anon targets.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_dependency(ctx):
    ///     node = ctx.configured_targets("root//bin:the_binary")
    ///     dependency = ctx.analysis(node).as_dependency()
    /// ```
    fn as_dependency<'v>(
        this: &'v StarlarkAnalysisResult,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, Dependency<'v>>> {
        Ok(eval.heap().alloc_representation(Dependency::new(
            eval.heap(),
            this.lookup.label().dupe(),
            this.lookup
                .providers()
                .add_heap_ref(eval.heap())
                .to_value_typed(),
            None,
        )))
    }
}
