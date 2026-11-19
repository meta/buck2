/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use allocative::Allocative;
use buck2_build_api::interpreter::rule_defs::cmd_args::value::CommandLineArg;
use buck2_build_api::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use buck2_build_api::interpreter::rule_defs::provider::collection::ProviderCollection;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use starlark::environment::Module;
use starlark::values::FrozenValueTyped;
use starlark::values::Heap;

/// Result of query evaluation from queries referenced in target nodes.
///
/// Queries are:
/// * `attrs.query()` queries
/// * macro queries like `$(query_outputs ...)`
#[derive(Allocative)]
pub struct AnalysisQueryResult {
    // TODO(nga): we perform analysis even when providers are not needed,
    //   for example, `$(query_targets ...)` only needs target labels.
    pub result: Vec<(ConfiguredTargetLabel, FrozenProviderCollectionValue)>,
}

/// The context for attribute resolution. Provides access to the providers from
/// dependents.
pub trait AttrResolutionContext<'v> {
    fn starlark_module(&self) -> &Module<'v>;

    fn heap(&self) -> Heap<'v> {
        self.starlark_module().heap()
    }

    /// Get the `ProviderCollection` for this label. This is converted to a `Dependency`
    /// by the `resolve()` method in `attrs::label`
    fn get_dep(
        &mut self,
        target: &ConfiguredProvidersLabel,
    ) -> buck2_error::Result<FrozenValueTyped<'v, ProviderCollection<'v>>>;

    fn resolve_unkeyed_placeholder(
        &mut self,
        name: &str,
    ) -> buck2_error::Result<Option<CommandLineArg<'v>>>;

    /// Provides the result of the query. This will only provide results for queries that are reported during the configured attr traversal.
    // TODO(cjhopman): Ideally, we wouldn't need to split query attr resolution in this way, but processing queries is an async operation and the starlark Heap cannot be used in async code.
    fn resolve_query(&mut self, query: &str) -> buck2_error::Result<Arc<AnalysisQueryResult>>;

    fn execution_platform_resolution(&self) -> &ExecutionPlatformResolution;
}
