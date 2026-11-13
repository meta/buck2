/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![cfg(test)]

use std::fmt;
use std::sync::Arc;

use buck2_error::BuckErrorOptionContext;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::BuckMutMap;
use buck2_query::query::traversal::NodeLookup;
use buck2_query::query::traversal::async_depth_first_postorder_traversal;
use buck2_query::query::traversal::async_depth_limited_traversal;
use derive_more::Display;
use derive_more::From;

use super::*;

#[derive(Debug, Copy, Clone, Dupe, Eq, PartialEq, Hash, Display, From)]
struct TestTargetId(u64);

impl NodeKey for TestTargetId {}

#[derive(Debug, Copy, Clone, Dupe, Eq, PartialEq, Hash, Display)]
struct TestTargetAttr;

#[derive(Clone, Dupe, Eq, PartialEq)]
struct TestTarget {
    id: TestTargetId,
    deps: Arc<BuckMutInsertionSet<TestTargetId>>,
}

/// Custom debug to make the test output more readable
impl fmt::Debug for TestTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id.0)
    }
}

impl LabeledNode for TestTarget {
    type Key = TestTargetId;

    fn node_key(&self) -> &Self::Key {
        &self.id
    }
}

impl QueryTarget for TestTarget {
    type Attr<'a> = TestTargetAttr;

    fn inputs_for_each<E: From<buck2_error::Error>, F: FnMut(CellPath) -> Result<(), E>>(
        &self,
        _func: F,
    ) -> Result<(), E> {
        unimplemented!()
    }

