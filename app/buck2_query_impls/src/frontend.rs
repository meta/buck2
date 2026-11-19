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

use async_trait::async_trait;
use buck2_build_api::actions::query::ActionQueryNode;
use buck2_build_api::query::oneshot::QUERY_FRONTEND;
use buck2_build_api::query::oneshot::QueryFrontend;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_node::configured_universe::CqueryUniverse;
use buck2_node::configured_universe::UNIVERSE_FROM_LITERALS;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_query::query::syntax::simple::eval::values::QueryEvaluationResult;
use dice::DiceComputations;
use futures::FutureExt;

use crate::aquery::evaluator::get_aquery_evaluator;
use crate::cquery::evaluator::eval_cquery;
use crate::cquery::evaluator::preresolve_literals_and_build_universe;
use crate::dice::get_dice_query_delegate;
use crate::uquery::evaluator::get_uquery_evaluator;

struct QueryFrontendImpl;

pub(crate) fn init_query_frontend() {
    QUERY_FRONTEND.init(&QueryFrontendImpl);
}

#[async_trait]
impl QueryFrontend for QueryFrontendImpl {
    async fn eval_uquery(
        &self,
        ctx: &mut DiceComputations<'_>,
        working_dir: &ProjectRelativePath,
        query: &str,
        query_args: &[String],
        allow_partial_graph: bool,
    ) -> buck2_error::Result<QueryEvaluationResult<TargetNode>> {
        Ok(ctx
            .with_linear_recompute(|ctx| {
                async move {
                    let evaluator =
                        get_uquery_evaluator(ctx, working_dir, allow_partial_graph).await?;
                    evaluator.eval_query(query, query_args).await
                }
                .boxed()
            })
            .await?)
    }

    /// Evaluate a cquery query.
    ///
    /// Long with query results, the function returns all the universes
    /// that was used to resolve query literals in non-deterministic order.
    /// Universes are used in Starlark profiler.
    async fn eval_cquery(
        &self,
        ctx: &mut DiceComputations<'_>,
        working_dir: &ProjectRelativePath,
        query: &str,
        query_args: &[String],
        global_cfg_options: GlobalCfgOptions,
        target_universe: Option<&[String]>,
        collect_universes: bool,
    ) -> buck2_error::Result<(
        QueryEvaluationResult<ConfiguredTargetNode>,
        Option<Vec<Arc<CqueryUniverse>>>,
    )> {
        Ok(ctx
            .with_linear_recompute(|ctx| {
                async move {
                    let dice_query_delegate = get_dice_query_delegate(
                        ctx,
                        working_dir,
                        global_cfg_options,
                        false, // allow_partial_graph
                    )
                    .await?;

                    // TODO(nga): this should support configured target patterns
                    //   similarly to what we do for `build` command.
                    //   Something like this should work:
                    //   ```
                    //   buck2 cquery --target-universe android//:binary 'deps("some//:lib (<arm32>)")'
                    //   ```
                    eval_cquery(
                        dice_query_delegate,
                        query,
                        query_args,
                        target_universe.as_ref().map(|v| &v[..]),
                        collect_universes,
                    )
                    .await
                }
                .boxed()
            })
            .await?)
    }

    async fn eval_aquery(
        &self,
        ctx: &mut DiceComputations<'_>,
        working_dir: &ProjectRelativePath,
        query: &str,
        query_args: &[String],
        global_cfg_options: GlobalCfgOptions,
    ) -> buck2_error::Result<QueryEvaluationResult<ActionQueryNode>> {
        Ok(ctx
            .with_linear_recompute(|ctx| {
                async move {
                    let evaluator =
                        get_aquery_evaluator(ctx, working_dir, global_cfg_options).await?;
                    evaluator.eval_query(query, query_args).await
                }
                .boxed()
            })
            .await?)
    }
}

async fn universe_from_literals(
    ctx: &mut DiceComputations<'_>,
    cwd: &ProjectRelativePath,
    literals: &[String],
    global_cfg_options: GlobalCfgOptions,
) -> buck2_error::Result<CqueryUniverse> {
    ctx.with_linear_recompute(|ctx| {
        async move {
            let query_delegate = get_dice_query_delegate(
                ctx,
                cwd,
                global_cfg_options,
                false, // allow_partial_graph
            )
            .await?;
            Ok(preresolve_literals_and_build_universe(
                &query_delegate,
                query_delegate.query_data(),
                literals,
            )
            .await?
            .0)
        }
        .boxed()
    })
    .await
}

pub(crate) fn init_universe_from_literals() {
    UNIVERSE_FROM_LITERALS.init(
        |ctx: &mut DiceComputations<'_>,
         cwd: &ProjectRelativePath,
         literals: &[String],
         global_cfg_options: GlobalCfgOptions| {
            Box::pin(universe_from_literals(
                ctx,
                cwd,
                literals,
                global_cfg_options,
            ))
        },
    );
}
