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
use buck2_build_api::query::oneshot::QUERY_FRONTEND;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_query::query::syntax::simple::eval::values::QueryEvaluationResult;
use buck2_query::query::syntax::simple::eval::values::QueryValueDepth;
use buck2_query::query::syntax::simple::functions::helpers::CapturedExpr;
use dice::DiceComputations;
use starlark::values::Heap;
use starlark::values::Value;

use crate::bxl::starlark_defs::context::BxlContextCoreData;
use crate::bxl::starlark_defs::file_set::OwnedFileSetExpr;
use crate::bxl::starlark_defs::file_set::StarlarkFileSet;
use crate::bxl::starlark_defs::query_util::parse_query_evaluation_result;
use crate::bxl::starlark_defs::target_list_expr::OwnedTargetListExprArg;
use crate::bxl::starlark_defs::targetset::StarlarkTargetSet;
use crate::bxl::starlark_defs::uquery::get_uquery_env;

#[derive(Debug, Allocative)]
pub(crate) enum LazyUqueryOperation {
    TestsOf(OwnedTargetListExprArg),
    AllPaths {
        from: OwnedTargetListExprArg,
        to: OwnedTargetListExprArg,
        filter: Option<String>,
    },
    SomePath {
        from: OwnedTargetListExprArg,
        to: OwnedTargetListExprArg,
        filter: Option<String>,
    },
    AttrFilter {
        attr: String,
        value: String,
        targets: OwnedTargetListExprArg,
    },
    AttrRegexFilter {
        attr: String,
        value: String,
        targets: OwnedTargetListExprArg,
    },
    Inputs(OwnedTargetListExprArg),
    Kind {
        regex: String,
        targets: OwnedTargetListExprArg,
    },
    Deps {
        universe: OwnedTargetListExprArg,
        depth: QueryValueDepth,
        filter: Option<String>,
    },
    Rdeps {
        universe: OwnedTargetListExprArg,
        from: OwnedTargetListExprArg,
        depth: QueryValueDepth,
        filter: Option<String>,
    },
    Filter {
        regex: String,
        targets: OwnedTargetListExprArg,
    },
    Buildfile(OwnedTargetListExprArg),
    AllBuildfiles(OwnedTargetListExprArg),
    Rbuildfiles {
        universe: OwnedFileSetExpr,
        argset: OwnedFileSetExpr,
    },
    Owner {
        files: OwnedFileSetExpr,
    },
    TargetsInBuildfile {
        files: OwnedFileSetExpr,
    },
    Eval {
        query: String,
        query_args: Vec<String>,
    },
}

pub(crate) enum LazyUqueryResult {
    TestsOf(StarlarkTargetSet<TargetNode>),
    AllPaths(StarlarkTargetSet<TargetNode>),
    SomePath(StarlarkTargetSet<TargetNode>),
    AttrFilter(StarlarkTargetSet<TargetNode>),
    AttrRegexFilter(StarlarkTargetSet<TargetNode>),
    Inputs(StarlarkFileSet),
    Kind(StarlarkTargetSet<TargetNode>),
    Deps(StarlarkTargetSet<TargetNode>),
    Rdeps(StarlarkTargetSet<TargetNode>),
    Filter(StarlarkTargetSet<TargetNode>),
    Buildfile(StarlarkFileSet),
    AllBuildfiles(StarlarkFileSet),
    Rbuildfiles(StarlarkFileSet),
    Owner(StarlarkTargetSet<TargetNode>),
    TargetsInBuildfile(StarlarkTargetSet<TargetNode>),
    Eval(QueryEvaluationResult<TargetNode>),
}

