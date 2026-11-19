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

use buck2_build_api::analysis::calculation::EVAL_ANALYSIS_QUERY;
use buck2_hash::BuckMutMap;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured_ref::ConfiguredGraphNodeRef;
use buck2_query::query::syntax::simple::eval::evaluator::QueryEvaluator;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use dice::DiceComputations;
use futures::FutureExt;

use crate::analysis::configured_graph::AnalysisConfiguredGraphQueryDelegate;
use crate::analysis::configured_graph::AnalysisDiceQueryDelegate;
use crate::analysis::environment::ConfiguredGraphQueryEnvironment;

pub(crate) fn init_eval_analysis_query() {
    EVAL_ANALYSIS_QUERY.init(|ctx, query, resolved_literals| {
        Box::pin(eval_analysis_query(ctx, query, resolved_literals))
    });
}

async fn eval_analysis_query(
    ctx: &mut DiceComputations<'_>,
    query: &str,
    resolved_literals: BuckMutMap<String, ConfiguredTargetNode>,
) -> buck2_error::Result<TargetSet<ConfiguredGraphNodeRef>> {
    ctx.with_linear_recompute(|ctx| {
        async move {
            let dice_query_delegate = Arc::new(AnalysisDiceQueryDelegate { ctx });
            let delegate = AnalysisConfiguredGraphQueryDelegate {
                dice_query_delegate,
                resolved_literals,
            };

            let functions = ConfiguredGraphQueryEnvironment::functions();
            let env = ConfiguredGraphQueryEnvironment::new(&delegate);
            let evaluator = QueryEvaluator::new(&env, &functions);

            let result = evaluator.eval_query(query).await?;
            result.try_into_targets()
        }
        .boxed()
    })
    .await
}