    fn rule_type(&self) -> Cow<'_, str> {
        unimplemented!()
    }

    fn name(&self) -> Cow<'_, str> {
        unimplemented!()
    }

    fn buildfile_path(&self) -> buck2_error::Result<&BuildFilePath> {
        unimplemented!()
    }

    fn deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
        Box::new(self.deps.iter())
    }

    fn exec_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
        Box::new(std::iter::empty())
    }

    fn target_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
        Box::new(std::iter::empty())
    }

    fn configuration_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
        Box::new(std::iter::empty())
    }

    fn toolchain_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
        Box::new(std::iter::empty())
    }

    fn attr_any_matches(
        _attr: &Self::Attr<'_>,
        _filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        unimplemented!()
    }

    fn special_attrs_for_each<
        E: From<buck2_error::Error>,
        F: FnMut(&str, &Self::Attr<'_>) -> Result<(), E>,
    >(
        &self,
        _func: F,
    ) -> Result<(), E> {
        unimplemented!()
    }

    fn attrs_for_each<
        E: From<buck2_error::Error>,
        F: FnMut(&str, &Self::Attr<'_>) -> Result<(), E>,
    >(
        &self,
        _func: F,
    ) -> Result<(), E> {
        unimplemented!()
    }

    fn defined_attrs_for_each<
        E: From<buck2_error::Error>,
        F: FnMut(&str, &Self::Attr<'_>) -> Result<(), E>,
    >(
        &self,
        _func: F,
    ) -> Result<(), E> {
        unimplemented!()
    }

    fn map_attr<R, F: FnMut(Option<&Self::Attr<'_>>) -> R>(
        &self,
        _key: &str,
        _func: F,
    ) -> buck2_error::Result<R> {
        unimplemented!()
    }

    fn map_any_attr<R, F: FnMut(Option<&Self::Attr<'_>>) -> R>(
        &self,
        _key: &str,
        _func: F,
    ) -> buck2_error::Result<R> {
        unimplemented!()
    }
}

struct TestEnv {
    graph: BuckMutMap<TestTargetId, TestTarget>,
}

impl NodeLookup<TestTarget> for TestEnv {
    fn get(&self, label: &<TestTarget as LabeledNode>::Key) -> buck2_error::Result<TestTarget> {
        self.graph
            .get(label)
            .duped()
            .with_internal_error(|| format!("Invalid node: {label:?}"))
    }
}

#[async_trait]
impl AsyncNodeLookup<TestTarget> for TestEnv {
    async fn get(
        &self,
        label: &<TestTarget as LabeledNode>::Key,
    ) -> buck2_error::Result<TestTarget> {
        self.graph
            .get(label)
            .duped()
            .with_internal_error(|| format!("Invalid node: {label:?}"))
    }
}

#[async_trait]
impl QueryEnvironment for TestEnv {
    type Target = TestTarget;

    async fn get_node(
        &self,
        node_ref: &<Self::Target as LabeledNode>::Key,
    ) -> buck2_error::Result<Self::Target> {
        <Self as NodeLookup<TestTarget>>::get(self, node_ref)
    }

    async fn get_node_for_default_configured_target(
        &self,
        _node_ref: &<Self::Target as LabeledNode>::Key,
    ) -> buck2_error::Result<MaybeCompatible<Self::Target>> {
        unimplemented!()
    }

    async fn eval_literals(
        &self,
        _literal: &[&str],
    ) -> buck2_error::Result<TargetSet<Self::Target>> {
        unimplemented!()
    }

    async fn eval_file_literal(&self, _literal: &str) -> buck2_error::Result<FileSet> {
        unimplemented!()
    }

    async fn dfs_postorder(
        &self,
        root: &TargetSet<Self::Target>,
        delegate: impl AsyncChildVisitor<Self::Target>,
        visit: impl FnMut(Self::Target) -> buck2_error::Result<()> + Send,
    ) -> buck2_error::Result<()> {
        // TODO: Should this be part of QueryEnvironment's default impl?
        async_depth_first_postorder_traversal(
            self,
            root.iter_names(),
            delegate,
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
            self,
            root.iter_names(),
            delegate,
            visit,
            depth,
            self.allow_partial_graph(),
        )
        .await
    }

    async fn owner(&self, _paths: &FileSet) -> buck2_error::Result<TargetSet<Self::Target>> {
        unimplemented!()
    }

    async fn targets_in_buildfile(
        &self,
        _paths: &FileSet,
    ) -> buck2_error::Result<TargetSet<Self::Target>> {
        unimplemented!()
    }
}

impl TestEnv {
    /// A helper to get e.g. stuff like "1,2,3" into a TargetSet.
    fn set(&self, entries: &str) -> buck2_error::Result<TargetSet<TestTarget>> {
        let mut set = TargetSet::new();
        for c in entries.split(',') {
            let id = TestTargetId(c.parse().buck_error_context("Invalid ID")?);
            set.insert(<Self as NodeLookup<TestTarget>>::get(self, &id)?);
        }
        Ok(set)
    }
}

#[derive(Default)]
pub struct TestEnvBuilder {
    graph: BuckMutMap<u64, BuckMutInsertionSet<u64>>,
}

impl TestEnvBuilder {
    fn edge(&mut self, from: u64, to: u64) {
        self.graph.entry(from).or_default().insert(to);
        self.graph.entry(to).or_default();
    }

    fn build(&self) -> TestEnv {
        TestEnv {
            graph: self
                .graph
                .iter()
                .map(|(id, vs)| {
                    let id = TestTargetId(*id);
                    let deps = Arc::new(vs.iter().map(|v| TestTargetId(*v)).collect());
                    (id, TestTarget { id, deps })
                })
                .collect(),
        }
    }
}

#[tokio::test]
async fn test_one_path() -> buck2_error::Result<()> {
    let mut env = TestEnvBuilder::default();
    // The actual path
    env.edge(1, 2);
    env.edge(2, 3);
    // Some unused edges
    env.edge(3, 4);
    env.edge(1, 10);
    env.edge(1, 12);
    let env = env.build();

    let path = env.allpaths(&env.set("1")?, &env.set("3")?, None).await?;
    let expected = env.set("1,2,3")?;
    assert_eq!(path, expected);

    let path = env.somepath(&env.set("1")?, &env.set("3")?, None).await?;
    let expected = env.set("1,2,3")?;
    assert_eq!(path, expected);

    Ok(())
}

#[tokio::test]
async fn test_many_paths() -> buck2_error::Result<()> {
    let mut env = TestEnvBuilder::default();
    env.edge(1, 2);
    env.edge(2, 3);
    env.edge(1, 10);
    env.edge(10, 11);
    env.edge(11, 3);
    // More unused edges
    env.edge(3, 4);
    env.edge(10, 20);
    let env = env.build();

    let path = env.allpaths(&env.set("1")?, &env.set("3")?, None).await?;
    let expected = env.set("1,10,11,2,3")?;
    assert_eq!(path, expected);

    let path = env.somepath(&env.set("1")?, &env.set("3")?, None).await?;
    let expected = env.set("1,2,3")?;
    assert_eq!(path, expected);

    Ok(())
}

#[tokio::test]
async fn test_distinct_paths() -> buck2_error::Result<()> {
    let mut env = TestEnvBuilder::default();
    env.edge(1, 10);
    env.edge(10, 100);
    env.edge(2, 20);
    env.edge(20, 200);
    let env = env.build();

    let path = env
        .allpaths(&env.set("1,2")?, &env.set("100,200")?, None)
        .await?;
    let expected = env.set("2,20,200,1,10,100")?;
    assert_eq!(path, expected);

    // Same as above
    let path = env
        .somepath(&env.set("1,2")?, &env.set("100,200")?, None)
        .await?;
    let expected = env.set("1,10,100")?;
    assert_eq!(path, expected);

    Ok(())
}

#[tokio::test]
async fn test_no_path() -> buck2_error::Result<()> {
    let mut env = TestEnvBuilder::default();
    env.edge(1, 10);
    env.edge(2, 20);
    let env = env.build();

    let path = env.allpaths(&env.set("1")?, &env.set("20")?, None).await?;
    let expected = TargetSet::new();
    assert_eq!(path, expected);

    let path = env.somepath(&env.set("1")?, &env.set("20")?, None).await?;
    let expected = TargetSet::new();
    assert_eq!(path, expected);

    Ok(())
}

#[tokio::test]
async fn test_nested_paths() -> buck2_error::Result<()> {
    let mut env = TestEnvBuilder::default();
    env.edge(1, 2);
    env.edge(2, 3);
    env.edge(3, 4);
    let env = env.build();

    let path = env.allpaths(&env.set("1")?, &env.set("2,4")?, None).await?;
    assert_eq!(path, env.set("1,2,3,4")?);

    let path = env.somepath(&env.set("1")?, &env.set("2,4")?, None).await?;
    assert_eq!(path, env.set("1,2")?);

    Ok(())
}

#[tokio::test]
async fn test_paths_with_cycles_present() -> buck2_error::Result<()> {
    let mut env = TestEnvBuilder::default();
    env.edge(1, 2);
    env.edge(2, 3);
    env.edge(3, 4);
    env.edge(4, 5);
    // Introduce cycles.
    env.edge(4, 1);
    env.edge(4, 3);
    let env = env.build();

    let path = env.allpaths(&env.set("3")?, &env.set("4")?, None).await?;
    assert_eq!(path, env.set("1,2,3,4")?);

    let path = env.allpaths(&env.set("1")?, &env.set("1")?, None).await?;
    assert_eq!(path, env.set("2,3,4,1")?);

    let path = env.allpaths(&env.set("1")?, &env.set("5")?, None).await?;
    assert_eq!(path, env.set("1,2,3,4,5")?);

    let path = env
        .rdeps(&env.set("1")?, &env.set("3")?, Some(2).into(), None)
        .await?;
    assert_eq!(path, env.set("4,1,2,3")?);

    Ok(())
}

#[tokio::test]
async fn test_rdeps() -> buck2_error::Result<()> {
    let mut env = TestEnvBuilder::default();
    env.edge(1, 2);
    env.edge(2, 3);
    env.edge(3, 4); // Dead end.
    env.edge(4, 5);
    env.edge(1, 3); // Shortcut.
    env.edge(3, 6);
    let env = env.build();

    let path = env
        .rdeps(&env.set("1")?, &env.set("6")?, Some(0).into(), None)
        .await?;
    assert_eq!(path, env.set("6")?);

    let path = env
        .rdeps(&env.set("1")?, &env.set("6")?, Some(1).into(), None)
        .await?;
    assert_eq!(path, env.set("3,6")?);

    let path = env
        .rdeps(&env.set("1")?, &env.set("6")?, Some(2).into(), None)
        .await?;
    assert_eq!(path, env.set("1,2,3,6")?);

    let path = env
        .rdeps(&env.set("1")?, &env.set("6")?, Some(3).into(), None)
        .await?;
    assert_eq!(path, env.set("1,2,3,6")?);

    let path = env
        .rdeps(&env.set("1")?, &env.set("6")?, Some(4).into(), None)
        .await?;
    assert_eq!(path, env.set("1,2,3,6")?);

    let path = env
        .rdeps(&env.set("1")?, &env.set("6")?, None::<u32>.into(), None)
        .await?;
    assert_eq!(path, env.set("1,2,3,6")?);

    Ok(())
}

/// `TestEnv` with `allow_partial_graph` on, as `buck2 uquery --allow-partial-graph` sets it.
struct PartialTestEnv(TestEnv);

#[async_trait]
impl QueryEnvironment for PartialTestEnv {
    type Target = TestTarget;

    fn allow_partial_graph(&self) -> bool {
        true
    }

    async fn get_node(
        &self,
        node_ref: &<Self::Target as LabeledNode>::Key,
    ) -> buck2_error::Result<Self::Target> {
        <TestEnv as NodeLookup<TestTarget>>::get(&self.0, node_ref)
    }

    async fn get_node_for_default_configured_target(
        &self,
        _node_ref: &<Self::Target as LabeledNode>::Key,
    ) -> buck2_error::Result<MaybeCompatible<Self::Target>> {
        unimplemented!()
    }

    async fn eval_literals(
        &self,
        _literal: &[&str],
    ) -> buck2_error::Result<TargetSet<Self::Target>> {
        unimplemented!()
    }

    async fn eval_file_literal(&self, _literal: &str) -> buck2_error::Result<FileSet> {
        unimplemented!()
    }

    async fn dfs_postorder(
        &self,
        root: &TargetSet<Self::Target>,
        delegate: impl AsyncChildVisitor<Self::Target>,
        visit: impl FnMut(Self::Target) -> buck2_error::Result<()> + Send,
    ) -> buck2_error::Result<()> {
        async_depth_first_postorder_traversal(&self.0, root.iter_names(), delegate, visit, true)
            .await
    }

    async fn depth_limited_traversal(
        &self,
        root: &TargetSet<Self::Target>,
        delegate: impl AsyncChildVisitor<Self::Target>,
        visit: impl FnMut(Self::Target) -> buck2_error::Result<()> + Send,
        depth: u32,
    ) -> buck2_error::Result<()> {
        async_depth_limited_traversal(&self.0, root.iter_names(), delegate, visit, depth, true)
            .await
    }

    async fn owner(&self, _paths: &FileSet) -> buck2_error::Result<TargetSet<Self::Target>> {
        unimplemented!()
    }

    async fn targets_in_buildfile(
        &self,
        _paths: &FileSet,
    ) -> buck2_error::Result<TargetSet<Self::Target>> {
        unimplemented!()
    }
}

/// A graph where a dep may name a node that is absent, so looking it up fails (like a target in
/// a package that fails to parse).
fn env_with_dangling_deps(nodes: &[(u64, &[u64])]) -> TestEnv {
    TestEnv {
        graph: nodes
            .iter()
            .map(|(id, deps)| {
                let id = TestTargetId(*id);
                let deps = Arc::new(deps.iter().map(|d| TestTargetId(*d)).collect());
                (id, TestTarget { id, deps })
            })
            .collect(),
    }
}

/// Behaves like the `first_order_deps()` filter: it loads every dep, so one dep that does not
/// load makes the whole filter fail for that node.
struct FirstOrderDepsFilter<'a>(&'a PartialTestEnv);

#[async_trait]
impl TraversalFilter<TestTarget> for FirstOrderDepsFilter<'_> {
    async fn get_children(
        &self,
        target: &TestTarget,
    ) -> buck2_error::Result<TargetSet<TestTarget>> {
        let mut deps = TargetSet::new();
        for dep in target.deps() {
            deps.insert(self.0.get_node(dep).await?);
        }
        Ok(deps)
    }
}

