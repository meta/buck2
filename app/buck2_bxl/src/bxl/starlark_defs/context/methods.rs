/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::iter;
use std::sync::Arc;

use buck2_build_api::interpreter::rule_defs::context::AnalysisActions;
use buck2_cli_proto::build_request::Materializations;
use buck2_cli_proto::build_request::Uploads;
use buck2_common::events::HasEvents;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::paths::CellRelativePath;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::buck2_error;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::starlark_promise::StarlarkPromise;
use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use buck2_interpreter::types::configured_providers_label::StarlarkProvidersLabel;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use buck2_node::configuration::calculation::CellNameForConfigurationResolution;
use buck2_node::configuration::resolved::ConfigurationSettingKey;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::frontend::TargetGraphCalculation;
use buck2_node::nodes::unconfigured::TargetNode;
use dupe::Dupe;
use either::Either;
use futures::FutureExt;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list::ListType;
use starlark::values::none::NoneOr;
use starlark::values::none::NoneType;
use starlark::values::structs::StructRef;

use crate::bxl::starlark_defs::analysis_result::StarlarkAnalysisResult;
use crate::bxl::starlark_defs::aquery::StarlarkAQueryCtx;
use crate::bxl::starlark_defs::audit::StarlarkAuditCtx;
use crate::bxl::starlark_defs::build_result::StarlarkBxlBuildResult;
use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::context::BxlContextError;
use crate::bxl::starlark_defs::context::BxlContextType;
use crate::bxl::starlark_defs::context::NotATargetLabelString;
use crate::bxl::starlark_defs::context::actions::BxlActions;
use crate::bxl::starlark_defs::context::actions::resolve_bxl_execution_platform;
use crate::bxl::starlark_defs::context::actions::validate_action_instantiation;
use crate::bxl::starlark_defs::context::analysis;
use crate::bxl::starlark_defs::context::anon_target::run_anon_target_promises;
use crate::bxl::starlark_defs::context::build;
use crate::bxl::starlark_defs::context::fs::BxlFilesystem;
use crate::bxl::starlark_defs::context::output::StarlarkOutputStream;
use crate::bxl::starlark_defs::cquery::StarlarkCQueryCtx;
use crate::bxl::starlark_defs::event::StarlarkUserEventParser;
use crate::bxl::starlark_defs::lazy_ctx::StarlarkLazyCtx;
use crate::bxl::starlark_defs::nodes::configured::StarlarkConfiguredTargetNode;
use crate::bxl::starlark_defs::nodes::unconfigured::StarlarkTargetNode;
use crate::bxl::starlark_defs::providers_expr::AnyProvidersExprArg;
use crate::bxl::starlark_defs::providers_expr::ConfiguredProvidersExprArg;
use crate::bxl::starlark_defs::providers_expr::ProvidersExpr;
use crate::bxl::starlark_defs::providers_expr::ProvidersExprArg;
use crate::bxl::starlark_defs::target_list_expr::ConfiguredTargetListExprArg;
use crate::bxl::starlark_defs::target_list_expr::TargetListExpr;
use crate::bxl::starlark_defs::target_list_expr::TargetListExprArg;
use crate::bxl::starlark_defs::target_list_expr::filter_incompatible;
use crate::bxl::starlark_defs::target_universe::StarlarkTargetUniverse;
use crate::bxl::starlark_defs::targetset::StarlarkTargetSet;
use crate::bxl::starlark_defs::uquery::StarlarkUQueryCtx;
use crate::bxl::value_as_starlark_target_label::ValueAsStarlarkTargetLabel;

