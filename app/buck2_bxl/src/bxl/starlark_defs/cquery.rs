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
use buck2_build_api::query::bxl::BxlCqueryFunctions;
use buck2_build_api::query::bxl::NEW_BXL_CQUERY_FUNCTIONS;
use buck2_build_api::query::oneshot::QUERY_FRONTEND;
use buck2_core::global_cfg_options::GlobalCfgOptions;
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
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;

use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::file_set::FileSetExpr;
use crate::bxl::starlark_defs::file_set::StarlarkFileSet;
use crate::bxl::starlark_defs::query_util::parse_query_evaluation_result;
use crate::bxl::starlark_defs::target_list_expr::ConfiguredTargetListExprArg;
use crate::bxl::starlark_defs::target_list_expr::TargetListExpr;
use crate::bxl::starlark_defs::target_list_expr::filter_incompatible;
use crate::bxl::starlark_defs::targetset::StarlarkTargetSet;
use crate::bxl::starlark_defs::uquery::UnpackUnconfiguredQueryArgs;

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
pub(crate) struct StarlarkCQueryCtx<'v> {
    #[derivative(Debug = "ignore")]
    ctx: ValueTyped<'v, BxlContext<'v>>,
    #[derivative(Debug = "ignore")]
    // Overrides the GlobalCfgOptions in the BxlContext
    global_cfg_options_override: GlobalCfgOptions,
}

starlark::methods_static!(CQUERY_METHODS = cquery_methods);

#[starlark_value(type = "bxl.CqueryContext", StarlarkTypeRepr, UnpackValue)]
impl<'v> StarlarkValue<'v> for StarlarkCQueryCtx<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(CQUERY_METHODS.methods())
    }
}

impl<'v> AllocValue<'v> for StarlarkCQueryCtx<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

pub(crate) async fn get_cquery_env(
    ctx: &BxlContext<'_>,
    global_cfg_options_override: &GlobalCfgOptions,
) -> buck2_error::Result<Box<dyn BxlCqueryFunctions>> {
    (NEW_BXL_CQUERY_FUNCTIONS.get()?)(
        global_cfg_options_override.clone(),
        ctx.project_root().dupe(),
        ctx.cell_name(),
        ctx.cell_resolver().dupe(),
    )
    .await
}

async fn unpack_targets<'v>(
    this: &StarlarkCQueryCtx<'v>,
    dice: &mut DiceComputations<'_>,
    targets: ConfiguredTargetListExprArg<'v>,
) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>> {
    filter_incompatible(
        TargetListExpr::<'v, ConfiguredTargetNode>::unpack(
            targets,
            &this.global_cfg_options_override,
            &this.ctx,
            dice,
        )
        .await?
        .get(dice)
        .await?,
        &*this.ctx,
    )
}

impl<'v> StarlarkCQueryCtx<'v> {
    pub(crate) fn new(
        ctx: ValueTyped<'v, BxlContext<'v>>,
        global_cfg_options: GlobalCfgOptions,
    ) -> buck2_error::Result<StarlarkCQueryCtx<'v>> {
        Ok(Self {
            ctx,
            global_cfg_options_override: global_cfg_options,
        })
    }
}

