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
use buck2_core::cells::cell_path::CellPath;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::BuckErrorOptionContext;
use buck2_node::configured_universe::CqueryUniverse;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured_node_ref::ConfiguredTargetNodeRefNode;
use buck2_node::nodes::configured_node_ref::ConfiguredTargetNodeRefNodeDeps;
use buck2_query::query::environment::QueryEnvironment;
use buck2_query::query::environment::QueryEnvironmentAsNodeLookup;
use buck2_query::query::environment::TraversalFilter;
use buck2_query::query::environment::deps;
use buck2_query::query::graph::dfs::dfs_postorder;
use buck2_query::query::graph::successors::AsyncChildVisitor;
use buck2_query::query::syntax::simple::eval::error::QueryError;
use buck2_query::query::syntax::simple::eval::file_set::FileSet;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_query::query::syntax::simple::eval::values::QueryValueDepth;
use buck2_query::query::syntax::simple::functions::DefaultQueryFunctionsModule;
use buck2_query::query::syntax::simple::functions::HasModuleDescription;
use buck2_query::query::syntax::simple::functions::docs::QueryEnvironmentDescription;
use buck2_query::query::traversal::async_depth_first_postorder_traversal;
use buck2_query::query::traversal::async_depth_limited_traversal;
use dice::DiceComputations;

use crate::uquery::environment::QueryLiterals;
use crate::uquery::environment::UqueryDelegate;
use crate::uquery::environment::allbuildfiles;
use crate::uquery::environment::rbuildfiles;
use crate::uquery::environment::validate_owner_paths;

/// CqueryDelegate resolves information needed by the QueryEnvironment.
#[async_trait]
pub(crate) trait CqueryDelegate: Send + Sync {
    fn uquery_delegate(&self) -> &dyn UqueryDelegate;

    async fn get_node_for_configured_target(
        &self,
        target: &ConfiguredTargetLabel,
    ) -> buck2_error::Result<ConfiguredTargetNode>;

    async fn get_node_for_default_configured_target(
        &self,
        target: &TargetLabel,
    ) -> buck2_error::Result<MaybeCompatible<ConfiguredTargetNode>>;

    fn ctx(&self) -> DiceComputations<'_>;
}

pub(crate) struct CqueryEnvironment<'c> {
    delegate: &'c dyn CqueryDelegate,
    literals: Arc<dyn QueryLiterals<ConfiguredTargetNode> + 'c>,
    // TODO(nga): BXL `cquery` function does not provides us the universe.
    // TODO(nga): do not compute the universe when we don't need it, because it is not free.
    //   For example, when evaluating
    //   ```
    //   buck2 cquery 'deps(//foo:bar)'
    //   ```
    universe: Option<Arc<CqueryUniverse>>,
}

impl<'c> CqueryEnvironment<'c> {
    pub(crate) fn new(
        delegate: &'c dyn CqueryDelegate,
        literals: Arc<dyn QueryLiterals<ConfiguredTargetNode> + 'c>,
        universe: Option<Arc<CqueryUniverse>>,
    ) -> Self {
        Self {
            delegate,
            literals,
            universe,
        }
    }

    pub(crate) fn describe() -> QueryEnvironmentDescription {
        QueryEnvironmentDescription {
            name: "Cquery Environment".to_owned(),
            mods: vec![DefaultQueryFunctionsModule::<Self>::describe()],
        }
    }

    async fn get_node(
        &self,
        label: &ConfiguredTargetLabel,
    ) -> buck2_error::Result<ConfiguredTargetNode> {
        self.delegate.get_node_for_configured_target(label).await
    }

    async fn get_node_for_default_configured_target(
        &self,
        label: &ConfiguredTargetLabel,
    ) -> buck2_error::Result<MaybeCompatible<ConfiguredTargetNode>> {
        self.delegate
            .get_node_for_default_configured_target(label.unconfigured())
            .await
    }

    fn owner_correct(&self, path: &CellPath) -> buck2_error::Result<Vec<ConfiguredTargetNode>> {
        let universe = self
            .universe
            .as_ref()
            .internal_error("Target universe not specified")?;
        universe.owners(path)
    }
}

