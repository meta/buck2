/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::fmt::Debug;
use std::iter;

use async_trait::async_trait;
use buck2_core::build_file_path::BuildFilePath;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::package::PackageLabel;
use buck2_error::BuckErrorContext;
use dupe::Dupe;
use dupe::OptionDupedExt;
use futures::stream::FuturesUnordered;
use futures::stream::TryStreamExt;

use crate::query::graph::async_bfs::async_bfs_find_path;
use crate::query::graph::graph::Graph;
use crate::query::graph::node::LabeledNode;
use crate::query::graph::node::NodeKey;
use crate::query::graph::successors::AsyncChildVisitor;
use crate::query::graph::successors::GraphSuccessors;
use crate::query::syntax::simple::eval::error::QueryError;
use crate::query::syntax::simple::eval::file_set::FileSet;
use crate::query::syntax::simple::eval::set::TargetSet;
use crate::query::syntax::simple::eval::values::QueryValueDepth;
use crate::query::traversal::AsyncNodeLookup;
use crate::query::traversal::ChildVisitor;
mod tests;

#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
pub enum QueryEnvironmentError {
    #[error("Missing target `{}`. Targets in the package: <{}>", .0, .1.join(", "))]
    MissingTargetError(String, Vec<String>),
    #[error("Expected package `{0}` to be available in traversal.")]
    TraversalMissingPackage(PackageLabel),
}

impl QueryEnvironmentError {
    pub fn missing_target<T: NodeKey, S: AsRef<str>, Iter: IntoIterator<Item = S>>(
        target: &T,
        package_targets: Iter,
    ) -> Self {
        let existing = package_targets
            .into_iter()
            .map(|e| format!("`{}`", e.as_ref()))
            .collect();
        Self::MissingTargetError(target.to_string(), existing)
    }
}

pub struct QueryTargets {}

impl QueryTargets {
    /// Used to process all the attrs of a node (both the normal rule attrs and the "special" attrs). Applies
    /// a function to the attrs instead of returning an iterator as some of them are owned and some are refs
    /// into the node.
    pub fn for_all_attrs<
        E: From<buck2_error::Error>,
        T: QueryTarget,
        F: FnMut(&str, &T::Attr<'_>) -> Result<(), E>,
    >(
        target: &T,
        mut func: F,
    ) -> Result<(), E> {
        target.special_attrs_for_each(&mut func)?;
        target.attrs_for_each(&mut func)?;
        Ok(())
    }
}

#[derive(Default, Clone, Dupe)]
pub struct AttrFmtOptions {
    pub exclude_quotes: bool,
}

pub trait QueryTarget: LabeledNode + Dupe + Send + Sync + 'static {
    type Attr<'a>: ?Sized + Debug + 'a;

