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
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_dynamic_actions::dynamic_value::StarlarkDynamicValue;
use derivative::Derivative;
use derive_more::Display;
use dupe::Dupe;
use lazy_cquery_ctx::StarlarkLazyCqueryCtx;
use operation::StarlarkLazy;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::values::AllocValue;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list::ListType;
use starlark::values::starlark_value;

use crate::bxl::starlark_defs::artifacts::ArtifactArg;
use crate::bxl::starlark_defs::artifacts::LazyBuildArtifact;
use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::lazy_ctx::lazy_uquery_ctx::StarlarkLazyUqueryCtx;
use crate::bxl::starlark_defs::providers_expr::ConfiguredProvidersLabelArg;
use crate::bxl::starlark_defs::target_list_expr::ConfiguredTargetNodeArg;
use crate::bxl::starlark_defs::target_list_expr::OwnedConfiguredTargetNodeArg;
use crate::bxl::starlark_defs::target_list_expr::OwnedTargetNodeArg;
use crate::bxl::starlark_defs::target_list_expr::TargetNodeOrTargetLabelOrStr;
use crate::bxl::value_as_starlark_target_label::ValueAsStarlarkTargetLabel;

pub(crate) mod lazy_cquery_ctx;
pub(crate) mod lazy_uquery_ctx;
pub(crate) mod operation;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum BxlBuildArtifactError {
    #[error(
        "`ctx.lazy.build_artifact()` does not accept declared artifact {0}. Use `ctx.output.ensure()` instead."
    )]
    NotSupportDeclaredArtifact(String),
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum BxlResolveDynamicValueError {
    #[error(
        "`ctx.lazy.resolve_dynamic_value()` does not accept dynamic value `{0}` \
         owned by a BXL invocation. Pass a value owned by a regular target or an \
         anonymous target instead. Anonymous targets created within BXL are also allowed."
    )]
    NotSupportBxlDynamicValue(String),
}

#[derive(
    ProvidesStaticType,
    Derivative,
    Display,
    Trace,
    NoSerialize,
    Allocative
)]
#[derivative(Debug)]
#[display("bxl.LazyContext")]
#[allocative(skip)]
pub(crate) struct StarlarkLazyCtx<'v> {
    #[derivative(Debug = "ignore")]
    ctx: ValueTyped<'v, BxlContext<'v>>,
}

impl<'v> AllocValue<'v> for StarlarkLazyCtx<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

impl<'v> StarlarkLazyCtx<'v> {
    pub(crate) fn new(ctx: ValueTyped<'v, BxlContext<'v>>) -> Self {
        Self { ctx }
    }
}

starlark::methods_static!(LAZY_CTX_METHODS = lazy_ctx_methods);

#[starlark_value(type = "bxl.LazyContext", StarlarkTypeRepr, UnpackValue)]
impl<'v> StarlarkValue<'v> for StarlarkLazyCtx<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(LAZY_CTX_METHODS.methods())
    }
}