fn sorted_ids(set: &TargetSet<TestTarget>) -> Vec<u64> {
    let mut ids: Vec<u64> = set.iter().map(|t| t.id.0).collect();
    ids.sort();
    ids
}

/// Graph 0 -> 1 -> 2 where node 2 does not load, so the filter fails for node 1. Node 1 itself
/// loaded, so every depth keeps it and drops only its children.
#[tokio::test]
async fn test_partial_graph_bounded_deps_keeps_node_whose_filter_fails() -> buck2_error::Result<()>
{
    let env = PartialTestEnv(env_with_dangling_deps(&[(0, &[1]), (1, &[2])]));
    let filter = FirstOrderDepsFilter(&env);
    let filter = Some(&filter as &dyn TraversalFilter<TestTarget>);
    let roots = env.0.set("0")?;

    let unbounded = deps(&env, &roots, QueryValueDepth::Unbounded, filter).await?;
    let depth1 = deps(&env, &roots, QueryValueDepth::Bounded(1), filter).await?;
    let depth2 = deps(&env, &roots, QueryValueDepth::Bounded(2), filter).await?;

    assert_eq!(vec![0, 1], sorted_ids(&unbounded));
    assert_eq!(vec![0, 1], sorted_ids(&depth1));
    assert_eq!(vec![0, 1], sorted_ids(&depth2));
    Ok(())
}

