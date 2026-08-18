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
use buck2_build_api::query::bxl::BxlCqueryFunctions;
use buck2_build_api::query::bxl::NEW_BXL_CQUERY_FUNCTIONS;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::target_aliases::HasTargetAliasResolver;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_interpreter_for_build::interpreter::context::HasInterpreterContext;
use buck2_node::configured_universe::CqueryUniverse;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_query::query::syntax::simple::eval::file_set::FileSet;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_query::query::syntax::simple::eval::values::QueryValueDepth;
use buck2_query::query::syntax::simple::functions::DefaultQueryFunctions;
use buck2_query::query::syntax::simple::functions::DefaultQueryFunctionsModule;
use buck2_query::query::syntax::simple::functions::helpers::CapturedExpr;
use dice::DiceComputations;
use dice::LinearRecomputeDiceComputations;
use dupe::Dupe;
use futures::FutureExt;

use crate::cquery::environment::CqueryEnvironment;
use crate::dice::DiceQueryData;
use crate::dice::DiceQueryDelegate;

fn cquery_functions<'v>() -> DefaultQueryFunctions<CqueryEnvironment<'v>> {
    DefaultQueryFunctions::new()
}

struct BxlCqueryFunctionsImpl {
    global_cfg_options: GlobalCfgOptions,
    project_root: ProjectRoot,
    working_dir: ProjectRelativePathBuf,
}

impl BxlCqueryFunctionsImpl {
    async fn setup_dice_query_delegate<'c, 'd>(
        &self,
        dice: LinearRecomputeDiceComputations<'c, 'd>,
    ) -> buck2_error::Result<DiceQueryDelegate<'c, 'd>> {
        let cell_resolver = dice.get().get_cell_resolver().await?;
        let cell_alias_resolver = dice
            .get()
            .get_cell_alias_resolver_for_dir(&self.working_dir)
            .await?
            .dupe();

        let target_alias_resolver = dice.get().target_alias_resolver().await?.dupe();
        let infer_target_names = dice
            .get()
            .get_interpreter_configuror()
            .await?
            .infer_target_names();

        let query_data = Arc::new(DiceQueryData::new(
            self.global_cfg_options.dupe(),
            cell_resolver.dupe(),
            cell_alias_resolver,
            &self.working_dir,
            self.project_root.dupe(),
            target_alias_resolver,
            infer_target_names,
            false, // allow_partial_graph
        ));

        Ok(DiceQueryDelegate::new(dice, query_data.dupe()))
    }

    async fn cquery_env<'c, 'd>(
        &self,
        dice_query_delegate: &'c DiceQueryDelegate<'c, 'd>,
        universe: Option<&TargetSet<ConfiguredTargetNode>>,
    ) -> buck2_error::Result<CqueryEnvironment<'c>> {
        let universe = match universe {
            Some(u) => Some(Arc::new(CqueryUniverse::build(u)?)),
            None => None,
        };
        let literals = dice_query_delegate.query_data().dupe();
        Ok(CqueryEnvironment::new(
            dice_query_delegate,
            literals,
            universe,
        ))
    }
}

