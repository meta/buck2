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
use buck2_build_api::query::bxl::BxlUqueryFunctions;
use buck2_build_api::query::bxl::NEW_BXL_UQUERY_FUNCTIONS;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::target_aliases::HasTargetAliasResolver;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_hash::BuckMutSet;
use buck2_interpreter_for_build::interpreter::context::HasInterpreterContext;
use buck2_node::nodes::unconfigured::TargetNode;
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

use crate::dice::DiceQueryData;
use crate::dice::DiceQueryDelegate;
use crate::uquery::environment::UqueryEnvironment;

fn uquery_functions<'v>() -> DefaultQueryFunctions<UqueryEnvironment<'v>> {
    DefaultQueryFunctions::new()
}

struct BxlUqueryFunctionsImpl {
    allow_partial_graph: bool,
    project_root: ProjectRoot,
    working_dir: ProjectRelativePathBuf,
}

impl BxlUqueryFunctionsImpl {
    async fn uquery_delegate<'c, 'd>(
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
            GlobalCfgOptions::default(),
            cell_resolver.dupe(),
            cell_alias_resolver,
            &self.working_dir,
            self.project_root.dupe(),
            target_alias_resolver,
            infer_target_names,
            self.allow_partial_graph,
        ));
        Ok(DiceQueryDelegate::new(dice, query_data))
    }

    async fn uquery_env<'c, 'd>(
        &self,
        delegate: &'c DiceQueryDelegate<'c, 'd>,
    ) -> buck2_error::Result<UqueryEnvironment<'c>> {
        let literals = delegate.query_data().dupe();
        Ok(UqueryEnvironment::new(
            delegate,
            literals,
            BuckMutSet::default(),
            self.allow_partial_graph,
        ))
    }
}

#[async_trait]
impl BxlUqueryFunctions for BxlUqueryFunctionsImpl {
    async fn allpaths(
        &self,
        dice: &mut DiceComputations<'_>,
        from: &TargetSet<TargetNode>,
        to: &TargetSet<TargetNode>,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<TargetNode>> {
        dice.with_linear_recompute(|dice| {
            async move {
                Ok(uquery_functions()
                    .allpaths(
                        &self.uquery_env(&self.uquery_delegate(dice).await?).await?,
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
        from: &TargetSet<TargetNode>,
        to: &TargetSet<TargetNode>,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<TargetNode>> {
        dice.with_linear_recompute(|dice| {
            async move {
                Ok(uquery_functions()
                    .somepath(
                        &self.uquery_env(&self.uquery_delegate(dice).await?).await?,
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
        targets: &TargetSet<TargetNode>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<TargetNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    uquery_functions()
                        .deps(
                            &self.uquery_env(&self.uquery_delegate(dice).await?).await?,
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
        universe: &TargetSet<TargetNode>,
        targets: &TargetSet<TargetNode>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<TargetNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    uquery_functions()
                        .rdeps(
                            &self.uquery_env(&self.uquery_delegate(dice).await?).await?,
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
        targets: &TargetSet<TargetNode>,
    ) -> buck2_error::Result<TargetSet<TargetNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    uquery_functions()
                        .testsof(
                            &self.uquery_env(&self.uquery_delegate(dice).await?).await?,
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
    ) -> buck2_error::Result<TargetSet<TargetNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    uquery_functions()
                        .owner(
                            &self.uquery_env(&self.uquery_delegate(dice).await?).await?,
                            file_set,
                        )
                        .await
                }
                .boxed()
            })
            .await?)
    }
    async fn targets_in_buildfile(
        &self,
        dice: &mut DiceComputations<'_>,
        file_set: &FileSet,
    ) -> buck2_error::Result<TargetSet<TargetNode>> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    uquery_functions()
                        .targets_in_buildfile(
                            &self.uquery_env(&self.uquery_delegate(dice).await?).await?,
                            file_set,
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
        universe: &TargetSet<TargetNode>,
    ) -> buck2_error::Result<FileSet> {
        Ok(dice
            .with_linear_recompute(|dice| {
                async move {
                    uquery_functions()
                        .allbuildfiles(
                            &self.uquery_env(&self.uquery_delegate(dice).await?).await?,
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
                    uquery_functions()
                        .rbuildfiles(
                            &self.uquery_env(&self.uquery_delegate(dice).await?).await?,
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

pub(crate) fn init_new_bxl_uquery_functions() {
    NEW_BXL_UQUERY_FUNCTIONS.init(
        |allow_partial_graph, project_root, cell_name, cell_resolver| {
            Box::pin(async move {
                let cell = cell_resolver.get(cell_name)?;
                // TODO(nga): working as as cell root is not right.
                //   Should be either the project root or user's current working directory.
                let working_dir = cell.path().as_project_relative_path().to_buf();

                Result::<Box<dyn BxlUqueryFunctions>, _>::Ok(Box::new(BxlUqueryFunctionsImpl {
                    allow_partial_graph,
                    project_root,
                    working_dir,
                }))
            })
        },
    )
}
