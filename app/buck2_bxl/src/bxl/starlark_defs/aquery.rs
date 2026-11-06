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
use buck2_artifact::actions::key::ActionKey;
use buck2_build_api::actions::query::ActionQueryNode;
use buck2_build_api::query::bxl::BxlAqueryFunctions;
use buck2_build_api::query::bxl::NEW_BXL_AQUERY_FUNCTIONS;
use buck2_build_api::query::oneshot::QUERY_FRONTEND;
use buck2_core::configuration::compatibility::IncompatiblePlatformReason;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_query::query::syntax::simple::functions::helpers::CapturedExpr;
use derivative::Derivative;
use derive_more::Display;
use dice::DiceComputations;
use dupe::Dupe;
use futures::FutureExt;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::AllocValue;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list::ListType;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::context::ErrorPrinter;
use crate::bxl::starlark_defs::nodes::action::StarlarkAction;
use crate::bxl::starlark_defs::nodes::action::StarlarkActionQueryNode;
use crate::bxl::starlark_defs::providers_expr::AnyProvidersExprArg;
use crate::bxl::starlark_defs::providers_expr::ProvidersExpr;
use crate::bxl::starlark_defs::query_util::parse_query_evaluation_result;
use crate::bxl::starlark_defs::target_list_expr::ConfiguredTargetListExprArg;
use crate::bxl::starlark_defs::target_list_expr::TargetListExpr;
use crate::bxl::starlark_defs::targetset::StarlarkTargetSet;
use crate::bxl::starlark_defs::uquery::UnpackUnconfiguredQueryArgs;
use crate::bxl::value_as_starlark_target_label::ValueAsStarlarkTargetLabel;

#[derive(
    ProvidesStaticType,
    Derivative,
    Display,
    Trace,
    NoSerialize,
    Allocative
)]
#[derivative(Debug)]
#[display("{:?}", self)]
#[allocative(skip)]
pub(crate) struct StarlarkAQueryCtx<'v> {
    #[derivative(Debug = "ignore")]
    ctx: ValueTyped<'v, BxlContext<'v>>,
    #[derivative(Debug = "ignore")]
    // Overrides the GlobalCfgOptions in the BxlContext
    global_cfg_options_override: GlobalCfgOptions,
}

starlark::methods_static!(AQUERY_METHODS = aquery_methods);

#[starlark_value(type = "bxl.AqueryContext", StarlarkTypeRepr, UnpackValue)]
impl<'v> StarlarkValue<'v> for StarlarkAQueryCtx<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(AQUERY_METHODS.methods())
    }
}

impl<'v> AllocValue<'v> for StarlarkAQueryCtx<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

impl<'v> StarlarkAQueryCtx<'v> {
    pub(crate) fn new(
        ctx: ValueTyped<'v, BxlContext<'v>>,
        global_target_platform: ValueAsStarlarkTargetLabel<'v>,
    ) -> buck2_error::Result<StarlarkAQueryCtx<'v>> {
        let global_cfg_options = ctx.resolve_global_cfg_options(global_target_platform, vec![])?;

        Ok(Self {
            ctx,
            global_cfg_options_override: global_cfg_options,
        })
    }
}

pub(crate) async fn get_aquery_env(
    ctx: &BxlContext<'_>,
    global_cfg_options_override: &GlobalCfgOptions,
) -> buck2_error::Result<Box<dyn BxlAqueryFunctions>> {
    (NEW_BXL_AQUERY_FUNCTIONS.get()?)(
        global_cfg_options_override.clone(),
        ctx.project_root().dupe(),
        ctx.cell_name(),
        ctx.cell_resolver().dupe(),
    )
    .await
}