impl LazyUqueryResult {
    pub(crate) fn into_value<'v>(self, heap: Heap<'v>) -> buck2_error::Result<Value<'v>> {
        match self {
            LazyUqueryResult::TestsOf(target_set) => {
                Ok(heap.alloc_representation(target_set).to_value())
            }
            LazyUqueryResult::AllPaths(target_set) => {
                Ok(heap.alloc_representation(target_set).to_value())
            }
            LazyUqueryResult::SomePath(target_set) => {
                Ok(heap.alloc_representation(target_set).to_value())
            }
            LazyUqueryResult::AttrFilter(target_set) => {
                Ok(heap.alloc_representation(target_set).to_value())
            }
            LazyUqueryResult::AttrRegexFilter(target_set) => {
                Ok(heap.alloc_representation(target_set).to_value())
            }
            LazyUqueryResult::Inputs(file_set) => {
                Ok(heap.alloc_representation(file_set).to_value())
            }
            LazyUqueryResult::Kind(target_set) => {
                Ok(heap.alloc_representation(target_set).to_value())
            }
            LazyUqueryResult::Deps(target_set) => {
                Ok(heap.alloc_representation(target_set).to_value())
            }
            LazyUqueryResult::Rdeps(target_set) => {
                Ok(heap.alloc_representation(target_set).to_value())
            }
            LazyUqueryResult::Filter(target_set) => {
                Ok(heap.alloc_representation(target_set).to_value())
            }
            LazyUqueryResult::Buildfile(file_set) => {
                Ok(heap.alloc_representation(file_set).to_value())
            }
            LazyUqueryResult::AllBuildfiles(file_set) => {
                Ok(heap.alloc_representation(file_set).to_value())
            }
            LazyUqueryResult::Rbuildfiles(file_set) => {
                Ok(heap.alloc_representation(file_set).to_value())
            }
            LazyUqueryResult::Owner(target_set) => {
                Ok(heap.alloc_representation(target_set).to_value())
            }
            LazyUqueryResult::TargetsInBuildfile(target_set) => {
                Ok(heap.alloc_representation(target_set).to_value())
            }
            LazyUqueryResult::Eval(result) => parse_query_evaluation_result(result, heap),
        }
    }
}

