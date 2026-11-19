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

use buck2_common::events::HasEvents;
use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::internal_error;
use buck2_events::dispatch::console_message;
use buck2_node::configured_universe::CqueryUniverse;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_query::query::syntax::simple::eval::values::QueryEvaluationResult;
use buck2_query::query::syntax::simple::functions::DefaultQueryFunctionsModule;
use dice::DiceComputations;
use dupe::Dupe;
use futures::StreamExt;
use futures::stream::FuturesUnordered;

use crate::analysis::evaluator::eval_query;
use crate::cquery::environment::CqueryEnvironment;
use crate::dice::DiceQueryData;
use crate::dice::DiceQueryDelegate;
use crate::uquery::environment::PreresolvedQueryLiterals;
use crate::uquery::environment::QueryLiterals;
use crate::uquery::environment::UqueryDelegate;

pub(crate) async fn eval_cquery(
    dice_query_delegate: DiceQueryDelegate<'_, '_>,
    query: &str,
    query_args: &[String],
    target_universe: Option<&[String]>,
    collect_universes: bool,
) -> buck2_error::Result<(
    QueryEvaluationResult<ConfiguredTargetNode>,
    Option<Vec<Arc<CqueryUniverse>>>,
)> {
    let dispatcher = dice_query_delegate
        .ctx()
        .per_transaction_data()
        .get_dispatcher()
        .dupe();
    let functions = DefaultQueryFunctionsModule::new();
    let dice_query_delegate = &dice_query_delegate;

    let target_universe = match target_universe {
        None => None,
        Some(target_universe) => Some(Arc::new(
            build_cquery_universe_from_literals(
                target_universe,
                dice_query_delegate.query_data(),
                &mut dice_query_delegate.ctx(),
            )
            .await?,
        )),
    };

    // Here we use queue to pass universes from `eval_query` callback to this function.
    // This is ugly, but I (Stiopa) cannot figure out how to do it in a better way,
    // without introducing a lot of complexity (generics, downcasting)
    // through query evaluation stack.
    let (universes_tx_value, universes_rx) = if collect_universes {
        let (universes_tx_value, universes_rx) = std::sync::mpsc::channel();
        (Some(universes_tx_value), Some(universes_rx))
    } else {
        (None, None)
    };

    if let (Some(target_universe), Some(universes_tx_value)) =
        (&target_universe, &universes_tx_value)
    {
        universes_tx_value
            .send(target_universe.dupe())
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
            .internal_error("Must be open")?;
    }

    let universes_tx = universes_tx_value.as_ref();

    let target_universe = &target_universe;

    let result = eval_query(
        dispatcher,
        &functions,
        query,
        query_args,
        |literals| async move {
            let (resolved_literals, universe) = match target_universe {
                None => {
                    if literals.is_empty() {
                        console_message(
                            "Query has no target literals and `--target-universe` is not specified.\n\
                        Such query is correct, but the result is always empty.\n\
                        Consider specifying `--target-universe` for this query\n\
                        or using `uquery` instead of `cquery`".to_owned());
                    }
                    // In the absence of a user-provided target universe, we use the target
                    // literals in the cquery as the universe.

                    let universe = build_cquery_universe_from_literals(
                        &literals,
                        dice_query_delegate.query_data(),
                        &mut dice_query_delegate.ctx(),
                    )
                        .await?;

                    let universe = Arc::new(universe);

                    if let Some(universes_tx) = universes_tx {
                        universes_tx.send(universe.dupe()).map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0)).internal_error("Must be open")?;
                    }

                    (
                        resolve_literals_in_universe(dice_query_delegate, &literals, &universe)
                            .await?,
                        universe,
                    )
                }
                Some(universe) => (
                    resolve_literals_in_universe(dice_query_delegate, &literals, universe)
                        .await?,
                    universe.dupe(),
                ),
            };
            Ok(CqueryEnvironment::new(
                dice_query_delegate,
                Arc::new(resolved_literals),
                Some(universe),
            ))
        },
    )
        .await?;

    drop(universes_tx_value);

    let universes = if let Some(universes_rx) = universes_rx {
        let universes: Vec<Arc<CqueryUniverse>> = universes_rx.try_iter().collect();
        match universes_rx.try_recv() {
            Ok(_) => return Err(internal_error!("tx must be closed at this moment")),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                return Err(internal_error!("tx must be closed at this moment"));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
        }
        Some(universes)
    } else {
        None
    };

    Ok((result, universes))
}

pub(crate) async fn preresolve_literals_and_build_universe(
    dice_query_delegate: &DiceQueryDelegate<'_, '_>,
    dice_query_data: &DiceQueryData,
    literals: &[String],
) -> buck2_error::Result<(
    CqueryUniverse,
    PreresolvedQueryLiterals<ConfiguredTargetNode>,
)> {
    let resolved_literals = PreresolvedQueryLiterals::pre_resolve(
        dice_query_data,
        literals,
        &mut dice_query_delegate.ctx(),
    )
    .await;
    let universe = CqueryUniverse::build(&resolved_literals.literals()?)?;
    Ok((universe, resolved_literals))
}

async fn build_cquery_universe_from_literals(
    universe: &[String],
    query_literals: &DiceQueryData,
    ctx: &mut DiceComputations<'_>,
) -> buck2_error::Result<CqueryUniverse> {
    let refs: Vec<_> = universe.iter().map(|v| v.as_str()).collect::<Vec<_>>();
    let universe_resolved = query_literals.eval_literals(&refs, ctx).await?;

    CqueryUniverse::build(&universe_resolved)
}

// This will first resolve the universe to configured nodes and then gather all
// the deps. From there, it resolves the literals to any matching nodes in the universe deps.
async fn resolve_literals_in_universe(
    dice_query_delegate: &DiceQueryDelegate<'_, '_>,
    literals: &[String],
    universe: &CqueryUniverse,
) -> buck2_error::Result<PreresolvedQueryLiterals<ConfiguredTargetNode>> {
    // TODO(cjhopman): We should probably also resolve the literals to TargetNode so that
    // we can get errors for packages or targets that don't exist or fail to load.

    // capture a reference so the ref can be moved into the future below.
    let universe_ref = &universe;

    // TODO(cjhopman): Using the default resolution for recursive literals is inefficient.
    // If we can have a package-trie or cellpath-trie we can do the resolution directly
    // against the universe.
    let resolution_futs: FuturesUnordered<_> = literals
        .iter()
        .map(|lit| async move {
            let result: buck2_error::Result<_> = try {
                let resolved_pattern = dice_query_delegate
                    .resolve_target_patterns(&[lit.as_str()])
                    .await?;
                universe_ref.get(&resolved_pattern)
            };

            (lit.to_owned(), result)
        })
        .collect();

    let resolved = resolution_futs.collect().await;
    Ok(PreresolvedQueryLiterals::new(resolved))
}