    /// Called by a caller about to read the attributes, inputs or build file of every one of
    /// `targets`. An implementation that loads those on demand loads them here, several at a
    /// time, so that the caller does not load them one target after another.
    ///
    /// This only saves time: what cannot be loaded here is left for the read that needs it
    /// to report. The error is for buck2's own tests, where a soft error fails the command.
    fn prefetch_bodies<'a>(
        _targets: impl IntoIterator<Item = &'a Self>,
    ) -> buck2_error::Result<()> {
        Ok(())
    }

    /// `filter()` function uses this.
    fn label_for_filter(&self) -> String {
        self.node_key().to_string()
    }

    /// Returns the input files for this node.
    fn inputs_for_each<E: From<buck2_error::Error>, F: FnMut(CellPath) -> Result<(), E>>(
        &self,
        func: F,
    ) -> Result<(), E>;

    fn rule_type(&self) -> Cow<'_, str>;

    fn name(&self) -> Cow<'_, str>;

    /// Return the path to the buildfile that defines this target, e.g. `fbcode//foo/bar/TARGETS`
    fn buildfile_path(&self) -> buck2_error::Result<&BuildFilePath>;

    fn deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a;

    fn exec_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a;

    fn target_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a;

    fn configuration_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a;

    fn toolchain_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a;

    fn tests<'a>(
        &'a self,
    ) -> buck2_error::Result<Option<impl Iterator<Item = Self::Key> + Send + 'a>> {
        Ok(None::<iter::Empty<Self::Key>>)
    }

    fn attr_any_matches(
        attr: &Self::Attr<'_>,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool>;

    /// Match a named user-defined or special attribute without requiring an owned attribute.
    fn attr_matches(
        &self,
        key: &str,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        self.map_any_attr(key, |value| match value {
            Some(value) => Self::attr_any_matches(value, filter),
            None => Ok(false),
        })?
    }

    fn special_attrs_for_each<
        E: From<buck2_error::Error>,
        F: FnMut(&str, &Self::Attr<'_>) -> Result<(), E>,
    >(
        &self,
        func: F,
    ) -> Result<(), E>;

    fn attrs_for_each<
        E: From<buck2_error::Error>,
        F: FnMut(&str, &Self::Attr<'_>) -> Result<(), E>,
    >(
        &self,
        func: F,
    ) -> Result<(), E>;

    fn defined_attrs_for_each<
        E: From<buck2_error::Error>,
        F: FnMut(&str, &Self::Attr<'_>) -> Result<(), E>,
    >(
        &self,
        func: F,
    ) -> Result<(), E>;

    /// map the attr named `key` to a value using `func`
    /// attribute here is usered defined attribute, not including special attributes.
    /// Fails only if the attributes could not be read.
    fn map_attr<R, F: FnMut(Option<&Self::Attr<'_>>) -> R>(
        &self,
        key: &str,
        func: F,
    ) -> buck2_error::Result<R>;

    /// map the any attr (user defined attribute or special attribute) named `key` to a value using `func`
    fn map_any_attr<R, F: FnMut(Option<&Self::Attr<'_>>) -> R>(
        &self,
        key: &str,
        func: F,
    ) -> buck2_error::Result<R>;
}

#[async_trait]
pub trait TraversalFilter<T: QueryTarget>: Send + Sync {
    /// Returns a the children that pass this filter.
    async fn get_children(&self, target: &T) -> buck2_error::Result<TargetSet<T>>;
}

/// The environment of a Buck query that can evaluate queries to produce a
/// result.
#[async_trait]
pub trait QueryEnvironment: Send + Sync {
    type Target: QueryTarget;

    /// Whether the query should skip nodes/edges that fail to load and return
    /// a best-effort partial result, instead of aborting on the first error.
    /// Defaults to false; `uquery` overrides it from `--allow-partial-graph`
    fn allow_partial_graph(&self) -> bool {
        false
    }

    async fn get_node(
        &self,
        node_ref: &<Self::Target as LabeledNode>::Key,
    ) -> buck2_error::Result<Self::Target>;

    /// `get_node`, told which node's expansion is asking for it. Environments
    /// that load packages on demand can credit the load to the requester.
    async fn get_node_from(
        &self,
        node_ref: &<Self::Target as LabeledNode>::Key,
        _requester: Option<&<Self::Target as LabeledNode>::Key>,
    ) -> buck2_error::Result<Self::Target> {
        self.get_node(node_ref).await
    }

    async fn get_node_for_default_configured_target(
        &self,
        node_ref: &<Self::Target as LabeledNode>::Key,
    ) -> buck2_error::Result<MaybeCompatible<Self::Target>>;

    /// Evaluates a literal target pattern. See buck2_common::pattern
    async fn eval_literals(&self, literal: &[&str])
    -> buck2_error::Result<TargetSet<Self::Target>>;

    /// Evaluates a file literal
    async fn eval_file_literal(&self, literal: &str) -> buck2_error::Result<FileSet>;

    /// Performs a depth first traversal, with a post-order callback. The
    /// delegate defines the traversal and receives the callback.
    async fn dfs_postorder(
        &self,
        root: &TargetSet<Self::Target>,
        successors: impl AsyncChildVisitor<Self::Target>,
        visit: impl FnMut(Self::Target) -> buck2_error::Result<()> + Send,
    ) -> buck2_error::Result<()>;

    async fn depth_limited_traversal(
        &self,
        root: &TargetSet<Self::Target>,
        successors: impl AsyncChildVisitor<Self::Target>,
        visit: impl FnMut(Self::Target) -> buck2_error::Result<()> + Send,
        depth: u32,
    ) -> buck2_error::Result<()>;

    async fn allpaths(
        &self,
        from: &TargetSet<Self::Target>,
        to: &TargetSet<Self::Target>,
        filter: Option<&dyn TraversalFilter<Self::Target>>,
    ) -> buck2_error::Result<TargetSet<Self::Target>> {
        self.rdeps(from, to, QueryValueDepth::Unbounded, filter)
            .await
    }

    async fn somepath(
        &self,
        from: &TargetSet<Self::Target>,
        to: &TargetSet<Self::Target>,
        filter: Option<&dyn TraversalFilter<Self::Target>>,
    ) -> buck2_error::Result<TargetSet<Self::Target>> {
        somepath(self, from, to, filter).await
    }

    async fn allbuildfiles(
        &self,
        _universe: &TargetSet<Self::Target>,
    ) -> buck2_error::Result<FileSet> {
        Err(QueryError::FunctionUnimplemented(
            "allbuildfiles() is implemented only for uquery and cquery.",
        )
        .into())
    }

    async fn rbuildfiles(
        &self,
        _universe: &FileSet,
        _argset: &FileSet,
    ) -> buck2_error::Result<FileSet> {
        Err(QueryError::FunctionUnimplemented(
            "rbuildfiles() is implemented only for uquery and cquery.",
        )
        .into())
    }

    async fn rdeps(
        &self,
        universe: &TargetSet<Self::Target>,
        from: &TargetSet<Self::Target>,
        depth: QueryValueDepth,
        filter: Option<&dyn TraversalFilter<Self::Target>>,
    ) -> buck2_error::Result<TargetSet<Self::Target>> {
        rdeps(self, universe, from, depth, filter).await
    }

    async fn testsof(
        &self,
        targets: &TargetSet<Self::Target>,
    ) -> buck2_error::Result<TargetSet<Self::Target>> {
        Self::Target::prefetch_bodies(targets)?;
        let target_tests = targets
            .iter()
            .map(|target| {
                let tests = target
                    .tests()?
                    .ok_or(QueryError::FunctionUnimplemented("testsof"))?;

                buck2_error::Ok((target, tests))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut futs = target_tests
            .into_iter()
            .flat_map(|(target, tests)| {
                tests.into_iter().map(move |test| async move {
                    let test = self.get_node(&test).await.with_buck_error_context(|| {
                        format!(
                            "Error getting test of target {}",
                            LabeledNode::node_key(target),
                        )
                    })?;
                    buck2_error::Ok(test)
                })
            })
            .collect::<FuturesUnordered<_>>();

        let mut ret = TargetSet::new();
        while let Some(test) = futs.try_next().await? {
            ret.insert(test);
        }

        Ok(ret)
    }

    async fn testsof_with_default_target_platform(
        &self,
        targets: &TargetSet<Self::Target>,
    ) -> buck2_error::Result<Vec<MaybeCompatible<Self::Target>>> {
        Self::Target::prefetch_bodies(targets)?;
        let target_tests = targets
            .iter()
            .map(|target| {
                let tests = target
                    .tests()?
                    .ok_or(QueryError::FunctionUnimplemented("testsof"))?;

                buck2_error::Ok((target, tests))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut futs = target_tests
            .into_iter()
            .flat_map(|(target, tests)| {
                tests.into_iter().map(move |test| async move {
                    let test = self
                        .get_node_for_default_configured_target(&test)
                        .await
                        .with_buck_error_context(|| {
                            format!(
                                "Error getting test of target {}",
                                LabeledNode::node_key(target),
                            )
                        })?;
                    buck2_error::Ok(test)
                })
            })
            .collect::<FuturesUnordered<_>>();

        let mut ret = Vec::new();
        while let Some(test) = futs.try_next().await? {
            ret.push(test);
        }

        Ok(ret)
    }

    async fn deps(
        &self,
        targets: &TargetSet<Self::Target>,
        depth: QueryValueDepth,
        filter: Option<&dyn TraversalFilter<Self::Target>>,
    ) -> buck2_error::Result<TargetSet<Self::Target>> {
        deps(self, targets, depth, filter).await
    }

    async fn owner(&self, _paths: &FileSet) -> buck2_error::Result<TargetSet<Self::Target>>;

    async fn targets_in_buildfile(
        &self,
        paths: &FileSet,
    ) -> buck2_error::Result<TargetSet<Self::Target>>;
}

pub async fn deps<Env: QueryEnvironment + ?Sized>(
    env: &Env,
    targets: &TargetSet<Env::Target>,
    depth: QueryValueDepth,
    filter: Option<&dyn TraversalFilter<Env::Target>>,
) -> buck2_error::Result<TargetSet<Env::Target>> {
    let mut deps = TargetSet::new();

    let visitor = QueryTargetFilteredDepsSuccesors { filter };
    let visit = |target| {
        deps.insert_unique_unchecked(target);
        Ok(())
    };

    match depth {
        QueryValueDepth::Bounded(v) => {
            env.depth_limited_traversal(targets, visitor, visit, v)
                .await?;
        }
        QueryValueDepth::Unbounded => {
            env.dfs_postorder(targets, visitor, visit).await?;
        }
    }

    Ok(deps)
}

pub async fn somepath<Env: QueryEnvironment + ?Sized>(
    env: &Env,
    from: &TargetSet<Env::Target>,
    to: &TargetSet<Env::Target>,
    filter: Option<&dyn TraversalFilter<Env::Target>>,
) -> buck2_error::Result<TargetSet<Env::Target>> {
    let path = async_bfs_find_path(
        from.iter(),
        QueryEnvironmentAsNodeLookup { env },
        QueryTargetFilteredDepsSuccesors { filter },
        |t| to.get(t).duped(),
        env.allow_partial_graph(),
    )
    .await?
    .unwrap_or_default();

    let target_set = TargetSet::from_iter(path);
    Ok(target_set)
}

pub async fn rdeps<Env: QueryEnvironment + ?Sized>(
    env: &Env,
    universe: &TargetSet<Env::Target>,
    from: &TargetSet<Env::Target>,
    depth: QueryValueDepth,
    filter: Option<&dyn TraversalFilter<Env::Target>>,
) -> buck2_error::Result<TargetSet<Env::Target>> {
    let graph = Graph::build_stable_dfs(
        &QueryEnvironmentAsNodeLookup { env },
        universe.iter().map(|n| n.node_key().clone()),
        QueryTargetFilteredDepsSuccesors { filter },
        env.allow_partial_graph(),
    )
    .await?;

    let graph = graph.reverse();

    let mut rdeps = TargetSet::new();

    let mut visit = |target| {
        rdeps.insert_unique_unchecked(target);
        Ok(())
    };

    let roots_in_universe = from.filter(|t| Ok(graph.get(t.node_key()).is_some()))?;

    match depth {
        QueryValueDepth::Bounded(depth) => {
            let graph = graph.take_max_depth(
                roots_in_universe.iter().map(|t| t.node_key().clone()),
                depth,
            );
            graph.depth_first_postorder_traversal(
                roots_in_universe.iter().map(|t| t.node_key().clone()),
                |t| visit(t.clone()),
            )?;
        }
        QueryValueDepth::Unbounded => {
            graph.depth_first_postorder_traversal(
                roots_in_universe.iter().map(|t| t.node_key().clone()),
                |t| visit(t.clone()),
            )?;
        }
    }

    Ok(rdeps)
}

pub struct QueryTargetDepsSuccessors;

impl<T: QueryTarget> AsyncChildVisitor<T> for QueryTargetDepsSuccessors {
    async fn for_each_child(
        &self,
        node: &T,
        mut children: impl ChildVisitor<T>,
    ) -> buck2_error::Result<()> {
        for dep in node.deps() {
            children.visit(dep)?;
        }
        Ok(())
    }
}

impl<T> GraphSuccessors<T> for QueryTargetDepsSuccessors
where
    T: QueryTarget<Key = T>,
{
    fn for_each_successor(&self, node: &T, mut cb: impl FnMut(&T)) {
        for dep in node.deps() {
            cb(dep);
        }
    }
}

pub struct QueryEnvironmentAsNodeLookup<'q, Q: QueryEnvironment + ?Sized> {
    pub env: &'q Q,
}

#[async_trait]
impl<'q, Q: QueryEnvironment + ?Sized> AsyncNodeLookup<Q::Target>
    for QueryEnvironmentAsNodeLookup<'q, Q>
{
    async fn get(&self, label: &<Q::Target as LabeledNode>::Key) -> buck2_error::Result<Q::Target> {
        self.env.get_node(label).await
    }

    async fn get_from(
        &self,
        label: &<Q::Target as LabeledNode>::Key,
        requester: Option<&<Q::Target as LabeledNode>::Key>,
    ) -> buck2_error::Result<Q::Target> {
        self.env.get_node_from(label, requester).await
    }
}

pub struct QueryTargetFilteredDepsSuccesors<'a, Q: QueryTarget> {
    filter: Option<&'a dyn TraversalFilter<Q>>,
}

impl<'a, Q: QueryTarget> AsyncChildVisitor<Q> for QueryTargetFilteredDepsSuccesors<'a, Q> {
    async fn for_each_child(
        &self,
        target: &Q,
        mut func: impl ChildVisitor<Q>,
    ) -> buck2_error::Result<()> {
        let res: buck2_error::Result<_> = try {
            match self.filter {
                Some(filter) => {
                    for dep in filter.get_children(target).await?.iter() {
                        func.visit(dep.node_key())?;
                    }
                }
                None => {
                    for dep in target.deps() {
                        func.visit(dep)?;
                    }
                }
            }
        };
        res.with_buck_error_context(|| {
            format!("Error traversing children of `{}`", target.node_key())
        })
    }
}