impl LazyUqueryOperation {
    pub(crate) async fn resolve(
        &self,
        dice: &mut DiceComputations<'_>,
        core_data: &BxlContextCoreData,
        allow_partial_graph: bool,
    ) -> buck2_error::Result<LazyUqueryResult> {
        match self {
            LazyUqueryOperation::TestsOf(expr) => {
                let target_set = expr.to_unconfigured_target_set(core_data, dice).await?;

                let res = get_uquery_env(core_data, allow_partial_graph)
                    .await?
                    .testsof(dice, &target_set)
                    .await?;

                Ok(LazyUqueryResult::TestsOf(StarlarkTargetSet::from(res)))
            }
            LazyUqueryOperation::AllPaths { from, to, filter } => {
                let from = from.to_unconfigured_target_set(core_data, dice).await?;
                let to = to.to_unconfigured_target_set(core_data, dice).await?;
                let filter = filter
                    .as_ref()
                    .map(|s| buck2_query_parser::parse_expr(s.as_str()))
                    .transpose()?;
                let expr = filter.as_ref().map(|expr| CapturedExpr { expr });

                let res = get_uquery_env(core_data, allow_partial_graph)
                    .await?
                    .allpaths(dice, &from, &to, expr.as_ref())
                    .await?;

                Ok(LazyUqueryResult::AllPaths(StarlarkTargetSet::from(res)))
            }
            LazyUqueryOperation::SomePath { from, to, filter } => {
                let from = from.to_unconfigured_target_set(core_data, dice).await?;
                let to = to.to_unconfigured_target_set(core_data, dice).await?;
                let filter = filter
                    .as_ref()
                    .map(|s| buck2_query_parser::parse_expr(s.as_str()))
                    .transpose()?;
                let expr = filter.as_ref().map(|expr| CapturedExpr { expr });

                let res = get_uquery_env(core_data, allow_partial_graph)
                    .await?
                    .somepath(dice, &from, &to, expr.as_ref())
                    .await?;

                Ok(LazyUqueryResult::SomePath(StarlarkTargetSet::from(res)))
            }
            LazyUqueryOperation::AttrFilter {
                attr,
                value,
                targets,
            } => {
                let target_set = targets.to_unconfigured_target_set(core_data, dice).await?;

                let res = target_set.attrfilter(attr, &|v| Ok(v == value))?;

                Ok(LazyUqueryResult::AttrFilter(StarlarkTargetSet::from(res)))
            }
            LazyUqueryOperation::AttrRegexFilter {
                attr,
                value,
                targets,
            } => {
                let target_set = targets.to_unconfigured_target_set(core_data, dice).await?;

                let res = target_set.attrregexfilter(attr, value)?;

                Ok(LazyUqueryResult::AttrRegexFilter(StarlarkTargetSet::from(
                    res,
                )))
            }
            LazyUqueryOperation::Inputs(expr) => {
                let target_set = expr.to_unconfigured_target_set(core_data, dice).await?;

                let res = target_set.inputs()?;

                Ok(LazyUqueryResult::Inputs(StarlarkFileSet::from(res)))
            }
            LazyUqueryOperation::Kind { regex, targets } => {
                let target_set = targets.to_unconfigured_target_set(core_data, dice).await?;

                let res = target_set.kind(regex)?;

                Ok(LazyUqueryResult::Kind(StarlarkTargetSet::from(res)))
            }
            LazyUqueryOperation::Deps {
                universe,
                depth,
                filter,
            } => {
                let target_set = universe.to_unconfigured_target_set(core_data, dice).await?;
                let filter = filter
                    .as_ref()
                    .map(|s| buck2_query_parser::parse_expr(s.as_str()))
                    .transpose()?;
                let expr = filter.as_ref().map(|expr| CapturedExpr { expr });

                let res = get_uquery_env(core_data, allow_partial_graph)
                    .await?
                    .deps(dice, &target_set, *depth, expr.as_ref())
                    .await?;

                Ok(LazyUqueryResult::Deps(StarlarkTargetSet::from(res)))
            }
            LazyUqueryOperation::Rdeps {
                universe,
                from,
                depth,
                filter,
            } => {
                let target_set = universe.to_unconfigured_target_set(core_data, dice).await?;
                let from_set = from.to_unconfigured_target_set(core_data, dice).await?;
                let filter = filter
                    .as_ref()
                    .map(|s| buck2_query_parser::parse_expr(s.as_str()))
                    .transpose()?;
                let expr = filter.as_ref().map(|expr| CapturedExpr { expr });

                let res = get_uquery_env(core_data, allow_partial_graph)
                    .await?
                    .rdeps(dice, &target_set, &from_set, *depth, expr.as_ref())
                    .await?;

                Ok(LazyUqueryResult::Rdeps(StarlarkTargetSet::from(res)))
            }
            LazyUqueryOperation::Filter { regex, targets } => {
                let target_set = targets.to_unconfigured_target_set(core_data, dice).await?;

                let res = target_set.filter_name(regex)?;

                Ok(LazyUqueryResult::Filter(StarlarkTargetSet::from(res)))
            }
            LazyUqueryOperation::Buildfile(expr) => {
                let target_set = expr.to_unconfigured_target_set(core_data, dice).await?;

                let res = target_set.buildfile();

                Ok(LazyUqueryResult::Buildfile(StarlarkFileSet::from(res)))
            }
            LazyUqueryOperation::AllBuildfiles(expr) => {
                let target_set = expr.to_unconfigured_target_set(core_data, dice).await?;

                let res = get_uquery_env(core_data, allow_partial_graph)
                    .await?
                    .allbuildfiles(dice, &target_set)
                    .await?;

                Ok(LazyUqueryResult::AllBuildfiles(StarlarkFileSet::from(res)))
            }
            LazyUqueryOperation::Rbuildfiles { universe, argset } => {
                let universe = universe.get(core_data)?;
                let argset = argset.get(core_data)?;

                let res = get_uquery_env(core_data, allow_partial_graph)
                    .await?
                    .rbuildfiles(dice, &universe, &argset)
                    .await?;

                Ok(LazyUqueryResult::Rbuildfiles(StarlarkFileSet::from(res)))
            }
            LazyUqueryOperation::Owner { files } => {
                let file_set = files.get(core_data)?;

                let res = get_uquery_env(core_data, allow_partial_graph)
                    .await?
                    .owner(dice, &file_set)
                    .await?;

                Ok(LazyUqueryResult::Owner(StarlarkTargetSet::from(res)))
            }
            LazyUqueryOperation::TargetsInBuildfile { files } => {
                let file_set = files.get(core_data)?;

                let res = get_uquery_env(core_data, allow_partial_graph)
                    .await?
                    .targets_in_buildfile(dice, &file_set)
                    .await?;

                Ok(LazyUqueryResult::TargetsInBuildfile(
                    StarlarkTargetSet::from(res),
                ))
            }
            LazyUqueryOperation::Eval { query, query_args } => {
                let res = QUERY_FRONTEND
                    .get()?
                    .eval_uquery(
                        dice,
                        &core_data.working_dir()?,
                        query,
                        query_args,
                        allow_partial_graph,
                    )
                    .await?;

                Ok(LazyUqueryResult::Eval(res))
            }
        }
    }
}
