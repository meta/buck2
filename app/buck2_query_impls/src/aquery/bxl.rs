/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use buck2_artifact::actions::key::ActionKey;
use buck2_build_api::actions::query::ActionQueryNode;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::query::bxl::BxlAqueryFunctions;
use buck2_build_api::query::bxl::NEW_BXL_AQUERY_FUNCTIONS;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::target_aliases::HasTargetAliasResolver;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_query::query::syntax::simple::eval::file_set::FileSet;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_query::query::syntax::simple::eval::values::QueryValue;
use buck2_query::query::syntax::simple::eval::values::QueryValueDepth;
use buck2_query::query::syntax::simple::functions::DefaultQueryFunctions;
use buck2_query::query::syntax::simple::functions::DefaultQueryFunctionsModule;
use buck2_query::query::syntax::simple::functions::helpers::CapturedExpr;
use buck2_util::future::try_join_all;
use dice::DiceComputations;
use dice::LinearRecomputeDiceComputations;
use dupe::Dupe;
use futures::FutureExt;
use itertools::Either;

use crate::aquery::environment::AqueryDelegate;
use crate::aquery::environment::AqueryEnvironment;
use crate::aquery::functions::AqueryFunctions;
use crate::dice::DiceQueryData;
use crate::dice::DiceQueryDelegate;
use crate::dice::aquery::DiceAqueryDelegate;

fn aquery_functions<'v>() -> DefaultQueryFunctions<AqueryEnvironment<'v>> {
    DefaultQueryFunctions::new()
}

fn special_aquery_functions<'v>() -> AqueryFunctions<'v> {
    AqueryFunctions(PhantomData)
}

struct BxlAqueryFunctionsImpl {
    global_cfg_options: GlobalCfgOptions,
    project_root: ProjectRoot,
    working_dir: ProjectRelativePathBuf,
}

impl BxlAqueryFunctionsImpl {
    async fn aquery_delegate<'c, 'd>(
        &self,
        dice: LinearRecomputeDiceComputations<'c, 'd>,
    ) -> buck2_error::Result<Arc<DiceAqueryDelegate<'c, 'd>>> {
        let cell_resolver = dice.get().get_cell_resolver().await?;
        let cell_alias_resolver = dice
            .get()
            .get_cell_alias_resolver_for_dir(&self.working_dir)
            .await?;

        let target_alias_resolver = dice.get().target_alias_resolver().await?.dupe();

        let query_data = Arc::new(DiceQueryData::new(
            self.global_cfg_options.dupe(),
            cell_resolver.dupe(),
            cell_alias_resolver.dupe(),
            &self.working_dir,
            self.project_root.dupe(),
            target_alias_resolver,
            false, // allow_partial_graph
        ));
        let query_delegate = DiceQueryDelegate::new(dice, query_data);

        Ok(Arc::new(DiceAqueryDelegate::new(query_delegate).await?))
    }

    async fn aquery_env<'c, 'd>(
        &self,
        delegate: &Arc<DiceAqueryDelegate<'c, 'd>>,
    ) -> buck2_error::Result<AqueryEnvironment<'c>> {
        let literals = delegate.query_data().dupe();
        Ok(AqueryEnvironment::new(delegate.dupe(), literals))
    }
}