#[async_trait]
impl QueryEnvironment for CqueryEnvironment<'_> {
    type Target = ConfiguredTargetNode;

    async fn get_node(
        &self,
        node_ref: &ConfiguredTargetLabel,
    ) -> buck2_error::Result<Self::Target> {
        CqueryEnvironment::get_node(self, node_ref).await
    }

    async fn get_node_for_default_configured_target(
        &self,
        node_ref: &ConfiguredTargetLabel,
    ) -> buck2_error::Result<MaybeCompatible<Self::Target>> {
        CqueryEnvironment::get_node_for_default_configured_target(self, node_ref).await
    }

    async fn eval_literals(
        &self,
        literals: &[&str],
    ) -> buck2_error::Result<TargetSet<Self::Target>> {
        self.literals
            .eval_literals(literals, &mut self.delegate.ctx())
            .await
    }

    async fn eval_file_literal(&self, literal: &str) -> buck2_error::Result<FileSet> {
        self.delegate
            .uquery_delegate()
            .eval_file_literal(literal)
            .await
    }

    async fn dfs_postorder(
        &self,
        root: &TargetSet<Self::Target>,
        traversal_delegate: impl AsyncChildVisitor<Self::Target>,
        visit: impl FnMut(Self::Target) -> buck2_error::Result<()> + Send,
    ) -> buck2_error::Result<()> {
        async_depth_first_postorder_traversal(
            &QueryEnvironmentAsNodeLookup { env: self },
            root.iter_names(),
            traversal_delegate,
            visit,
            self.allow_partial_graph(),
        )
        .await
    }

    async fn depth_limited_traversal(
        &self,
        root: &TargetSet<Self::Target>,
        delegate: impl AsyncChildVisitor<Self::Target>,
        visit: impl FnMut(Self::Target) -> buck2_error::Result<()> + Send,
        depth: u32,
    ) -> buck2_error::Result<()> {
        async_depth_limited_traversal(
            &QueryEnvironmentAsNodeLookup { env: self },
            root.iter_names(),
            delegate,
            visit,
            depth,
            self.allow_partial_graph(),
        )
        .await
    }

    async fn allbuildfiles(
        &self,
        universe: &TargetSet<Self::Target>,
    ) -> buck2_error::Result<FileSet> {
        return allbuildfiles(universe, self.delegate.uquery_delegate()).await;
    }

    async fn rbuildfiles(
        &self,
        universe: &FileSet,
        argset: &FileSet,
    ) -> buck2_error::Result<FileSet> {
        return rbuildfiles(universe, argset, self.delegate.uquery_delegate()).await;
    }

    async fn owner(&self, paths: &FileSet) -> buck2_error::Result<TargetSet<Self::Target>> {
        validate_owner_paths(&mut self.delegate.ctx(), paths).await?;
        let mut result = TargetSet::new();

        for path in paths.iter() {
            let owners = self.owner_correct(path)?;
            result.extend(owners);
        }
        Ok(result)
    }

    async fn targets_in_buildfile(
        &self,
        _paths: &FileSet,
    ) -> buck2_error::Result<TargetSet<Self::Target>> {
        Err(QueryError::FunctionUnimplemented("targets_in_buildfile").into())
    }

    async fn deps(
        &self,
        targets: &TargetSet<Self::Target>,
        depth: QueryValueDepth,
        filter: Option<&dyn TraversalFilter<Self::Target>>,
    ) -> buck2_error::Result<TargetSet<Self::Target>> {
        if depth.is_unbounded() && filter.is_none() {
            // TODO(nga): fast lookup with depth too.

            let mut deps = TargetSet::new();
            dfs_postorder::<ConfiguredTargetNodeRefNode>(
                targets.iter().map(ConfiguredTargetNodeRefNode::new),
                ConfiguredTargetNodeRefNodeDeps,
                |target| {
                    deps.insert_unique_unchecked(target.to_node());
                    Ok(())
                },
            )?;
            Ok(deps)
        } else {
            deps(self, targets, depth, filter).await
        }
    }
}
