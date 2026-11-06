/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;

use allocative::Allocative;
use buck2_build_api::query::bxl::BxlUqueryFunctions;
use buck2_build_api::query::bxl::NEW_BXL_UQUERY_FUNCTIONS;
use buck2_build_api::query::oneshot::QUERY_FRONTEND;
use buck2_node::nodes::unconfigured::TargetNode;
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

use super::file_set::StarlarkFileSet;
use super::target_list_expr::TargetListExpr;
use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::context::BxlContextCoreData;
use crate::bxl::starlark_defs::file_set::FileSetExpr;
use crate::bxl::starlark_defs::query_util::parse_query_evaluation_result;
use crate::bxl::starlark_defs::target_list_expr::TargetListExprArg;
use crate::bxl::starlark_defs::targetset::StarlarkTargetSet;

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
pub(crate) struct StarlarkUQueryCtx<'v> {
    #[derivative(Debug = "ignore")]
    ctx: ValueTyped<'v, BxlContext<'v>>,
    allow_partial_graph: bool,
}

starlark::methods_static!(UQUERY_METHODS = uquery_methods);

#[starlark_value(type = "bxl.UqueryContext", StarlarkTypeRepr, UnpackValue)]
impl<'v> StarlarkValue<'v> for StarlarkUQueryCtx<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(UQUERY_METHODS.methods())
    }
}

pub(crate) async fn get_uquery_env(
    ctx: &BxlContextCoreData,
    allow_partial_graph: bool,
) -> buck2_error::Result<Box<dyn BxlUqueryFunctions>> {
    (NEW_BXL_UQUERY_FUNCTIONS.get()?)(
        allow_partial_graph,
        ctx.project_root().dupe(),
        ctx.cell_name(),
        ctx.cell_resolver().dupe(),
    )
    .await
}

impl<'v> AllocValue<'v> for StarlarkUQueryCtx<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

impl<'v> StarlarkUQueryCtx<'v> {
    pub(crate) fn new(
        ctx: ValueTyped<'v, BxlContext<'v>>,
        allow_partial_graph: bool,
    ) -> buck2_error::Result<Self> {
        Ok(Self {
            ctx,
            allow_partial_graph,
        })
    }
}

async fn unpack_targets<'c, 'v>(
    this: &'c StarlarkUQueryCtx<'v>,
    dice: &'c mut DiceComputations<'_>,
    targets: TargetListExprArg<'v>,
) -> buck2_error::Result<Cow<'v, TargetSet<TargetNode>>> {
    TargetListExpr::<'v, TargetNode>::unpack(targets, &this.ctx, dice)
        .await?
        .get(dice)
        .await
}

