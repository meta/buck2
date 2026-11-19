/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use buck2_artifact::actions::key::ActionKey;
use buck2_core::cells::CellResolver;
use buck2_core::cells::name::CellName;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_query::query::syntax::simple::eval::file_set::FileSet;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_query::query::syntax::simple::eval::values::QueryValueDepth;
use buck2_query::query::syntax::simple::functions::helpers::CapturedExpr;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;

use crate::actions::query::ActionQueryNode;

#[async_trait]
pub trait BxlCqueryFunctions: Send {
    async fn allpaths(
        &self,
        dice: &mut DiceComputations<'_>,
        from: &TargetSet<ConfiguredTargetNode>,
        to: &TargetSet<ConfiguredTargetNode>,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>>;
    async fn somepath(
        &self,
        dice: &mut DiceComputations<'_>,
        from: &TargetSet<ConfiguredTargetNode>,
        to: &TargetSet<ConfiguredTargetNode>,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>>;
    async fn owner(
        &self,
        dice: &mut DiceComputations<'_>,
        file_set: &FileSet,
        target_universe: Option<&TargetSet<ConfiguredTargetNode>>,
    ) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>>;
    async fn deps(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<ConfiguredTargetNode>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>>;
    async fn rdeps(
        &self,
        dice: &mut DiceComputations<'_>,
        universe: &TargetSet<ConfiguredTargetNode>,
        targets: &TargetSet<ConfiguredTargetNode>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>>;
    async fn testsof(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<ConfiguredTargetNode>,
    ) -> buck2_error::Result<TargetSet<ConfiguredTargetNode>>;
    async fn testsof_with_default_target_platform(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<ConfiguredTargetNode>,
    ) -> buck2_error::Result<Vec<MaybeCompatible<ConfiguredTargetNode>>>;
    async fn allbuildfiles(
        &self,
        dice: &mut DiceComputations<'_>,
        universe: &TargetSet<ConfiguredTargetNode>,
    ) -> buck2_error::Result<FileSet>;
    async fn rbuildfiles(
        &self,
        dice: &mut DiceComputations<'_>,
        universe: &FileSet,
        argset: &FileSet,
    ) -> buck2_error::Result<FileSet>;
}

#[async_trait]
pub trait BxlUqueryFunctions: Send {
    async fn allpaths(
        &self,
        dice: &mut DiceComputations<'_>,
        from: &TargetSet<TargetNode>,
        to: &TargetSet<TargetNode>,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<TargetNode>>;
    async fn somepath(
        &self,
        dice: &mut DiceComputations<'_>,
        from: &TargetSet<TargetNode>,
        to: &TargetSet<TargetNode>,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<TargetNode>>;
    async fn deps(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<TargetNode>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<TargetNode>>;
    async fn rdeps(
        &self,
        dice: &mut DiceComputations<'_>,
        universe: &TargetSet<TargetNode>,
        targets: &TargetSet<TargetNode>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<TargetNode>>;
    async fn testsof(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<TargetNode>,
    ) -> buck2_error::Result<TargetSet<TargetNode>>;
    async fn owner(
        &self,
        dice: &mut DiceComputations<'_>,
        file_set: &FileSet,
    ) -> buck2_error::Result<TargetSet<TargetNode>>;
    async fn targets_in_buildfile(
        &self,
        dice: &mut DiceComputations<'_>,
        file_set: &FileSet,
    ) -> buck2_error::Result<TargetSet<TargetNode>>;
    async fn allbuildfiles(
        &self,
        dice: &mut DiceComputations<'_>,
        universe: &TargetSet<TargetNode>,
    ) -> buck2_error::Result<FileSet>;
    async fn rbuildfiles(
        &self,
        dice: &mut DiceComputations<'_>,
        universe: &FileSet,
        argset: &FileSet,
    ) -> buck2_error::Result<FileSet>;
}

#[async_trait]
pub trait BxlAqueryFunctions: Send {
    async fn allpaths(
        &self,
        dice: &mut DiceComputations<'_>,
        from: &TargetSet<ActionQueryNode>,
        to: &TargetSet<ActionQueryNode>,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>>;
    async fn somepath(
        &self,
        dice: &mut DiceComputations<'_>,
        from: &TargetSet<ActionQueryNode>,
        to: &TargetSet<ActionQueryNode>,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>>;
    async fn deps(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<ActionQueryNode>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>>;
    async fn rdeps(
        &self,
        dice: &mut DiceComputations<'_>,
        universe: &TargetSet<ActionQueryNode>,
        targets: &TargetSet<ActionQueryNode>,
        depth: QueryValueDepth,
        captured_expr: Option<&CapturedExpr>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>>;
    async fn testsof(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<ActionQueryNode>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>>;
    async fn owner(
        &self,
        dice: &mut DiceComputations<'_>,
        file_set: &FileSet,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>>;
    async fn get_target_set(
        &self,
        dice: &mut DiceComputations<'_>,
        configured_labels: Vec<ConfiguredProvidersLabel>,
    ) -> buck2_error::Result<(Vec<ConfiguredTargetLabel>, TargetSet<ActionQueryNode>)>;
    async fn all_outputs(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<ActionQueryNode>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>>;
    async fn all_actions(
        &self,
        dice: &mut DiceComputations<'_>,
        targets: &TargetSet<ActionQueryNode>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>>;
    async fn get_action_nodes(
        &self,
        dice: &mut DiceComputations<'_>,
        action_keys: Vec<ActionKey>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>>;
}

pub static NEW_BXL_CQUERY_FUNCTIONS: LateBinding<
    fn(
        // Target configuration info (target platform + cli modifiers)
        GlobalCfgOptions,
        ProjectRoot,
        CellName,
        CellResolver,
    ) -> Pin<Box<dyn Future<Output = buck2_error::Result<Box<dyn BxlCqueryFunctions>>>>>,
> = LateBinding::new("NEW_BXL_CQUERY_FUNCTIONS");

pub static NEW_BXL_UQUERY_FUNCTIONS: LateBinding<
    fn(
        // allow_partial_graph
        bool,
        ProjectRoot,
        CellName,
        CellResolver,
    )
        -> Pin<Box<dyn Future<Output = buck2_error::Result<Box<dyn BxlUqueryFunctions>>> + Send>>,
> = LateBinding::new("NEW_BXL_UQUERY_FUNCTIONS");

pub static NEW_BXL_AQUERY_FUNCTIONS: LateBinding<
    fn(
        // Target configuration info (target platform + cli modifiers)
        GlobalCfgOptions,
        ProjectRoot,
        CellName,
        CellResolver,
    ) -> Pin<Box<dyn Future<Output = buck2_error::Result<Box<dyn BxlAqueryFunctions>>>>>,
> = LateBinding::new("NEW_BXL_AQUERY_FUNCTIONS");