#[async_trait]
impl BxlAqueryFunctions for BxlAqueryFunctionsImpl {
    async fn allpaths(
        &self,
        dice: &mut DiceComputations<'_>,
        from: &TargetSet<ActionQueryNode>,
        to: &TargetSet<ActionQueryNode>,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
        dice.with_linear_recompute(|dice| {
            async move {
                Ok(aquery_functions()
                    .allpaths(
                        &self.aquery_env(&self.aquery_delegate(dice).await?).await?,
                        &DefaultQueryFunctionsModule::new(),
                        from,
                        to,
                        captured_expr,
                    )
                    .await?)
            }
            .boxed()
        })
        .await
    }
    async fn somepath(
        &self,
        dice: &mut DiceComputations<'_>,
        from: &TargetSet<ActionQueryNode>,
        to: &TargetSet<ActionQueryNode>,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
        dice.with_linear_recompute(|dice| {
            async move {
                Ok(aquery_functions()
                    .somepath(
                        &self.aquery_env(&self.aquery_delegate(dice).await?).await?,
                        &DefaultQueryFunctionsModule::new(),
                        from,
                        to,
                        captured_expr,
                    )
                    .await?)
            }
            .boxed()
        })
        .await
    }
    async fn deps(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<ActionQueryNode>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    aquery_functions()
                        .deps(
                            &self.aquery_env(&self.aquery_delegate(dice).await?).await?,
                            &DefaultQueryFunctionsModule::new(),
                            targets,
                            depth,
                            captured_expr,
                        )
                        .await
                }
                .boxed()
            })
            .await?)
    }
    async fn rdeps(
        &self,
        dice: &mut DiceComputations<'_>,
        universe: &TargetSet<ActionQueryNode>,
        targets: &TargetSet<ActionQueryNode>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    aquery_functions()
                        .rdeps(
                            &self.aquery_env(&self.aquery_delegate(dice).await?).await?,
                            &DefaultQueryFunctionsModule::new(),
                            universe,
                            targets,
                            depth,
                            captured_expr,
                        )
                        .await
                }
                .boxed()
            })
            .await?)
    }
    async fn testsof(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<ActionQueryNode>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    aquery_functions()
                        .testsof(
                            &self.aquery_env(&self.aquery_delegate(dice).await?).await?,
                            targets,
                        )
                        .await
                }
                .boxed()
            })
            .await?)
    }
    async fn owner(
        &self,
        dice: &mut DiceComputations<'_>,
        file_set: &FileSet,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    aquery_functions()
                        .owner(
                            &self.aquery_env(&self.aquery_delegate(dice).await?).await?,
                            file_set,
                        )
                        .await
                }
                .boxed()
            })
            .await?)
    }

    async fn get_target_set(
        &self,
        dice: &mut DiceComputations<'_>,
        configured_labels: Vec<ConfiguredProvidersLabel>,
    ) -> buck2_error::Result<(Vec<ConfiguredTargetLabel>, TargetSet<ActionQueryNode>)> {
        let target_sets = dice
            .try_compute_join(
                configured_labels,
                async |ctx: &mut DiceComputations,
                       label: ConfiguredProvidersLabel|
                       -> buck2_error::Result<
                    Either<ConfiguredTargetLabel, TargetSet<ActionQueryNode>>,
                > {
                    let maybe_result = ctx.get_analysis_result(label.target()).await.ok()?;

                    match maybe_result {
                        MaybeCompatible::Incompatible(reason) => {
                            // Aquery skips incompatible targets by default on the CLI, but let's at least
                            // log the error messages to BXL's stderr
                            Ok(Either::Left(reason.target.dupe()))
                        }
                        MaybeCompatible::Compatible(result) => {
                            ctx.with_linear_recompute(|ctx| {
                                async move {
                                    let delegate = &self.aquery_delegate(ctx).await?;
                                    let target_set = delegate
                                        .get_target_set_from_analysis(&label, result.dupe())
                                        .await?;
                                    Ok(Either::Right(target_set))
                                }
                                .boxed()
                            })
                            .await
                        }
                    }
                },
            )
            .await?;

        let mut result = TargetSet::new();
        let mut incompatible_targets = Vec::new();
        target_sets.into_iter().for_each(|t| match t {
            Either::Left(incompatible) => incompatible_targets.push(incompatible),
            Either::Right(compatible) => result.extend(&compatible),
        });

        Ok((incompatible_targets, result))
    }

    async fn all_outputs(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<ActionQueryNode>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
        dice.with_linear_recompute(|dice| {
            async move {
                let query_val = special_aquery_functions()
                    .all_outputs(
                        &self.aquery_env(&self.aquery_delegate(dice).await?).await?,
                        targets.clone(),
                    )
                    .await?;

                match &query_val {
                    QueryValue::TargetSet(s) => Ok(s.clone()),
                    _ => unreachable!("all_outputs should always return target set"),
                }
            }
            .boxed()
        })
        .await
    }

    async fn all_actions(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<ActionQueryNode>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
        dice.with_linear_recompute(|dice| {
            async move {
                let query_val = special_aquery_functions()
                    .all_actions(
                        &self.aquery_env(&self.aquery_delegate(dice).await?).await?,
                        targets.clone(),
                    )
                    .await?;

                match &query_val {
                    QueryValue::TargetSet(s) => Ok(s.clone()),
                    _ => unreachable!("all_actions should always return target set"),
                }
            }
            .boxed()
        })
        .await
    }

    async fn get_action_nodes(
        &self,
        dice: &mut DiceComputations<'_>,
        action_keys: Vec<ActionKey>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
        dice.with_linear_recompute(|dice| {
            async move {
                let delegate = self.aquery_delegate(dice).await?;
                let nodes =
                    try_join_all(action_keys.iter().map(|key| delegate.get_action_node(key)))
                        .await?;
                Ok(nodes.into_iter().collect())
            }
            .boxed()
        })
        .await
    }
}

pub(crate) fn init_new_bxl_aquery_functions() {
    NEW_BXL_AQUERY_FUNCTIONS.init(
        |global_cfg_options, project_root, cell_name, cell_resolver| {
            Box::pin(async move {
                let cell = cell_resolver.get(cell_name)?;
                let working_dir = cell.path().as_project_relative_path().to_buf();

                Result::<Box<dyn BxlAqueryFunctions>, _>::Ok(Box::new(BxlAqueryFunctionsImpl {
                    global_cfg_options: global_cfg_options.dupe(),
                    project_root,
                    working_dir,
                }))
            })
        },
    )
}