/// The context for performing `uquery` operations in bxl. The functions offered on this ctx are
/// the same behaviour as the query functions available within uquery command.
///
/// An instance may be obtained with [`bxl.Context.uquery()`](../Context/#contextuquery).
#[starlark_module]
fn uquery_methods(builder: &mut MethodsBuilder) {
    /// The `allpaths` query for computing all dependency paths.
    fn allpaths<'v>(
        this: &StarlarkUQueryCtx<'v>,
        from: TargetListExprArg<'v>,
        to: TargetListExprArg<'v>,
        #[starlark(default = NoneOr::None)] filter: NoneOr<&'v str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<TargetNode>> {
        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    let filter = filter
                        .into_option()
                        .map(buck2_query_parser::parse_expr)
                        .transpose()?;
                    let from = unpack_targets(this, dice, from).await?;
                    let to = unpack_targets(this, dice, to).await?;
                    get_uquery_env(&this.ctx, this.allow_partial_graph)
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

    /// The somepaths query, which returns the graph of nodes on some arbitrary path from a start to destination target.
    fn somepath<'v>(
        this: &StarlarkUQueryCtx<'v>,
        from: TargetListExprArg<'v>,
        to: TargetListExprArg<'v>,
        #[starlark(default = NoneOr::None)] filter: NoneOr<&'v str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<TargetNode>> {
        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    let filter = filter
                        .into_option()
                        .map(buck2_query_parser::parse_expr)
                        .transpose()?;

                    let from = unpack_targets(this, dice, from).await?;
                    let to = unpack_targets(this, dice, to).await?;
                    get_uquery_env(&this.ctx, this.allow_partial_graph)
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
        this: &StarlarkUQueryCtx<'v>,
        attr: &str,
        value: &str,
        targets: TargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<TargetNode>> {
        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    let targets = unpack_targets(this, dice, targets).await?;
                    targets
                        .attrfilter(attr, &|v| Ok(v == value))
                        .map(StarlarkTargetSet::from)
                }
                .boxed_local()
            })
        })?)
    }

    /// The inputs query for finding input files.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_inputs(ctx):
    ///     result = ctx.uquery().inputs("root//bin:the_binary")
    ///     ctx.output.print(result)
    /// ```
    fn inputs<'v>(
        this: &StarlarkUQueryCtx<'v>,
        targets: TargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkFileSet> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let targets = unpack_targets(this, dice, targets).await?;
                        targets.inputs()
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkFileSet::from)?)
    }

    /// Filter targets by rule type.
    /// Returns a subset of `targets` where the rule type matches the specified `regex`. The specified pattern can be a regular expression.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_kind(ctx):
    ///     kind = ctx.uquery().kind("cpp.*", "bin/libs/...")
    ///     ctx.output.print(nodes)
    /// ```
    fn kind<'v>(
        this: &'v StarlarkUQueryCtx<'v>,
        regex: &str,
        targets: TargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<TargetNode>> {
        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    let targets = unpack_targets(this, dice, targets).await?;
                    targets.kind(regex).map(StarlarkTargetSet::from)
                }
                .boxed_local()
            })
        })?)
    }

    /// The deps query for finding the transitive closure of dependencies.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_deps(ctx):
    ///     result = ctx.uquery().deps("root//bin:the_binary", 1)
    ///     ctx.output.print(result)
    /// ```
    fn deps<'v>(
        this: &StarlarkUQueryCtx<'v>,
        universe: TargetListExprArg<'v>,
        #[starlark(default = NoneOr::None)] depth: NoneOr<i32>,
        #[starlark(default = NoneOr::None)] filter: NoneOr<&'v str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<TargetNode>> {
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

                        get_uquery_env(&this.ctx, this.allow_partial_graph)
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

    /// The rdeps query for finding the transitive closure of reverse dependencies.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_rdeps(ctx):
    ///     result = ctx.uquery().rdeps("root//bin:the_binary", "//lib:file1", 100)
    ///     ctx.output.print(result)
    /// ```
    fn rdeps<'v>(
        this: &StarlarkUQueryCtx<'v>,
        universe: TargetListExprArg<'v>,
        from: TargetListExprArg<'v>,
        #[starlark(default = NoneOr::None)] depth: NoneOr<i32>,
        #[starlark(default = NoneOr::None)] filter: NoneOr<&'v str>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<TargetNode>> {
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

                        get_uquery_env(&this.ctx, this.allow_partial_graph)
                            .await?
                            .rdeps(
                                dice,
                                &universe,
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
    ///     result = ctx.uquery().filter(".*the_binary", "root//...")
    ///     ctx.output.print(result)
    /// ```
    fn filter<'v>(
        this: &StarlarkUQueryCtx<'v>,
        regex: &str,
        targets: TargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<TargetNode>> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let targets = unpack_targets(this, dice, targets).await?;
                        targets.filter_name(regex)
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkTargetSet::from)?)
    }

    /// The testsof query for listing the tests of the specified targets.
    ///
    /// Sample usage:
    /// ```python
    /// def _testsof_impl(ctx):
    ///     result = ctx.uquery().testsof("//:foo_lib")
    ///     ctx.output.print(result)
    /// ```
    fn testsof<'v>(
        this: &StarlarkUQueryCtx<'v>,
        targets: TargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<TargetNode>> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let targets = unpack_targets(this, dice, targets).await?;
                        get_uquery_env(&this.ctx, this.allow_partial_graph)
                            .await?
                            .testsof(dice, &targets)
                            .await
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkTargetSet::from)?)
    }

    /// Find the build file(s) that defines a target or a target set.
    ///
    /// Sample usage:
    /// ```python
    /// def _buildfile_impl(ctx):
    ///     owner = ctx.uquery().owner(["bin/TARGET", "bin/kind"])
    ///     result = ctx.uquery().buildfile(owner)
    ///     ctx.output.print(result)
    /// ```
    fn buildfile<'v>(
        this: &StarlarkUQueryCtx<'v>,
        targets: TargetListExprArg<'v>,
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

    /// The owner query for finding targets that own specified files. Note that if you do not pass in a cell
    /// path (where the format is `<cell>//path/to/file`), the path is resolved against the cell that the BXL
    /// script lives in. If you need to evaluate a file path that lives in a different cell, you must pass in
    /// the fully qualified cell path.
    ///
    /// Sample usage:
    /// ```python
    /// def _owner_impl(ctx):
    ///     owner = ctx.uquery().owner("bin/TARGETS.fixture")
    ///     ctx.output.print(owner)
    /// ```
    fn owner<'v>(
        this: &'v StarlarkUQueryCtx<'v>,
        files: FileSetExpr,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<TargetNode>> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        get_uquery_env(&this.ctx, this.allow_partial_graph)
                            .await?
                            .owner(dice, (files.get(&this.ctx).await?).as_ref())
                            .await
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkTargetSet::from)?)
    }

    /// Given a set of buildfiles, return all targets within those buildfiles.
    ///
    /// Usage:
    /// ```python
    /// def _targets_in_buildfile_impl(ctx):
    ///     targets = ctx.uquery().targets_in_buildfile("bin/TARGETS.fixture")
    ///     ctx.output.print(targets)
    /// ```
    ///
    /// This is subject to be removed in future in favor of a more general `targets_in_packages`.
    fn targets_in_buildfile<'v>(
        this: &'v StarlarkUQueryCtx<'v>,
        files: FileSetExpr,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<TargetNode>> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        get_uquery_env(&this.ctx, this.allow_partial_graph)
                            .await?
                            .targets_in_buildfile(dice, (files.get(&this.ctx).await?).as_ref())
                            .await
                    }
                    .boxed_local()
                })
            })
            .map(StarlarkTargetSet::from)?)
    }

    /// The allbuildfiles query for finding the build files of the given targets and their transitive imports.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_allbuildfiles(ctx):
    ///     result = ctx.uquery().allbuildfiles("root//bin:the_binary")
    ///     ctx.output.print(result)
    /// ```
    fn allbuildfiles<'v>(
        this: &StarlarkUQueryCtx<'v>,
        universe: TargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkFileSet> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        let universe = unpack_targets(this, dice, universe).await?;
                        get_uquery_env(&this.ctx, this.allow_partial_graph)
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
    ///     result = ctx.uquery().rbuildfiles("bin/TARGETS", "bin/defs.bzl")
    ///     ctx.output.print(result)
    /// ```
    fn rbuildfiles<'v>(
        this: &StarlarkUQueryCtx<'v>,
        universe: FileSetExpr,
        argset: FileSetExpr,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkFileSet> {
        Ok(this
            .ctx
            .via_dice(eval, |dice| {
                dice.via(|dice| {
                    async {
                        get_uquery_env(&this.ctx, this.allow_partial_graph)
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

    /// The attrregexfilter query for rule attribute filtering with regex.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_attrregexfilter(ctx):
    ///     filtered = ctx.uquery().attrregexfilter("foo", "he.lo", "bin/kind/...")
    ///     ctx.output.print(filtered)
    /// ```
    fn attrregexfilter<'v>(
        this: &StarlarkUQueryCtx<'v>,
        attribute: &str,
        value: &str,
        targets: TargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkTargetSet<TargetNode>> {
        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async {
                    let targets = unpack_targets(this, dice, targets).await?;
                    targets
                        .attrregexfilter(attribute, value)
                        .map(StarlarkTargetSet::from)
                }
                .boxed_local()
            })
        })?)
    }

    /// Evaluates some general query string, `query_args` can be a target_set of unconfigured nodes, or
    /// a list of strings. Returns a `dict` of target labels mapped to their `target_set` results if `query_args`
    /// was passed in, otherwise returns a single `target_set`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_eval(ctx):
    ///     result1 = ctx.uquery().eval("inputs(cell//path/to/file:target)")
    ///     ctx.output.print(result1)
    ///
    ///     result2 = ctx.uquery().eval("inputs(%s)", query_args = ["cell//path/to/file:target"])
    ///     ctx.output.print(result2)
    /// ```
    fn eval<'v>(
        this: &StarlarkUQueryCtx<'v>,
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
                            .eval_uquery(
                                dice,
                                &this.ctx.working_dir()?,
                                query,
                                &query_args,
                                this.allow_partial_graph,
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

#[derive(StarlarkTypeRepr, UnpackValue)]
pub(crate) enum UnpackUnconfiguredQueryArgs<'v> {
    ListOfStrings(ValueOf<'v, ListType<String>>),
    TargetSet(&'v StarlarkTargetSet<TargetNode>),
}

impl<'v> UnpackUnconfiguredQueryArgs<'v> {
    pub(crate) fn into_strings(self) -> buck2_error::Result<Vec<String>> {
        Ok(match self {
            UnpackUnconfiguredQueryArgs::ListOfStrings(list) => list.try_to_vec()?,
            UnpackUnconfiguredQueryArgs::TargetSet(set) => {
                // TODO - we really should change eval_query() to handle this, but escaping the unconfigured target label for now
                // as a quick solution.
                set.0.iter_names().map(|e| format!("\"{e}\"")).collect()
            }
        })
    }
}