/// Graph 0 -> {1, 2}, 1 -> 9 (absent), 2 -> 3. The filter fails for node 1, which the path
/// 0 -> 2 -> 3 does not need.
#[tokio::test]
async fn test_partial_graph_somepath_skips_node_whose_filter_fails() -> buck2_error::Result<()> {
    let env = PartialTestEnv(env_with_dangling_deps(&[
        (0, &[1, 2]),
        (1, &[9]),
        (2, &[3]),
        (3, &[]),
    ]));
    let filter = FirstOrderDepsFilter(&env);
    let filter = Some(&filter as &dyn TraversalFilter<TestTarget>);
    let from = env.0.set("0")?;
    let to = env.0.set("3")?;

    let all = env.allpaths(&from, &to, filter).await?;
    assert_eq!(vec![0, 2, 3], sorted_ids(&all));

    let path = env.somepath(&from, &to, filter).await?;
    assert_eq!(
        vec![0, 2, 3],
        path.iter().map(|t| t.id.0).collect::<Vec<_>>()
    );
    Ok(())
}

/// Without a filter, a node that itself fails to load is skipped.
#[tokio::test]
async fn test_partial_graph_somepath_skips_node_that_fails_to_load() -> buck2_error::Result<()> {
    let env = PartialTestEnv(env_with_dangling_deps(&[(0, &[1, 2]), (2, &[3]), (3, &[])]));
    let path = env
        .somepath(&env.0.set("0")?, &env.0.set("3")?, None)
        .await?;
    assert_eq!(
        vec![0, 2, 3],
        path.iter().map(|t| t.id.0).collect::<Vec<_>>()
    );
    Ok(())
}