/// The bxl context that the top level bxl implementation receives as parameter.
/// This context contains all the core bxl functions to query, build, create actions, etc.
#[starlark_module]
pub(crate) fn bxl_context_methods(builder: &mut MethodsBuilder) {
    /// Gets the output stream to the console via stdout. Items written to the output stream
    /// are considered to be the results of a bxl script, which will be displayed to stdout by
    /// buck2 even when the script is cached.
    ///
    /// Prints that are not result of the bxl should be printed via stderr via the stdlib `print`
    /// and `pprint`.
    ///
    /// This function is not available on the `bxl_ctx` when called from `dynamic_output`.
    #[starlark(attribute)]
    fn output<'v>(
        this: &'v BxlContext<'v>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkOutputStream>> {
        let output_stream = this
            .context_type
            .unpack_root()
            .map_err(|_| {
                buck2_error::Error::from(BxlContextError::Unsupported("output".to_owned()))
            })?
            .output_stream;
        Ok(output_stream)
    }

    /// Returns the absolute path to the root of the repository
    ///
    /// This function is not available on the `bxl_ctx` when called from `dynamic_output`.
    fn root<'v>(this: &'v BxlContext<'v>) -> starlark::Result<String> {
        let _root_type = this.context_type.unpack_root().map_err(|_| {
            buck2_error::Error::from(BxlContextError::Unsupported("root".to_owned()))
        })?;
        Ok(this.project_root().root().to_str()?.to_owned())
    }

    /// Returns the absolute path to the cell of the repository
    ///
    /// This function is not available on the `bxl_ctx` when called from `dynamic_output`.
    fn cell_root<'v>(this: &'v BxlContext<'v>) -> starlark::Result<String> {
        let _root_type = this.context_type.unpack_root().map_err(|_| {
            buck2_error::Error::from(BxlContextError::Unsupported("root".to_owned()))
        })?;
        Ok(this.cell_root_abs().to_owned().to_string())
    }

    /// Gets the target nodes for the `labels`, accepting an optional `target_platform` which is the
    /// target platform configuration used to resolve configurations of any unconfigured target
    /// nodes.
    /// The `target_platform` is either a string that can be parsed as a target label, or a
    /// target label.
    ///
    /// The given `labels` is a [`TargetListExpr`], which is either:
    ///     - a single string that is a `target pattern`.
    ///     - a single target node or label, configured or unconfigured
    ///     - a list of the two options above.
    ///
    /// Note that this function does not accept `Label` (which is a configured provider label), since this
    /// is the label of a subtarget. You can get the underlying configured target label on the `Label`
    /// using `configured_targets()` (ex: `my_label.configured_target()`).
    ///
    /// This returns either a single `target_node` if the given `labels`
    /// is "singular", a dict keyed by target labels of `target_node` if the
    /// given `labels` is list-like
    fn configured_targets<'v>(
        this: &'v BxlContext<'v>,
        #[starlark(require = pos)] labels: ConfiguredTargetListExprArg<'v>,
        #[starlark(default = ValueAsStarlarkTargetLabel::NONE)]
        target_platform: ValueAsStarlarkTargetLabel<'v>,
        #[starlark(require = named, default = NoneOr::None)] modifiers: NoneOr<
            ValueOf<'v, ListType<String>>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<
        Either<NoneOr<StarlarkConfiguredTargetNode>, StarlarkTargetSet<ConfiguredTargetNode>>,
    > {
        let cli_modifiers = match modifiers.into_option() {
            Some(cli_modifiers) => cli_modifiers.try_to_vec()?,
            None => Vec::new(),
        };
        let global_cfg_options = this.resolve_global_cfg_options(target_platform, cli_modifiers)?;

        Ok(this.via_dice(eval, |dice| {
            dice.via(|ctx| {
                async move {
                    let target_expr = TargetListExpr::<'v, ConfiguredTargetNode>::unpack(
                        labels,
                        &global_cfg_options,
                        this,
                        ctx,
                    )
                    .await?;

                    if let Some(one) = target_expr.get_one(ctx).await? {
                        let result = filter_incompatible(iter::once(one), this)?;
                        if let Some(node) = result.iter().next() {
                            Ok(Either::Left(NoneOr::Other(StarlarkConfiguredTargetNode(
                                node.dupe(),
                            ))))
                        } else {
                            Ok(Either::Left(NoneOr::None))
                        }
                    } else {
                        Ok(Either::Right(StarlarkTargetSet(filter_incompatible(
                            target_expr.get(ctx).await?,
                            this,
                        )?)))
                    }
                }
                .boxed_local()
            })
        })?)
    }

    /// Gets the unconfigured target nodes for the `labels`
    ///
    /// The given `labels` is either:
    ///     - a single string that is a `target pattern`.
    ///     - a single unconfigured  target node or label
    ///     - a list of the two options above.
    ///
    /// This returns either a single `UnconfiguredTargetNode` if the given `labels`
    /// is "singular", a dict keyed by target labels of `UnconfiguredTargetNode` if the
    /// given `labels` is list-like
    fn unconfigured_targets<'v>(
        this: &'v BxlContext<'v>,
        labels: TargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Either<StarlarkTargetNode, StarlarkTargetSet<TargetNode>>> {
        Ok(this.via_dice(eval, |ctx| {
            ctx.via(|ctx| {
                async move {
                    let expr = TargetListExpr::<'v, TargetNode>::unpack(labels, this, ctx).await?;
                    if let Some(one) = expr.get_one(ctx).await? {
                        Ok(Either::Left(StarlarkTargetNode(one)))
                    } else {
                        Ok(Either::Right(StarlarkTargetSet(
                            expr.get(ctx).await?.into_owned(),
                        )))
                    }
                }
                .boxed_local()
            })
        })?)
    }

    /// Gets the unconfigured subtargets for the given `labels`
    ///
    /// The given `labels` is a providers expression, which is either:
    ///     - a single string that is a `target pattern`.
    ///     - a single target node or label, configured or unconfigured
    ///     - a single subtarget label, configured or unconfigured
    ///     - a list of the two options above.
    ///
    /// This returns either a single `providers_label` if the given `labels` argument
    /// is "singular", or dict of the subtarget string representation to the
    /// `providers_label` if the given `labels` argument is list-like.
    ///
    /// Note that this function does not check that this subtarget exists in the repo.
    fn unconfigured_sub_targets<'v>(
        this: &BxlContext<'v>,
        // TODO(nga): parameter should be either positional or named, not both.
        labels: ProvidersExprArg<'v>,
    ) -> starlark::Result<
        Either<StarlarkProvidersLabel, BuckMutInsertionMap<String, StarlarkProvidersLabel>>,
    > {
        let providers = ProvidersExpr::<ProvidersLabel>::unpack(labels, this)?;

        match providers {
            ProvidersExpr::Literal(provider) => {
                Ok(Either::Left(StarlarkProvidersLabel::new(provider)))
            }
            ProvidersExpr::Iterable(providers) => Ok(Either::Right(
                providers
                    .into_iter()
                    .map(|p| (p.to_string(), StarlarkProvidersLabel::new(p)))
                    .collect(),
            )),
        }
    }

    /// Returns the `TargetUniverse` that can lookup valid configured nodes in the universe.
    ///
    /// The given `labels` is a target expression, which is either:
    ///     - a single string that is a `target pattern`.
    ///     - a single target node or label, configured or unconfigured
    ///     - a single subtarget label, configured or unconfigured
    ///     - a list of the two options above.
    ///
    /// Also takes in an optional `target_platform` param to configure the nodes with, and a `keep_going`
    /// flag to skip any loading or configuration errors.
    ///
    /// The default modifiers used to configure the target nodes are empty. If you want to use the
    /// modifiers from the cli, you can pass `ctx.modifiers` to the argument `modifiers` of this function.
    fn target_universe<'v>(
        this: ValueTyped<'v, BxlContext<'v>>,
        labels: ConfiguredTargetListExprArg<'v>,
        #[starlark(default = ValueAsStarlarkTargetLabel::NONE)]
        target_platform: ValueAsStarlarkTargetLabel<'v>,
        #[starlark(require = named, default = false)] keep_going: bool,
        #[starlark(require = named, default = ValueOf::empty_list())] modifiers: ValueOf<
            'v,
            ListType<String>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetUniverse<'v>> {
        let modifiers = modifiers.try_to_vec()?;
        let global_cfg_options = this.resolve_global_cfg_options(target_platform, modifiers)?;

        Ok(this.via_dice(eval, |ctx| {
            ctx.via(|ctx| {
                async move {
                    let target_expr = TargetListExpr::<'v, ConfiguredTargetNode>::unpack_opt(
                        labels,
                        &global_cfg_options,
                        &this,
                        ctx,
                        keep_going,
                    )
                    .await?;

                    let maybe_compatible_set = target_expr.get(ctx).await?;

                    let target_set = filter_incompatible(maybe_compatible_set, &*this)?;

                    StarlarkTargetUniverse::new(this, target_set).await
                }
                .boxed_local()
            })
        })?)
    }

    /// Returns the `uqueryctx` that holds all uquery functions.
    ///
    /// `allow_partial_graph` behaves as `buck2 uquery --allow-partial-graph`.
    ///
    /// Current limitation: package load errors while expanding recursive patterns
    /// passed directly to query methods still fail the query. For example,
    /// `deps("//foo/...")` still fails if a package in that pattern cannot load.
    /// Use `eval("deps(//foo/...)")` to tolerate those errors.
    fn uquery<'v>(
        this: ValueTyped<'v, BxlContext<'v>>,
        #[starlark(require = named, default = false)] allow_partial_graph: bool,
    ) -> starlark::Result<StarlarkUQueryCtx<'v>> {
        Ok(StarlarkUQueryCtx::new(this, allow_partial_graph)?)
    }

    /// Returns the `cqueryctx` that holds all the cquery functions.
    /// This function takes an optional parameter `target_platform`, which is the target platform
    /// configuration used to configured any unconfigured target nodes.
    ///
    /// The `target_platform` is a target label, or a string that is a target label.
    fn cquery<'v>(
        this: ValueTyped<'v, BxlContext<'v>>,
        // TODO(nga): parameter should be either positional or named, not both.
        #[starlark(default = ValueAsStarlarkTargetLabel::NONE)]
        target_platform: ValueAsStarlarkTargetLabel<'v>,
    ) -> starlark::Result<StarlarkCQueryCtx<'v>> {
        let global_cfg_options = this.resolve_global_cfg_options(target_platform, vec![])?;
        Ok(StarlarkCQueryCtx::new(this, global_cfg_options)?)
    }

    /// Returns the `aqueryctx` that holds all the aquery functions.
    /// This function takes an optional parameter `target_platform`, which is the target platform
    /// configuration used to configured any unconfigured target nodes.
    ///
    /// The `target_platform` is a target label, or a string that is a target label.
    fn aquery<'v>(
        this: ValueTyped<'v, BxlContext<'v>>,
        #[starlark(default = ValueAsStarlarkTargetLabel::NONE)]
        target_platform: ValueAsStarlarkTargetLabel<'v>,
    ) -> starlark::Result<StarlarkAQueryCtx<'v>> {
        Ok(StarlarkAQueryCtx::new(this, target_platform)?)
    }

    /// Returns the bxl actions to create and register actions for this
    /// bxl function. This will have the execution platform resolved according to the execution
    /// deps and toolchains you pass into this function.
    /// You'll be able to access the analysis action factory of the correct execution platform,
    /// toolchains, and execution deps of the corresponding configuration via this context.
    ///
    /// Actions created by bxl will not be built by default. Instead, they are marked to be built
    /// by `ctx.output.ensure(artifact)` on the output module of the `bxl_ctx`. Only artifacts
    /// marked by ensure will be built.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_write_action(ctx):
    ///     bxl_actions = ctx.bxl_actions()
    ///     output = bxl_actions.actions.write("my_output", "my_content")
    ///     ensured = ctx.output.ensure(output)
    ///     ctx.output.print(ensured)
    /// ```
    ///
    /// There are several optional named parameters:
    ///
    /// `exec_deps` - These are dependencies you wish to access as executables for creating the action.
    /// This is usually the same set of targets one would pass to rule's `attr.exec_dep`.
    /// `toolchains` - The set of toolchains needed for the actions you intend to create.
    /// `target_platform` - The intended target platform for your toolchains
    /// `exec_compatible_with` - Explicit list of configuration nodes (like platforms or constraints)
    /// that these actions are compatible with. This is the 'exec_compatible_with' attribute of a target.
    ///
    /// If you passed in `exec_deps` or `toolchains`, you can access the resolved dependencies using the `exec_deps`
    /// and `toolchains` attributes on the `bxl_actions`, which both return a `dict` of unconfigured subtarget labels
    /// and their configured/resolved `dependency` objects.
    ///
    /// Note that the keys of `exec_deps` and `toolchains` must be unconfigured subtarget labels (`providers_label`s),
    /// and not unconfigured target labels. You can use `ctx.unconfigured_sub_targets(...)` or `with_sub_target()` on
    /// `target_label` to create the label.
    ///
    /// ```python
    /// def _impl_run_action(ctx):
    ///    my_exec_dep = ctx.unconfigured_sub_targets("foo//bar:baz") # has some provider that you would use in the action
    ///    bxl_actions = ctx.bxl_actions(exec_deps = [my_exec_dep]) # call once, reuse wherever needed
    ///    output = bxl_actions.actions.run(
    ///        [
    ///            "python3",
    ///            bxl_actions.exec_deps[my_exec_dep][RunInfo], # access resolved exec_deps on the `bxl_actions`
    ///            out.as_output(),
    ///        ],
    ///        category = "command",
    ///        local_only = True,
    ///    )
    ///    ctx.output.ensure(output)
    /// ```
    ///
    /// When called from a `dynamic_output`, `bxl_actions()` cannot be configured with a different execution
    /// platform resolution from the parent BXL.
    fn bxl_actions<'v>(
        this: &'v BxlContext<'v>,
        #[starlark(require = named, default = NoneOr::None)] exec_deps: NoneOr<
            ProvidersExprArg<'v>,
        >,
        #[starlark(require = named, default = NoneOr::None)] toolchains: NoneOr<
            ProvidersExprArg<'v>,
        >,
        #[starlark(require = named, default = ValueAsStarlarkTargetLabel::NONE)]
        target_platform: ValueAsStarlarkTargetLabel<'v>,
        #[starlark(require = named, default = NoneOr::None)] exec_compatible_with: NoneOr<
            TargetListExprArg<'v>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<BxlActions<'v>> {
        let heap = eval.heap();
        Ok(this.via_dice(eval, |ctx| {
            ctx.via(|ctx| {
                async {
                    let (exec_deps, toolchains) = match &this.context_type {
                        BxlContextType::Root { .. } => {
                            let target_platform = this.resolve_target_platform(target_platform)?;
                            let exec_deps = match exec_deps {
                                NoneOr::None => Vec::new(),
                                NoneOr::Other(exec_deps) => {
                                    ProvidersExpr::<ProvidersLabel>::unpack(exec_deps, this)?
                                        .labels()
                                        .cloned()
                                        .collect()
                                }
                            };

                            let toolchains = match toolchains {
                                NoneOr::None => Vec::new(),
                                NoneOr::Other(toolchains) => {
                                    ProvidersExpr::<ProvidersLabel>::unpack(toolchains, this)?
                                        .labels()
                                        .cloned()
                                        .collect()
                                }
                            };

                            let exec_compatible_with: Arc<[_]> = match exec_compatible_with {
                                NoneOr::None => Arc::new([]),
                                NoneOr::Other(exec_compatible_with) => {
                                    TargetListExpr::<TargetNode>::unpack(
                                        exec_compatible_with,
                                        this,
                                        ctx,
                                    )
                                    .await?
                                    .get(ctx)
                                    .await?
                                    .iter()
                                    .map(|n| {
                                        // FIXME(nbadami,JakobDegen): This should support subtargets, but `TargetListExpr` cannot express that
                                        ConfigurationSettingKey(ProvidersLabel::default_for(
                                            n.label().dupe(),
                                        ))
                                    })
                                    .collect()
                                }
                            };

                            let execution_resolution = resolve_bxl_execution_platform(
                                ctx,
                                CellNameForConfigurationResolution(this.cell_name()),
                                exec_deps,
                                toolchains,
                                target_platform.dupe(),
                                exec_compatible_with.clone(),
                            )
                            .await?;

                            validate_action_instantiation(this, &execution_resolution)?;

                            (
                                execution_resolution.exec_deps_configured,
                                execution_resolution.toolchain_deps_configured,
                            )
                        }
                        BxlContextType::Dynamic(data) => {
                            if !exec_deps.is_none()
                                || !toolchains.is_none()
                                || !target_platform.is_none()
                                || !exec_compatible_with.is_none()
                            {
                                return Err(
                                    BxlContextError::RequireSameExecutionPlatformAsRoot.into()
                                );
                            }
                            (data.exec_deps.clone(), data.toolchains.clone())
                        }
                        BxlContextType::AnonTarget => {
                            if !exec_deps.is_none()
                                || !toolchains.is_none()
                                || !target_platform.is_none()
                                || !exec_compatible_with.is_none()
                            {
                                return Err(
                                    BxlContextError::RequireSameExecutionPlatformAsRoot.into()
                                );
                            }
                            // We will have a soft error for accessing bxl.Actions.exec_deps and
                            // bxl.Actions.toolchains for anon targets and dynamic actions
                            (vec![], vec![])
                        }
                    };

                    BxlActions::new(
                        this.state,
                        exec_deps.to_vec(),
                        toolchains.to_vec(),
                        heap,
                        ctx,
                    )
                    .await
                }
                .boxed_local()
            })
        })?)
    }

    /// Runs analysis on the given configured `labels`, accepting an optional `target_platform` which is the
    /// target platform configuration used to resolve configurations of any unconfigured target
    /// nodes, and an optional `skip_incompatible` boolean that indicates whether to skip analysis
    /// of nodes that are incompatible with the target platform.
    /// The `target_platform` is either a string that can be parsed as a target label, or a
    /// target label.
    ///
    /// The given `labels` is a providers expression of configured targets, which is either:
    ///     - a single target node or label, configured
    ///     - a single sub target label, configured
    ///     - a list of the two options above.
    ///     - targetset of configured target labels
    ///
    /// This returns either a single `analysis_result` if the given `labels` argument is "singular",
    /// or a dict keyed by sub target labels of `analysis` if the given `labels` argument
    /// is list-like
    fn analysis<'v>(
        this: &BxlContext<'v>,
        // TODO(nga): these parameters should be either position or named, not both.
        labels: ConfiguredProvidersExprArg<'v>,
        #[starlark(require = named, default = true)] skip_incompatible: bool,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<
        Either<
            NoneOr<StarlarkAnalysisResult>,
            BuckMutInsertionMap<
                ValueTyped<'v, StarlarkConfiguredProvidersLabel>,
                ValueTyped<'v, StarlarkAnalysisResult>,
            >,
        >,
    > {
        let providers = labels.unpack()?;

        let res: buck2_error::Result<_> = this.via_dice(eval, |dice| {
            dice.via(|dice| {
                async { analysis::analysis(dice, this, providers, skip_incompatible).await }
                    .boxed_local()
            })
        });

        Ok(match res? {
            Either::Left(single) => Either::Left(NoneOr::from_option(single)),
            Either::Right(many) => Either::Right(
                many.into_iter()
                    .map(|(t, v)| {
                        Ok((
                            eval.heap()
                                .alloc_typed(StarlarkConfiguredProvidersLabel::new(t))
                                .hashed()
                                .unwrap(),
                            eval.heap().alloc_typed(v),
                        ))
                    })
                    .collect::<buck2_error::Result<_>>()?,
            ),
        })
    }

    /// Runs a build on the given `labels`, accepting an optional `target_platform` which is the
    /// target platform configuration used to resolve configurations. Note that when `build()` is called,
    /// the artifacts are materialized without needing to additionally call `ensure()` on them.
    ///
    /// The given `labels` is a providers expression, which is either:
    ///     - a single string that is a `target pattern`.
    ///     - a single target node or label, configured or unconfigured
    ///     - a single provider label, configured or unconfigured
    ///     - a list of the two options above.
    ///
    /// materializations can be one of:
    ///    - "default"
    ///        - defer to the configuration settings to decide whether to materialize or not
    ///    - "materialize"
    ///        - force materialization of build results at the end of the build.
    ///    - "skip"
    ///        - skip materialization of the build results
    ///
    /// This returns a dict keyed by sub target labels mapped to `bxl_build_result`s if the
    /// given `labels` argument is list-like.
    ///
    /// This function is not available on the `bxl_ctx` when called from `dynamic_output`.
    fn build<'v>(
        this: &'v BxlContext<'v>,
        // TODO(nga): parameter should be either positional or named, not both.
        labels: AnyProvidersExprArg<'v>,
        #[starlark(default = ValueAsStarlarkTargetLabel::NONE)]
        target_platform: ValueAsStarlarkTargetLabel<'v>,
        #[starlark(require = named, default = "default")] materializations: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<
        BuckMutInsertionMap<
            ValueTyped<'v, StarlarkConfiguredProvidersLabel>,
            ValueTyped<'v, StarlarkBxlBuildResult>,
        >,
    > {
        Ok(build::build(
            this,
            labels,
            target_platform,
            Materializations::from_str_name(&materializations.to_uppercase()).ok_or_else(|| {
                buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "Unknown materialization setting `{}`",
                    materializations
                )
            })?,
            Uploads::Never,
            eval,
        )?)
    }

    /// A struct of the command line args as declared using the [`cli_args`] module.
    /// These command lines are resolved per the users input on the cli when invoking the bxl script.
    ///
    /// If you wish to pass in a kebab-cased arg, the arg accessed from the BXL context's `cli_args`
    /// attribute will always be in snakecase. For example, if you passed in `my-arg`, accessing it
    /// within BXL would look like `ctx.cli_args.my_arg`.
    ///
    /// This attribute is not available on the bxl context within the a dynamic lambda.
    #[starlark(attribute)]
    fn cli_args<'v>(this: &'v BxlContext<'v>) -> starlark::Result<ValueOf<'v, StructRef<'v>>> {
        let cli_args = this
            .context_type
            .unpack_root()
            .map_err(|_| {
                buck2_error::Error::from(BxlContextError::Unsupported("cli_args".to_owned()))
            })?
            .cli_args;

        Ok(cli_args)
    }

    /// Returns the `bxl.Filesystem` for performing a basic set of filesystem operations within bxl
    #[starlark(attribute)]
    fn fs<'v>(this: ValueTyped<'v, BxlContext<'v>>) -> starlark::Result<BxlFilesystem<'v>> {
        Ok(BxlFilesystem::new(this))
    }

    /// Checks if a target label exists. Target label must be a string literal, and an exact target.
    fn target_exists<'v>(
        this: &'v BxlContext<'v>,
        label: &'v str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<bool> {
        Ok(this.via_dice(eval, |ctx| {
            ctx.via(|ctx| {
                async move {
                    match ParsedPattern::<TargetPatternExtra>::parse_relaxed(
                        this.target_alias_resolver(),
                        CellPath::new(this.cell_name(), CellRelativePath::empty()),
                        label,
                        this.cell_resolver(),
                        this.cell_alias_resolver(),
                    )? {
                        ParsedPattern::Target(pkg, name, TargetPatternExtra) => {
                            let target_label = TargetLabel::new(pkg, name.as_ref());
                            Ok(ctx.get_target_node(&target_label).await.ok().is_some())
                        }
                        _ => Err(NotATargetLabelString.into()),
                    }
                }
                .boxed_local()
            })
        })?)
    }

    /// Returns the `audit_ctx` that holds all the audit functions.
    fn audit<'v>(this: ValueTyped<'v, BxlContext<'v>>) -> starlark::Result<StarlarkAuditCtx<'v>> {
        let working_dir = this
            .cell_resolver()
            .get(this.cell_name())?
            .path()
            .as_project_relative_path()
            .to_buf();

        Ok(StarlarkAuditCtx::new(this, working_dir)?)
    }

    /// Awaits a promise and returns an optional value of the promise.
    ///
    /// Sample usage:
    /// ```python
    /// load("//path/to/rules:rules.bzl", "my_anon_targets_rule", "my_map_function")
    ///
    /// def _resolve_impl(ctx):
    ///     actions = ctx.bxl_actions().actions
    ///     my_attrs = {
    ///         "false": False,
    ///         "int": 42,
    ///         "list_string": ["a", "b", "c"],
    ///         "string": "a-string",
    ///         "true": True,
    ///     }
    ///
    ///     promise = actions.anon_target(my_anon_targets_rule, attrs).promise.map(my_map_function)
    ///     providers_result = ctx.resolve(actions, promise) # result is `ProviderCollection` type, which is a collection of `Provider`s
    ///     ctx.output.print(providers_result[0].my_field)
    /// ```
    fn resolve<'v>(
        this: &'v BxlContext<'v>,
        action_factory: ValueTyped<'v, AnalysisActions<'v>>,
        promise: ValueTyped<'v, StarlarkPromise<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneOr<Value<'v>>> {
        run_anon_target_promises(action_factory, this, eval)?;
        Ok(match promise.get() {
            Some(v) => NoneOr::Other(v),
            None => NoneOr::None,
        })
    }

    /// Emits a user-defined instant event, taking in a required string id and a metadata dictionary where the
    /// keys are strings, and values are either strings, bools, or ints. The id is user-supplied, and used to
    /// identify the instant events in the event logs more easily.
    ///
    /// You may pass in an ensured artifact as a value in the metadata. The resulting output would be the ensured
    /// artifact's relative or absolute path as a string.
    fn instant_event<'v>(
        this: &'v BxlContext<'v>,
        #[starlark(require = named)] id: &str,
        #[starlark(require = named)] metadata: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneType> {
        let parser = StarlarkUserEventParser {
            artifact_fs: this.artifact_fs(),
            project_fs: this.project_fs(),
        };
        let event = parser.parse(id, metadata)?;

        let Ok(()) = this.via_dice(eval, |ctx| {
            ctx.per_transaction_data()
                .get_dispatcher()
                .instant_event(event);
            Ok::<_, !>(())
        });

        Ok(NoneType)
    }

    /// Lazy/batch/error handling operations.
    #[starlark(attribute)]
    fn lazy<'v>(this: ValueTyped<'v, BxlContext<'v>>) -> starlark::Result<StarlarkLazyCtx<'v>> {
        Ok(StarlarkLazyCtx::new(this))
    }

    /// The target_platform from the bxl invocation. It is from the `--target-platforms` flag.
    #[starlark(attribute)]
    fn target_platform<'v>(
        this: &'v BxlContext<'v>,
    ) -> starlark::Result<NoneOr<StarlarkTargetLabel>> {
        Ok(NoneOr::from_option(
            this.global_cfg_options()
                .target_platform
                .dupe()
                .map(StarlarkTargetLabel::new),
        ))
    }

    /// The modifiers from the bxl invocation. It is from the `--modifier` flag.
    #[starlark(attribute)]
    fn modifiers<'v>(this: &'v BxlContext<'v>) -> starlark::Result<Vec<String>> {
        Ok((*this.global_cfg_options().cli_modifiers).clone())
    }
}