#[async_trait]
impl BxlCqueryFunctions for BxlCqueryFunctionsImpl {
    async fn allpaths(
        &self,
        dice: &mut DiceComputations<'_>,
        from: &TargetSet<ConfiguredTargetNode>,
        to: &TargetSet<ConfiguredTargetNode>,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    buck2_error::Ok(
                        cquery_functions()
                            .allpaths(
                                &self
                                    .cquery_env(&self.setup_dice_query_delegate(dice).await?, None)
                                    .await?,
                                &DefaultQueryFunctionsModule::new(),
                                from,
                                to,
                                captured_expr,
                            )
                            .await?,
                    )
                }
                .boxed()
            })
            .await?)
    }

    async fn somepath(
        &self,
        dice: &mut DiceComputations<'_>,
        from: &TargetSet<ConfiguredTargetNode>,
        to: &TargetSet<ConfiguredTargetNode>,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    buck2_error::Ok(
                        cquery_functions()
                            .somepath(
                                &self
                                    .cquery_env(&self.setup_dice_query_delegate(dice).await?, None)
                                    .await?,
                                &DefaultQueryFunctionsModule::new(),
                                from,
                                to,
                                captured_expr,
                            )
                            .await?,
                    )
                }
                .boxed()
            })
            .await?)
    }

    async fn owner(
        &self,
        dice: &mut DiceComputations<'_>,
        file_set: &FileSet,
        target_universe: Option<&TargetSet<ConfiguredTargetNode>>,
    ) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    let query_delegate = self.setup_dice_query_delegate(dice).await?;
                    let cquery_env = self.cquery_env(&query_delegate, target_universe).await?;
                    cquery_functions().owner(&cquery_env, file_set).await
                }
                .boxed()
            })
            .await?)
    }

    async fn deps(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<ConfiguredTargetNode>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    cquery_functions()
                        .deps(
                            &self
                                .cquery_env(&self.setup_dice_query_delegate(dice).await?, None)
                                .await?,
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
        universe: &TargetSet<ConfiguredTargetNode>,
        targets: &TargetSet<ConfiguredTargetNode>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    cquery_functions()
                        .rdeps(
                            &self
                                .cquery_env(&self.setup_dice_query_delegate(dice).await?, None)
                                .await?,
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
        targets: &TargetSet<ConfiguredTargetNode>,
    ) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    cquery_functions()
                        .testsof(
                            &self
                                .cquery_env(&self.setup_dice_query_delegate(dice).await?, None)
                                .await?,
                            targets,
                        )
                        .await
                }
                .boxed()
            })
            .await?)
    }

    async fn testsof_with_default_target_platform(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<ConfiguredTargetNode>,
    ) -> buck2_error::Result<Vec<MaybeCompatible<ConfiguredTargetNode>>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    cquery_functions()
                        .testsof_with_default_target_platform(
                            &self
                                .cquery_env(&self.setup_dice_query_delegate(dice).await?, None)
                                .await?,
                            targets,
                        )
                        .await
                }
                .boxed()
            })
            .await?)
    }

    async fn allbuildfiles(
        &self,
        dice: &mut DiceComputations<'_>,
        universe: &TargetSet<ConfiguredTargetNode>,
    ) -> buck2_error::Result<FileSet> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    cquery_functions()
                        .allbuildfiles(
                            &self
                                .cquery_env(&self.setup_dice_query_delegate(dice).await?, None)
                                .await?,
                            universe,
                        )
                        .await
                }
                .boxed()
            })
            .await?)
    }

    async fn rbuildfiles(
        &self,
        dice: &mut DiceComputations<'_>,
        universe: &FileSet,
        argset: &FileSet,
    ) -> buck2_error::Result<FileSet> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    cquery_functions()
                        .rbuildfiles(
                            &self
                                .cquery_env(&self.setup_dice_query_delegate(dice).await?, None)
                                .await?,
                            universe,
                            argset,
                        )
                        .await
                }
                .boxed()
            })
            .await?)
    }
}

pub(crate) fn init_new_bxl_cquery_functions() {
    NEW_BXL_CQUERY_FUNCTIONS.init(
        |global_cfg_options: GlobalCfgOptions, project_root, cell_name, cell_resolver| {
            Box::pin(async move {
                let cell = cell_resolver.get(cell_name)?;
                // TODO(nga): working as as cell root is not right.
                //   Should be either the project root or user's current working directory.
                let working_dir = cell.path().as_project_relative_path().to_buf();

                Result::<Box<dyn BxlCqueryFunctions>, _>::Ok(Box::new(BxlCqueryFunctionsImpl {
                    global_cfg_options,
                    project_root,
                    working_dir,
                }))
            })
        },
    )
}