/// The context for performing `cquery` operations in bxl. The functions offered on this ctx are
/// the same behaviour as the query functions available within cquery command.
///
/// An instance may be obtained with [`bxl.Context.cquery`](../Context/#contextcquery).
///
/// Query results are `target_set`s of `target_node`s, which supports iteration,
/// indexing, `len()`, set addition/subtraction, and `equals()`.
#[starlark_module]
fn cquery_methods(builder: &mut MethodsBuilder) {
    /// The `allpaths` query for computing all dependency paths.
    fn allpaths<'v>(
        this: &StarlarkCQueryCtx<'v>,
        from: ConfiguredTargetListExprArg<'v>,
        to: ConfiguredTargetListExprArg<'v>,
        #[starlark(default = NoneOr::None)] filter: NoneOr<&'v str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ConfiguredTargetNode>> {
        Ok(this.ctx.via_dice(eval, move |dice| {
            dice.via(|dice| {
                async move {
                    let filter = filter
                        .into_option()
                        .map(buck2_query_parser::parse_expr)
                        .transpose()?;
                    let from = unpack_targets(this, dice, from).await?;
                    let to = unpack_targets(this, dice, to).await?;
                    get_cquery_env(&this.ctx, &this.global_cfg_options_override)
                        .await?
                        .allpaths(
                            dice,
                            &from,
                            &to,
                            filter.as_ref().map(|expr| CapturedExpr { expr }).as_ref(),
                        )
                        .await
                        .map(StarlarkTargetSet::from)
                }
                .boxed_local()
            })
        })?)
    }

    // The somepath query.
    fn somepath<'v>(
        this: &StarlarkCQueryCtx<'v>,
        from: ConfiguredTargetListExprArg<'v>,
        to: ConfiguredTargetListExprArg<'v>,
        #[starlark(default = NoneOr::None)] filter: NoneOr<&'v str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ConfiguredTargetNode>> {
        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    let filter = filter
                        .into_option()
                        .map(buck2_query_parser::parse_expr)
                        .transpose()?;

                    let from = unpack_targets(this, dice, from).await?;
                    let to = unpack_targets(this, dice, to).await?;
                    get_cquery_env(&this.ctx, &this.global_cfg_options_override)
                        .await?
                        .somepath(
                            dice,
                            &from,
                            &to,
                            filter.as_ref().map(|expr| CapturedExpr { expr }).as_ref(),
                        )
                        .await
                        .map(StarlarkTargetSet::from)
                }
                .boxed_local()
            })
        })?)
    }

    /// The attrfilter query for rule attribute filtering.
    fn attrfilter<'v>(
        this: &StarlarkCQueryCtx<'v>,
        attr: &str,
        value: &str,
        targets: ConfiguredTargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ConfiguredTargetNode>> {
        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    unpack_targets(this, dice, targets)
                        .await?
                        .attrfilter(attr, &|v| Ok(v == value))
                        .map(StarlarkTargetSet::from)
                }
                .boxed_local()
            })
        })?)
    }

    /// The nattrfilter query for rule attribute filtering.
    /// It is the opposite of `attrfilter`, i.e. it filters targets by attribute but excludes those that match.
    fn nattrfilter<'v>(
        this: &StarlarkCQueryCtx<'v>,
        attr: &str,
        value: &str,
        targets: ConfiguredTargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ConfiguredTargetNode>> {
        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    unpack_targets(this, dice, targets)
                        .await?
                        .nattrfilter(attr, &|v| Ok(v == value))
                        .map(StarlarkTargetSet::from)
                }
                .boxed_local()
            })
        })?)
    }

    /// Filter targets by rule type.
    /// Returns a subset of `targets` where the rule type matches the specified `regex`. The specified pattern can be a regular expression.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_kind(ctx):
    ///     kind = ctx.cquery().kind("cpp.*", "bin/libs/...")
    ///     ctx.output.print(nodes)
    /// ```
    fn kind<'v>(
        this: &StarlarkCQueryCtx<'v>,
        regex: &str,
        targets: ConfiguredTargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ConfiguredTargetNode>> {
        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    unpack_targets(this, dice, targets)
                        .await?
                        .kind(regex)
                        .map(StarlarkTargetSet::from)
                }
                .boxed_local()
            })
        })?)
    }

    /// The attrregexfilter query for rule attribute filtering with regex.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_attrregexfilter(ctx):
    ///     filtered = ctx.cquery().attrregexfilter("foo", "he.lo", "bin/kind/...")
    ///     ctx.output.print(filtered)
    /// ```
    fn attrregexfilter<'v>(
        this: &StarlarkCQueryCtx<'v>,
        attribute: &str,
        value: &str,
        targets: ConfiguredTargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ConfiguredTargetNode>> {
        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    unpack_targets(this, dice, targets)
                        .await?
                        .attrregexfilter(attribute, value)
                        .map(StarlarkTargetSet::from)
                }
                .boxed_local()
            })
        })?)
    }

    /// The owner query for finding targets that own specified files. Note that if you do not pass in a cell
    /// path (where the format is `<cell>//path/to/file`), the path is resolved against the cell that the BXL
    /// script lives in. If you need to evaluate a file path that lives in a different cell, you must pass in
    /// the fully qualified cell path.
    ///
    /// Sample usage:
    /// ```python
    /// def _owner_impl(ctx):
    ///     owner = ctx.cquery().owner("bin/TARGETS.fixture", "foo//target/universe/...")
    ///     ctx.output.print(owner)
    /// ```
    fn owner<'v>(
        this: &StarlarkCQueryCtx<'v>,
        files: FileSetExpr,
        #[starlark(default = NoneOr::None)] universe: NoneOr<ConfiguredTargetListExprArg<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ConfiguredTargetNode>> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let universe = match universe.into_option() {
                            Some(universe) => Some(unpack_targets(this, dice, universe).await?),
                            None => None,
                        };

                        get_cquery_env(&this.ctx, &this.global_cfg_options_override)
                            .await?
                            .owner(
                                dice,
                                files.get(&this.ctx).await?.as_ref(),
                                universe.as_ref(),
                            )
                            .await
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkTargetSet::from)?)
    }

    /// The deps query for finding the transitive closure of dependencies.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_deps(ctx):
    ///     result = ctx.cquery().deps("root//bin:the_binary", 1)
    ///     ctx.output.print(result)
    /// ```
    fn deps<'v>(
        this: &StarlarkCQueryCtx<'v>,
        universe: ConfiguredTargetListExprArg<'v>,
        #[starlark(default = NoneOr::None)] depth: NoneOr<i32>,
        #[starlark(default = NoneOr::None)] filter: NoneOr<&'v str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ConfiguredTargetNode>> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let filter = filter
                            .into_option()
                            .map(buck2_query_parser::parse_expr)
                            .transpose()?;

                        let targets = unpack_targets(this, dice, universe).await?;

                        get_cquery_env(&this.ctx, &this.global_cfg_options_override)
                            .await?
                            .deps(
                                dice,
                                &targets,
                                depth.into_option().into(),
                                filter.as_ref().map(|expr| CapturedExpr { expr }).as_ref(),
                            )
                            .await
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkTargetSet::from)?)
    }

    /// The filter query for filtering targets by name.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_filter(ctx):
    ///     result = ctx.cquery().filter(".*the_binary", "root//...")
    ///     ctx.output.print(result)
    /// ```
    fn filter<'v>(
        this: &StarlarkCQueryCtx<'v>,
        regex: &str,
        targets: ConfiguredTargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ConfiguredTargetNode>> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        unpack_targets(this, dice, targets)
                            .await?
                            .filter_name(regex)
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkTargetSet::from)?)
    }

    /// The inputs query for finding input files.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_inputs(ctx):
    ///     result = ctx.cquery().inputs("root//bin:the_binary")
    ///     ctx.output.print(result)
    /// ```
    fn inputs<'v>(
        this: &StarlarkCQueryCtx<'v>,
        targets: ConfiguredTargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkFileSet> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async { unpack_targets(this, dice, targets).await?.inputs() }.boxed_local()
                })
            })
            .map(StarlarkFileSet::from)?)
    }

    /// The testsof query for listing the tests of the specified targets.
    fn testsof<'v>(
        this: &StarlarkCQueryCtx<'v>,
        targets: ConfiguredTargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ConfiguredTargetNode>> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let targets = unpack_targets(this, dice, targets).await?;
                        get_cquery_env(&this.ctx, &this.global_cfg_options_override)
                            .await?
                            .testsof(dice, &targets)
                            .await
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkTargetSet::from)?)
    }

    /// The testsof query for listing the tests of the specified targets. Performs default target platform
    /// resolution under the hood for the tests found.
    fn testsof_with_default_target_platform<'v>(
        this: &StarlarkCQueryCtx<'v>,
        targets: ConfiguredTargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ConfiguredTargetNode>> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let targets = unpack_targets(this, dice, targets).await?;
                        let maybe_compatibles =
                            get_cquery_env(&this.ctx, &this.global_cfg_options_override)
                                .await?
                                .testsof_with_default_target_platform(dice, &targets)
                                .await?;

                        filter_incompatible(maybe_compatibles, &*this.ctx)
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkTargetSet::from)?)
    }

    /// The rdeps query for finding the transitive closure of reverse dependencies.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_rdeps(ctx):
    ///     result = ctx.cquery().rdeps("root//bin:the_binary", "//lib:file1", 100)
    ///     ctx.output.print(result)
    /// ```
    fn rdeps<'v>(
        this: &StarlarkCQueryCtx<'v>,
        universe: ConfiguredTargetListExprArg<'v>,
        from: ConfiguredTargetListExprArg<'v>,
        depth: Option<i32>,
        #[starlark(default = NoneOr::None)] filter: NoneOr<&'v str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<ConfiguredTargetNode>> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let filter = filter
                            .into_option()
                            .map(buck2_query_parser::parse_expr)
                            .transpose()?;
                        let universe = unpack_targets(this, dice, universe).await?;
                        let targets = unpack_targets(this, dice, from).await?;
                        get_cquery_env(&this.ctx, &this.global_cfg_options_override)
                            .await?
                            .rdeps(
                                dice,
                                &universe,
                                &targets,
                                depth.into(),
                                filter.as_ref().map(|expr| CapturedExpr { expr }).as_ref(),
                            )
                            .await
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkTargetSet::from)?)
    }

    /// Evaluates some general query string. `query_args` can be a target_set of unconfigured nodes, or
    /// a list of strings. Returns a `dict` of target labels mapped to their `target_set` results if `query_args`
    /// was passed in, otherwise returns a single `target_set`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_eval(ctx):
    ///     result1 = ctx.cquery().eval("inputs(root//bin:the_binary)")
    ///     ctx.output.print(result1)
    ///
    ///     result2 = ctx.cquery().eval("inputs(%s)", query_args = ["cell//path/to/file:target"])
    ///     ctx.output.print(result2)
    /// ```
    fn eval<'v>(
        this: &StarlarkCQueryCtx<'v>,
        query: &'v str,
        #[starlark(default = NoneOr::None)] query_args: NoneOr<UnpackUnconfiguredQueryArgs<'v>>,
        #[starlark(default = NoneOr::None)] target_universe: NoneOr<
            ValueOf<'v, ListOrTupleType<String>>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        let query_args = match query_args {
            NoneOr::None => Vec::new(),
            NoneOr::Other(query_args) => query_args.into_strings()?,
        };

        let target_universe = target_universe
            .into_option()
            .map(|v| v.try_to_vec())
            .transpose()?;
        let heap = eval.heap();

        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    parse_query_evaluation_result(
                        QUERY_FRONTEND
                            .get()?
                            .eval_cquery(
                                dice,
                                &this.ctx.working_dir()?,
                                query,
                                &query_args,
                                this.global_cfg_options_override.clone(),
                                target_universe.as_deref(),
                                false,
                            )
                            .await?
                            .0,
                        heap,
                    )
                }
                .boxed_local()
            })
        })?)
    }

    /// Find the build file(s) that defines a target or a target set.
    ///
    /// Sample usage:
    /// ```python
    /// def _buildfile_impl(ctx):
    ///     owner = ctx.cquery().owner(["bin/TARGET", "bin/kind"])
    ///     result = ctx.cquery().buildfile(owner)
    ///     ctx.output.print(result)
    /// ```
    fn buildfile<'v>(
        this: &StarlarkCQueryCtx<'v>,
        targets: ConfiguredTargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkFileSet> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let targets = unpack_targets(this, dice, targets).await?;
                        Ok(targets.buildfile())
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkFileSet::from)?)
    }

    /// The allbuildfiles query for finding the build files of the given targets and their transitive imports.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_allbuildfiles(ctx):
    ///     result = ctx.cquery().allbuildfiles("root//bin:the_binary")
    ///     ctx.output.print(result)
    /// ```
    fn allbuildfiles<'v>(
        this: &StarlarkCQueryCtx<'v>,
        universe: ConfiguredTargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkFileSet> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let universe = unpack_targets(this, dice, universe).await?;
                        get_cquery_env(&this.ctx, &this.global_cfg_options_override)
                            .await?
                            .allbuildfiles(dice, &universe)
                            .await
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkFileSet::from)?)
    }

    /// The rbuildfiles query for finding all build files that transitively depend on the given files.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_rbuildfiles(ctx):
    ///     result = ctx.cquery().rbuildfiles("bin/TARGETS", "bin/defs.bzl")
    ///     ctx.output.print(result)
    /// ```
    fn rbuildfiles<'v>(
        this: &StarlarkCQueryCtx<'v>,
        universe: FileSetExpr,
        argset: FileSetExpr,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkFileSet> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        get_cquery_env(&this.ctx, &this.global_cfg_options_override)
                            .await?
                            .rbuildfiles(
                                dice,
                                (universe.get(&this.ctx).await?).as_ref(),
                                (argset.get(&this.ctx).await?).as_ref(),
                            )
                            .await
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkFileSet::from)?)
    }
}
