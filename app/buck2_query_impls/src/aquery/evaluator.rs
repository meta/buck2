/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Implementation of the cli and query_* attr query language.
use std::sync::Arc;

use buck2_build_api::actions::query::ActionQueryNode;
use buck2_common::events::HasEvents;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_query::query::syntax::simple::eval::values::QueryEvaluationResult;
use dice::LinearRecomputeDiceComputations;
use dupe::Dupe;

use crate::analysis::evaluator::eval_query;
use crate::aquery::environment::AqueryDelegate;
use crate::aquery::environment::AqueryEnvironment;
use crate::aquery::functions::aquery_functions;
use crate::dice::aquery::DiceAqueryDelegate;
use crate::dice::get_dice_query_delegate;
use crate::uquery::environment::PreresolvedQueryLiterals;

pub(crate) struct AqueryEvaluator<'c, 'd> {
    dice_query_delegate: Arc<DiceAqueryDelegate<'c, 'd>>,
}

impl AqueryEvaluator<'_, '_> {
    pub(crate) async fn eval_query(
        &self,
        query: &str,
        query_args: &[String],
    ) -> buck2_error::Result<QueryEvaluationResult<ActionQueryNode>> {
        let functions = aquery_functions();

        eval_query(
            self.dice_query_delegate
                .ctx()
                .per_transaction_data()
                .get_dispatcher()
                .dupe(),
            &functions,
            query,
            query_args,
            |literals| async move {
                let resolved_literals = PreresolvedQueryLiterals::pre_resolve(
                    &**self.dice_query_delegate.query_data(),
                    &literals,
                    &mut self.dice_query_delegate.ctx(),
                )
                .await;
                Ok(AqueryEnvironment::new(
                    self.dice_query_delegate.dupe(),
                    Arc::new(resolved_literals),
                ))
            },
        )
        .await
    }
}

/// Evaluates some query expression. TargetNodes are resolved via the interpreter from
/// the provided DiceCtx.
pub(crate) async fn get_aquery_evaluator<'a, 'c: 'a, 'd>(
    ctx: LinearRecomputeDiceComputations<'c, 'd>,
    working_dir: &'a ProjectRelativePath,
    global_cfg_options: GlobalCfgOptions,
) -> buck2_error::Result<AqueryEvaluator<'c, 'd>> {
    let dice_query_delegate =
        get_dice_aquery_delegate(ctx, working_dir, global_cfg_options).await?;
    Ok(AqueryEvaluator {
        dice_query_delegate,
    })
}

// Provides the dice query delegate for aquery evaluator
pub(crate) async fn get_dice_aquery_delegate<'a, 'c: 'a, 'd>(
    ctx: LinearRecomputeDiceComputations<'c, 'd>,
    working_dir: &'a ProjectRelativePath,
    global_cfg_options: GlobalCfgOptions,
) -> buck2_error::Result<Arc<DiceAqueryDelegate<'c, 'd>>> {
    let dice_query_delegate = get_dice_query_delegate(
        ctx,
        working_dir,
        global_cfg_options,
        false, // allow_partial_graph
    )
    .await?;
    let dice_query_delegate = Arc::new(DiceAqueryDelegate::new(dice_query_delegate).await?);
    Ok(dice_query_delegate)
}