#[derive(StarlarkTypeRepr, UnpackValue)]
enum UnpackActionNodes<'v> {
    ActionQueryNodes(ValueOf<'v, ListType<StarlarkActionQueryNode>>),
    ActionQueryNodesSet(&'v StarlarkTargetSet<ActionQueryNode>),
    ConfiguredProviders(AnyProvidersExprArg<'v>),
    ConfiguredTargets(ConfiguredTargetListExprArg<'v>),
    StarlarkActions(ValueOf<'v, ListType<StarlarkAction>>),
}

// Aquery operates on `ActionQueryNode`s. Under the hood, the target set of action query nodes is obtained
// by running analysis on a configured providers label. We can accept either a `TargetExpr`, `ProvidersExpr`,
// or a `TargetSet<ActionQueryNode>` (which would have been produced from a previous aquery). For `TargetExpr`
// and `ProvidersExpr`, we need to pass the aquery delegate a list of configured providers labels, and it will
// run analysis on them to construct the `ActionQueryNode`s.
async fn unpack_action_nodes<'v>(
    this: &StarlarkAQueryCtx<'v>,
    dice: &mut DiceComputations<'_>,
    expr: UnpackActionNodes<'v>,
) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
    let aquery_env = get_aquery_env(&this.ctx, &this.global_cfg_options_override).await?;
    let providers = match expr {
        UnpackActionNodes::ActionQueryNodes(action_nodes) => {
            return action_nodes
                .try_items()
                .map(|v| buck2_error::Ok(v?.0))
                .collect();
        }
        UnpackActionNodes::ActionQueryNodesSet(action_nodes) => return Ok(action_nodes.0.clone()),
        UnpackActionNodes::StarlarkActions(actions) => {
            let action_keys: Vec<ActionKey> = actions
                .try_items()
                .map(|starlark_action| buck2_error::Ok(starlark_action?.0.key().dupe()))
                .collect::<buck2_error::Result<_>>()?;

            return aquery_env.get_action_nodes(dice, action_keys).await;
        }
        UnpackActionNodes::ConfiguredProviders(arg) => {
            ProvidersExpr::<ConfiguredProvidersLabel>::unpack(
                arg,
                &this.global_cfg_options_override,
                &this.ctx,
                dice,
            )
            .await?
            .labels()
            .cloned()
            .collect()
        }
        UnpackActionNodes::ConfiguredTargets(arg) => {
            TargetListExpr::<ConfiguredTargetNode>::unpack_opt(
                arg,
                &this.global_cfg_options_override,
                &this.ctx,
                dice,
                false,
            )
            .await?
            .as_provider_labels()
        }
    };

    let (incompatible_targets, result) = aquery_env.get_target_set(dice, providers).await?;

    if !incompatible_targets.is_empty() {
        this.ctx.print_to_error_stream(
            IncompatiblePlatformReason::skipping_message_for_multiple(incompatible_targets.iter()),
        )?;
    }

    Ok(result)
}
/// The context for performing `aquery` operations in bxl. The functions offered on this ctx are
/// the same behaviour as the query functions available within aquery command.
///
/// An instance may be obtained with [`bxl.Context.aquery()`](../Context/#contextaquery).
///
/// Query results are `target_set`s of `action_query_node`s, which supports iteration,
/// indexing, `len()`, set addition/subtraction, and `equals()`.
///
/// Actions can be specified as:
/// - Target expressions (configured targets/providers)
/// - Existing action query nodes or target sets
/// - `bxl.Action` objects (obtained from `ctx.audit().output()`)
#[starlark_module]
fn aquery_methods(builder: &mut MethodsBuilder) {
    /// The deps query for finding the transitive closure of dependencies.
    fn deps<'v>(
        this: &StarlarkAQueryCtx<'v>,
        // TODO(nga): parameters should be either positional or named, not both.
        universe: UnpackActionNodes<'v>,
        #[starlark(default = NoneOr::None)] depth: NoneOr<i32>,
        #[starlark(default = NoneOr::None)] filter: NoneOr<&'v str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ActionQueryNode>> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let filter = filter
                            .into_option()
                            .map(buck2_query_parser::parse_expr)
                            .transpose()?;

                        let universe = unpack_action_nodes(this, dice, universe).await?;

                        let aquery_env =
                            get_aquery_env(&this.ctx, &this.global_cfg_options_override).await?;
                        aquery_env
                            .deps(
                                dice,
                                &universe,
                                depth.into_option().into(),
                                filter
                                    .as_ref()
                                    .map(|span| CapturedExpr { expr: span })
                                    .as_ref(),
                            )
                            .await
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkTargetSet::from)?)
    }

    /// Obtain all the actions declared within the analysis of a given target.
    ///
    /// This operation only makes sense on a target literal (it is a simple passthrough when passed
    /// an action).
    fn all_actions<'v>(
        this: &StarlarkAQueryCtx<'v>,
        // TODO(nga): parameters should be either positional or named, not both.
        targets: UnpackActionNodes<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ActionQueryNode>> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let targets = unpack_action_nodes(this, dice, targets).await?;
                        get_aquery_env(&this.ctx, &this.global_cfg_options_override)
                            .await?
                            .all_actions(dice, &targets)
                            .await
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkTargetSet::from)?)
    }

    /// Obtain the actions for all the outputs provided by the `DefaultInfo` for the targets passed
    /// as input. This includes both the `default_outputs` and `other_outputs`.
    ///
    /// This operation only makes sense on a target literal (it does nothing if passed something
    /// else).
    fn all_outputs<'v>(
        this: &StarlarkAQueryCtx<'v>,
        // TODO(nga): parameters should be either positional or named, not both.
        targets: UnpackActionNodes<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ActionQueryNode>> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let targets = unpack_action_nodes(this, dice, targets).await?;

                        get_aquery_env(&this.ctx, &this.global_cfg_options_override)
                            .await?
                            .all_outputs(dice, &targets)
                            .await
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkTargetSet::from)?)
    }

    /// The attrfilter query for rule attribute filtering.
    fn attrfilter<'v>(
        this: &StarlarkAQueryCtx<'v>,
        // TODO(nga): parameters should be either positional or named, not both.
        attr: &str,
        value: &str,
        targets: UnpackActionNodes<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ActionQueryNode>> {
        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    let targets = unpack_action_nodes(this, dice, targets).await?;

                    targets
                        .attrfilter(attr, &|v| Ok(v == value))
                        .map(StarlarkTargetSet::from)
                }
                .boxed_local()
            })
        })?)
    }

    /// Evaluates some general query string. `query_args` can be a target_set of unconfigured nodes, or
    /// a list of strings. Returns a `dict` of target labels mapped to their `target_set` results if `query_args`
    /// was passed in, otherwise returns a single `target_set`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_eval(ctx):
    ///     result = ctx.aquery().eval(":foo")
    ///     ctx.output.print(result)
    /// ```
    fn eval<'v>(
        this: &StarlarkAQueryCtx<'v>,
        query: &'v str,
        #[starlark(default = NoneOr::None)] query_args: NoneOr<UnpackUnconfiguredQueryArgs<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        let query_args = match query_args {
            NoneOr::None => Vec::new(),
            NoneOr::Other(query_args) => query_args.into_strings()?,
        };

        let heap = eval.heap();

        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    parse_query_evaluation_result(
                        QUERY_FRONTEND
                            .get()?
                            .eval_aquery(
                                dice,
                                &this.ctx.working_dir()?,
                                query,
                                &query_args,
                                this.global_cfg_options_override.clone(),
                            )
                            .await?,
                        heap,
                    )
                }
                .boxed_local()
            })
        })?)
    }
}