mod multi_query_merge {
    use buck2_core::cells::cell_path::CellPath;
    use buck2_hash::BuckMutInsertionMap;

    use super::*;
    use crate::query::syntax::simple::eval::file_set::FileNode;
    use crate::query::syntax::simple::eval::multi_query::MultiQueryResult;
    use crate::query::syntax::simple::eval::values::QueryEvaluationValue;

    /// The per-argument results of `uquery '%s' 'inputs(//bin:the_binary)' '//bin:the_binary'`:
    /// a set of files for the first argument and a set of targets for the second.
    fn files_then_targets() -> MultiQueryResult<TestTarget> {
        let mut env = TestEnvBuilder::default();
        env.edge(1, 2);
        let env = env.build();
        let files = FileSet::new(BuckMutInsertionSet::from_iter([FileNode(
            CellPath::testing_new("root//bin/TARGETS"),
        )]));
        MultiQueryResult(BuckMutInsertionMap::from_iter([
            (
                "inputs(root//bin:the_binary)".to_owned(),
                Ok(QueryEvaluationValue::FileSet(files)),
            ),
            (
                "root//bin:the_binary".to_owned(),
                Ok(QueryEvaluationValue::TargetSet(env.set("1").unwrap())),
            ),
        ]))
    }

    /// Merging the results of a `%s` query (every output mode except `--json`) reports mixed
    /// result kinds as an input error.
    #[test]
    fn merging_files_with_targets_is_an_error() {
        let err = files_then_targets().merged().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("can only merge results of one kind"), "{msg}");
        assert!(msg.contains("`inputs(root//bin:the_binary)`"), "{msg}");
        assert!(msg.contains("`root//bin:the_binary`"), "{msg}");
    }
}

mod query_function_args {
    use buck2_core::cells::cell_path::CellPath;
    use buck2_query_parser::parse_expr;

    use super::*;
    use crate::query::syntax::simple::eval::evaluator::QueryEvaluator;
    use crate::query::syntax::simple::eval::file_set::FileNode;
    use crate::query::syntax::simple::eval::literals::extract_target_literals;
    use crate::query::syntax::simple::eval::values::QueryEvaluationValue;
    use crate::query::syntax::simple::functions::DefaultQueryFunctions;
    use crate::query::syntax::simple::functions::DefaultQueryFunctionsModule;
    use crate::query::syntax::simple::functions::helpers::CapturedExpr;

