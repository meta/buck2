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
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_node::configured_universe::CqueryUniverse;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_query::query::syntax::simple::eval::values::QueryEvaluationResult;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;

use crate::actions::query::ActionQueryNode;

#[async_trait]
pub trait QueryFrontend: Send + Sync + 'static {
    async fn eval_uquery(
        &self,
        ctx: &mut DiceComputations<'_>,
        working_dir: &ProjectRelativePath,
        query: &str,
        query_args: &[String],
        allow_partial_graph: bool,
    ) -> buck2_error::Result<QueryEvaluationResult<TargetNode>>;

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
    )>;

    async fn eval_aquery(
        &self,
        ctx: &mut DiceComputations<'_>,
        working_dir: &ProjectRelativePath,
        query: &str,
        query_args: &[String],
        global_cfg_options: GlobalCfgOptions,
    ) -> buck2_error::Result<QueryEvaluationResult<ActionQueryNode>>;
}

pub static QUERY_FRONTEND: LateBinding<&'static dyn QueryFrontend> =
    LateBinding::new("QUERY_FRONTEND");