/// Context for lazy/batch/error handling operations.
///
/// Available as [`bxl.Context.lazy`](../Context#contextlazy).
#[starlark_module]
fn lazy_ctx_methods(builder: &mut MethodsBuilder) {
    /// Join two lazy operations into a single operation that can be evaluated.
    ///
    /// Example:
    /// ```python
    /// def _impl(ctx):
    ///     ...
    ///     joined = ctx.lazy.join(ctx.lazy.analysis(t1), ctx.lazy.analysis(t2))
    ///     (res1, res2) = joined.resolve()
    ///     ctx.output.print(res1)
    ///     ctx.output.print(res2)
    /// ```
    fn join<'v>(
        #[starlark(this)] _this: &'v StarlarkLazyCtx,
        #[starlark(require = pos)] lazy0: &'v StarlarkLazy,
        #[starlark(require = pos)] lazy1: &'v StarlarkLazy,
    ) -> starlark::Result<StarlarkLazy> {
        Ok(StarlarkLazy::new_join(lazy0.dupe(), lazy1.dupe()))
    }

    /// Join a list of lazy operations into a single operation that can be evaluated.
    /// This is useful when you want to evaluate multiple operations in parallel.
    /// Using `.catch().resolve()` can catch errors for the individual operations.
    ///
    /// Example:
    /// ```python
    /// def _impl(ctx):
    ///     ...
    ///     joined = ctx.lazy.join_all([ctx.lazy.analysis(t) for t in targets])
    ///     analysis_results = joined.resolve()
    ///     ctx.output.print(analysis_results)
    /// ```
    fn join_all<'v>(
        #[starlark(this)] _this: &'v StarlarkLazyCtx,
        #[starlark(require = pos)] operations: ValueOf<'v, ListType<&'v StarlarkLazy>>,
    ) -> starlark::Result<StarlarkLazy> {
        Ok(StarlarkLazy::new_batch(
            operations
                .try_items()
                .map(|o| starlark::Result::Ok(o?.dupe()))
                .collect::<starlark::Result<Vec<_>>>()?,
        ))
    }

    /// Resolve a `DynamicValue` to a [`ResolvedDynamicValue`](../../build/ResolvedDynamicValue) lazily.
    ///
    /// Resolving evaluates the dynamic action that produces the value, which includes building
    /// the artifacts it reads and resolving the dynamic values it depends on. Use `.catch()` to get
    /// a failure of any of these as a [`bxl.Result`](../Result) error instead of failing the script.
    ///
    /// Only dynamic values owned by a regular or anonymous target are accepted, including anonymous
    /// targets created within BXL. Dynamic values owned by the BXL invocation itself, i.e. created
    /// directly by BXL actions, are rejected.
    ///
    /// Example:
    /// ```python
    /// def _impl(ctx):
    ///     target = ctx.configured_targets("cell//path/to:target")
    ///     value = ctx.analysis(target).providers()[MyInfo].dynamic_value
    ///     result = ctx.lazy.resolve_dynamic_value(value).catch().resolve()
    ///     if result.is_ok():
    ///         metadata = result.unwrap().providers[MetadataInfo]
    ///     else:
    ///         err = result.unwrap_err()
    /// ```
    fn resolve_dynamic_value<'v>(
        #[starlark(this)] _this: &'v StarlarkLazyCtx<'v>,
        #[starlark(require = pos)] value: &'v StarlarkDynamicValue,
    ) -> starlark::Result<StarlarkLazy> {
        let value = &value.dynamic_value;
        if let BaseDeferredKey::BxlLabel(_) = value.dynamic_lambda_results_key.owner() {
            return Err(buck2_error::Error::from(
                BxlResolveDynamicValueError::NotSupportBxlDynamicValue(value.to_string()),
            )
            .into());
        }
        Ok(StarlarkLazy::new_dynamic_value(value.dupe()))
    }

    /// Analyze a target lazily. This will return a lazy operation that can be evaluated later.
    /// The target should be a ConfiguredTargetLabel, a ConfiguredProvidersLabel, or a ConfiguredTargetNode.
    ///
    /// Example:
    /// ```python
    /// def _impl(ctx):
    ///     target = ctx.configured_targets("cell//path/to:target")
    ///     # Get the analysis result without catching errors
    ///     analysis_result = ctx.lazy.analysis(target).resolve()
    ///     # Catch errors. It will return a `bxl.Result`
    ///     result = ctx.lazy.analysis(target).catch().resolve()
    ///     if result.is_ok():
    ///         analysis_result = result.unwrap()
    ///     else:
    ///         err = result.unwrap_err()
    /// ```
    fn analysis<'v>(
        #[starlark(this)] _this: &'v StarlarkLazyCtx,
        #[starlark(require = pos)] label: ConfiguredProvidersLabelArg<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        let configured_providers_label = label.configured_providers_label();
        Ok(StarlarkLazy::new_analysis(configured_providers_label))
    }

    /// Gets the configured target node for the `expr`.
    /// If given a string target pattern, it will resolve to a target set of configured target nodes.
    /// it also accepts an optional `target_platform` and an optional modifiers list which is used
    /// to resolve configurations of any unconfigured target nodes.
    /// The `target_platform` is either a string that can be parsed as a target label, or a
    /// target label.
    ///
    /// The given `expr` is either:
    ///     - a single string that is a target or a target pattern.
    ///     - a single target node or label, configured or unconfigured
    ///
    /// Note that this function does not accept `ConfiguredProviderLabel` (which is a configured provider label), since this
    /// is the label of a subtarget. You can get the underlying configured target label on the `Label`
    /// using `configured_targets()` (ex: `my_label.configured_target()`).
    ///
    /// This returns either a target set of `ConfiguredTargetNode`s if the given `expr` is a target pattern string,
    /// else a single `ConfiguredTargetNode`.
    ///
    /// When the given a target pattern (returns the target set), for the incompatible targets, it will print the warning message of these incompatible targets.
    /// Else (returns a single `ConfiguredTargetNode`), it will raise an error if incompatible when resolve. Use `Lazy.catch()` to catch the error.
    ///
    /// Example:
    /// ```python
    /// def _impl(ctx):
    ///     # returns a single `ConfiguredTargetNode`
    ///     node = ctx.lazy.configured_target_node("cell//path/to:target").resolve()
    ///
    ///     # returns a target set of `ConfiguredTargetNode`s
    ///     target_set = ctx.lazy.configured_target_node("cell//path/to:").resolve()
    /// ```
    fn configured_target_node<'v>(
        #[starlark(this)] this: &'v StarlarkLazyCtx,
        #[starlark(require = pos)] expr: ConfiguredTargetNodeArg<'v>,
        #[starlark(require = named, default = ValueAsStarlarkTargetLabel::NONE)]
        target_platform: ValueAsStarlarkTargetLabel<'v>,
        #[starlark(require = named, default = ValueOf::empty_list())] modifiers: ValueOf<
            'v,
            ListType<String>,
        >,
    ) -> starlark::Result<StarlarkLazy> {
        let global_cfg_options = this
            .ctx
            .resolve_global_cfg_options(target_platform, modifiers.try_to_vec()?);
        let owned = OwnedConfiguredTargetNodeArg::from_ref(expr);
        Ok(StarlarkLazy::new_configured_target_node(
            owned,
            Ok(global_cfg_options?),
        ))
    }

    /// Gets the unconfigured target node(s) for the `expr`
    ///
    /// The given `expr` is either:
    ///     - a single string that is a target or a target pattern.
    ///     - a single unconfigured target node or label
    ///
    /// This returns either a target set of `UnconfiguredTargetNode`s if the given `expr` is a target pattern string,
    /// else a single `UnconfiguredTargetNode`.
    fn unconfigured_target_node<'v>(
        #[starlark(this)] _this: &'v StarlarkLazyCtx,
        #[starlark(require = pos)] expr: TargetNodeOrTargetLabelOrStr<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        let expr = OwnedTargetNodeArg::from_ref(&expr);
        Ok(StarlarkLazy::new_unconfigured_target_node(expr))
    }

    /// Gets the unconfigured target nodes for the given target pattern with keep-going behavior.
    /// This method will continue processing even when errors are encountered, similar to
    /// `buck2 targets --keep-going`.
    ///
    /// Unlike `ctx.lazy.unconfigured_target_node`, this method accepts only a single string target pattern
    /// and returns a lazy operation that resolves to a tuple containing both successful results and errors,
    /// allowing you to handle failures gracefully rather than failing fast.
    ///
    /// The given `pattern` must be a string that is a valid target pattern, such as:
    ///     - `"//path/to:target"` - A specific target
    ///     - `"//path/to:"` - All targets in a package
    ///     - `"//path/to/..."` - All targets in a path
    ///
    /// This returns a lazy operation (`bxl.Lazy[(UnconfiguredTargetSet, dict[PackagePath, bxl.Error])]`) that resolves to a tuple where:
    /// - First element: A `UnconfiguredTargetSet` containing successfully loaded unconfigured target nodes
    /// - Second element: A dict mapping `PackagePath` to `Error` for packages that failed to load
    ///
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_keep_going(ctx):
    ///     lazy_result = ctx.lazy.unconfigured_target_nodes_keep_going("//my/package/...")
    ///     success_targets, error_map = lazy_result.resolve()
    ///     
    ///     # Process successful targets
    ///     for target in success_targets:
    ///         ctx.output.print(f"Successfully loaded: {target.label}")
    ///     
    ///     # Handle errors
    ///     for package_path, error in error_map.items():
    ///         ctx.output.print(f"Failed to load package {package_path}: {error}")
    /// ```
    fn unconfigured_target_nodes_keep_going<'v>(
        #[starlark(this)] _this: &'v StarlarkLazyCtx,
        #[starlark(require = pos)] pattern: &str,
    ) -> starlark::Result<StarlarkLazy> {
        Ok(StarlarkLazy::new_unconfigured_target_node_keep_going(
            pattern.to_owned(),
        ))
    }

    /// Gets the lazy uquery context.
    ///
    /// `allow_partial_graph` behaves as `buck2 uquery --allow-partial-graph`.
    ///
    /// Current limitation: package load errors while expanding recursive patterns
    /// passed directly to query methods still fail the query. For example,
    /// `deps("//foo/...")` still fails if a package in that pattern cannot load.
    /// Use `eval("deps(//foo/...)")` to tolerate those errors.
    fn uquery<'v>(
        #[starlark(this)] _this: &'v StarlarkLazyCtx,
        #[starlark(require = named, default = false)] allow_partial_graph: bool,
    ) -> starlark::Result<StarlarkLazyUqueryCtx> {
        Ok(StarlarkLazyUqueryCtx::new(allow_partial_graph))
    }

    /// Gets the lazy cquery context.
    fn cquery<'v>(
        this: &'v StarlarkLazyCtx,
        #[starlark(require = named, default = ValueAsStarlarkTargetLabel::NONE)]
        target_platform: ValueAsStarlarkTargetLabel<'v>,
        #[starlark(require = named, default = ValueOf::empty_list())] modifiers: ValueOf<
            'v,
            ListType<String>,
        >,
    ) -> starlark::Result<StarlarkLazyCqueryCtx> {
        let global_cfg_options = this
            .ctx
            .resolve_global_cfg_options(target_platform, modifiers.try_to_vec()?)?;
        Ok(StarlarkLazyCqueryCtx::new(global_cfg_options))
    }

    /// Build the given artifact, but it will not materialize the artifact. If the artifact need to be materialized,
    /// call `ctx.output.ensure` for the resolved value to defer materialization of the artifact.
    ///
    /// **Attention**: This api does not accept declared artifact. If you want to materialize a declared artifact, use `ctx.output.ensure`.
    fn build_artifact<'v>(
        #[starlark(this)] _this: &'v StarlarkLazyCtx<'v>,
        // Use `ArtifactArg` instead of `StarlarkArtifact` to avoid the confused type mismatch error "Type of parameter 'artifact' doesn't match, expected 'artifact', actual 'artifact'" when given declared artifacts.
        #[starlark(require = pos)] artifact: ArtifactArg<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        match artifact {
            ArtifactArg::DeclaredArtifact(_) => {
                return Err(buck2_error::Error::from(
                    BxlBuildArtifactError::NotSupportDeclaredArtifact(artifact.to_string()),
                )
                .into());
            }
            ArtifactArg::Artifact(artifact) => {
                let artifact = LazyBuildArtifact::new(artifact.as_ref());
                Ok(StarlarkLazy::new_build_artifact(artifact))
            }
        }
    }
}