    /// `TestEnv` with literals: a number is a target id, anything else is not a target pattern;
    /// any file literal is accepted.
    struct LiteralEnv(TestEnv);

    #[async_trait]
    impl QueryEnvironment for LiteralEnv {
        type Target = TestTarget;

        async fn get_node(&self, node_ref: &TestTargetId) -> buck2_error::Result<TestTarget> {
            self.0.get_node(node_ref).await
        }

        async fn get_node_for_default_configured_target(
            &self,
            _node_ref: &TestTargetId,
        ) -> buck2_error::Result<MaybeCompatible<TestTarget>> {
            unimplemented!()
        }

        async fn eval_literals(
            &self,
            literals: &[&str],
        ) -> buck2_error::Result<TargetSet<TestTarget>> {
            let mut set = TargetSet::new();
            for literal in literals {
                let id: u64 = literal.parse().map_err(|_| {
                    buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "`{literal}` is not a target pattern"
                    )
                })?;
                set.insert(self.0.get_node(&TestTargetId(id)).await?);
            }
            Ok(set)
        }

        async fn eval_file_literal(&self, literal: &str) -> buck2_error::Result<FileSet> {
            Ok(FileSet::new(BuckMutInsertionSet::from_iter([FileNode(
                CellPath::testing_new(&format!("root//{literal}")),
            )])))
        }

        async fn dfs_postorder(
            &self,
            root: &TargetSet<TestTarget>,
            delegate: impl AsyncChildVisitor<TestTarget>,
            visit: impl FnMut(TestTarget) -> buck2_error::Result<()> + Send,
        ) -> buck2_error::Result<()> {
            self.0.dfs_postorder(root, delegate, visit).await
        }

        async fn depth_limited_traversal(
            &self,
            root: &TargetSet<TestTarget>,
            delegate: impl AsyncChildVisitor<TestTarget>,
            visit: impl FnMut(TestTarget) -> buck2_error::Result<()> + Send,
            depth: u32,
        ) -> buck2_error::Result<()> {
            self.0
                .depth_limited_traversal(root, delegate, visit, depth)
                .await
        }

        async fn owner(&self, _paths: &FileSet) -> buck2_error::Result<TargetSet<TestTarget>> {
            unimplemented!()
        }

        async fn targets_in_buildfile(
            &self,
            _paths: &FileSet,
        ) -> buck2_error::Result<TargetSet<TestTarget>> {
            unimplemented!()
        }
    }

    fn env() -> LiteralEnv {
        let mut env = TestEnvBuilder::default();
        env.edge(1, 2);
        env.edge(2, 3);
        LiteralEnv(env.build())
    }

    async fn eval(env: &LiteralEnv, query: &str) -> buck2_error::Result<String> {
        let functions = DefaultQueryFunctionsModule::new();
        match QueryEvaluator::new(env, &functions)
            .eval_query(query)
            .await?
        {
            QueryEvaluationValue::TargetSet(targets) => Ok(targets.to_string()),
            QueryEvaluationValue::FileSet(files) => Ok(files.to_string()),
        }
    }

    /// Both the literal visitor and the evaluator reject a fourth argument to `deps`, also when
    /// the call is inside a filter expression that is evaluated per node.
    #[tokio::test]
    async fn extra_arguments_are_rejected_by_the_evaluator() -> buck2_error::Result<()> {
        let env = env();
        let query = "deps(set(1), 1, first_order_deps(), nosuchfunction())";
        let functions = DefaultQueryFunctionsModule::<LiteralEnv>::new();
        let expected = "too many args. function `deps` accepts maximum 3 args, got 4";
        let err = extract_target_literals(&functions, query).unwrap_err();
        assert!(format!("{err:#}").contains(expected));
        let err = eval(&env, query).await.unwrap_err();
        assert!(format!("{err:#}").contains(expected), "{err:#}");

        let filter = parse_expr("first_order_deps(nosuchfunction(), 7)")?;
        let err = DefaultQueryFunctions::<LiteralEnv>::new()
            .deps(
                &env,
                &functions,
                &env.0.set("1")?,
                QueryValueDepth::Unbounded,
                Some(&CapturedExpr { expr: &filter }),
            )
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains(
                "too many args. function `first_order_deps` accepts maximum 0 args, got 2"
            ),
            "{err:#}"
        );
        Ok(())
    }
}
