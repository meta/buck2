/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future;

use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use futures::StreamExt;
use futures::future::Either;
use futures::stream::FuturesOrdered;
use starlark_map::Hashed;
use starlark_map::unordered_map;
use starlark_map::unordered_map::UnorderedMap;

use crate::query::graph::node::LabeledNode;
use crate::query::graph::successors::AsyncChildVisitor;
use crate::query::traversal::AsyncNodeLookup;

struct Node<N: LabeledNode + 'static> {
    /// `None` for roots.
    parent: Option<N::Key>,
    /// `None` when not yet looked up.
    node: Option<N>,
}

struct BfsVisited<N: LabeledNode + 'static> {
    visited: UnorderedMap<N::Key, Node<N>>,
}

impl<N: LabeledNode + 'static> BfsVisited<N> {
    fn take_path(mut self, last: &N::Key, mut item: impl FnMut(N)) -> buck2_error::Result<()> {
        let node = self
            .visited
            .remove(last)
            .with_internal_error(|| format!("missing node {last}"))?;
        if node.node.is_some() {
            return Err(internal_error!("duplicate node {}", last));
        }
        let mut parent_key = node.parent;
        while let Some(key) = parent_key {
            let node = self
                .visited
                .remove(&key)
                .with_internal_error(|| format!("missing node {key}"))?;
            item(
                node.node
                    .with_internal_error(|| format!("missing node {key}"))?,
            );
            parent_key = node.parent;
        }
        Ok(())
    }
}

pub async fn async_bfs_find_path<'a, N: LabeledNode + 'static>(
    roots: impl IntoIterator<Item = &N>,
    lookup: impl AsyncNodeLookup<N>,
    successors: impl AsyncChildVisitor<N>,
    target: impl Fn(&N::Key) -> Option<N> + Sync,
    allow_partial_graph: bool,
) -> buck2_error::Result<Option<Vec<N>>> {
    let lookup = &lookup;

    let mut visited = BfsVisited::<N> {
        visited: UnorderedMap::new(),
    };

    let mut queue = FuturesOrdered::new();

    for root in roots {
        let root_key = Hashed::new(root.node_key());
        match visited.visited.raw_entry_mut().from_key_hashed(root_key) {
            unordered_map::RawEntryMut::Occupied(_) => {}
            unordered_map::RawEntryMut::Vacant(e) => {
                if let Some(target) = target(root_key.key()) {
                    return Ok(Some(vec![target]));
                }

                e.insert_hashed(
                    root_key.cloned(),
                    Node {
                        parent: None,
                        node: None,
                    },
                );
                queue.push_back(Either::Left(future::ready((
                    root_key.into_key().clone(),
                    buck2_error::Ok(root.dupe()),
                ))));
            }
        }
    }

    while let Some((key, node)) = queue.next().await {
        match node {
            Ok(node) => {
                let mut found: Option<N> = None;
                let children = successors
                    .for_each_child(&node, &mut |succ: &N::Key| {
                        if found.is_some() {
                            return Ok(());
                        }

                        let succ = Hashed::new(succ);
                        match visited.visited.raw_entry_mut().from_key_hashed(succ) {
                            unordered_map::RawEntryMut::Occupied(_) => {}
                            unordered_map::RawEntryMut::Vacant(e) => {
                                if let Some(target) = target(succ.key()) {
                                    found = Some(target);
                                    return Ok(());
                                }

                                e.insert_hashed(
                                    succ.cloned(),
                                    Node {
                                        parent: Some(node.node_key().clone()),
                                        node: None,
                                    },
                                );
                                let succ = (*succ.key()).clone();
                                let requester = node.node_key().clone();
                                queue.push_back(Either::Right(async move {
                                    let succ_node = lookup.get_from(&succ, Some(&requester)).await;
                                    (succ, succ_node)
                                }));
                            }
                        }

                        Ok(())
                    })
                    .await;
                if let Err(e) = children {
                    if allow_partial_graph {
                        // The node itself loaded: keep the children enumerated so far and
                        // carry on, as `Graph::build` does.
                        tracing::trace!(
                            "query allow-partial-graph: skipping children of `{key}` due to error: {e:#}"
                        );
                    } else {
                        return Err(e);
                    }
                }

                if let Some(found) = found {
                    let key = node.node_key().clone();
                    let mut path: Vec<N> = vec![found, node];
                    visited.take_path(&key, |node| path.push(node))?;
                    path.reverse();
                    return Ok(Some(path));
                }
                let prev = visited
                    .visited
                    .get_mut(&key)
                    .with_internal_error(|| format!("missing node {key}"))?
                    .node
                    .replace(node);
                if prev.is_some() {
                    return Err(internal_error!("duplicate node {}", key));
                }
            }
            Err(mut e) => {
                if allow_partial_graph {
                    // Skip nodes with errors if we allow a partial graph
                    continue;
                }
                e = e.context(format!("traversing {key}"));
                let mut nodes = Vec::new();
                visited.take_path(&key, |node| nodes.push(node))?;
                for node in nodes {
                    e = e.context(format!("traversing {}", node.node_key()));
                }
                return Err(e);
            }
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use buck2_hash::BuckMutMap;
    use buck2_hash::BuckMutSet;
    use buck2_query::query::traversal::ChildVisitor;
    use dupe::Dupe;

    use crate::query::graph::async_bfs::async_bfs_find_path;
    use crate::query::graph::node::LabeledNode;
    use crate::query::graph::node::NodeKey;
    use crate::query::graph::successors::AsyncChildVisitor;
    use crate::query::traversal::AsyncNodeLookup;

    #[derive(Copy, Clone, Dupe, derive_more::Display, Debug, Eq, PartialEq, Hash)]
    #[display("{:?}", self)]
    struct TestNodeKey(u32);
    #[derive(Copy, Clone, Dupe, Debug, Eq, PartialEq)]
    struct TestNode(TestNodeKey);

    impl NodeKey for TestNodeKey {}

    impl LabeledNode for TestNode {
        type Key = TestNodeKey;

        fn node_key(&self) -> &Self::Key {
            &self.0
        }
    }

    #[derive(Default)]
    struct TestGraph {
        successors: BuckMutMap<u32, Vec<u32>>,
        errors: BuckMutSet<u32>,
    }

    impl TestGraph {
        fn add_edge(&mut self, from: u32, to: u32) {
            self.successors.entry(from).or_default().push(to);
        }

        fn add_error(&mut self, node: u32) {
            self.errors.insert(node);
        }
    }

    impl TestGraph {
        async fn bfs_find_path(
            &self,
            roots: impl IntoIterator<Item = u32>,
            target: u32,
        ) -> buck2_error::Result<Option<Vec<u32>>> {
            let roots: Vec<TestNode> = roots
                .into_iter()
                .map(|n| TestNode(TestNodeKey(n)))
                .collect();
            let path = async_bfs_find_path(
                &roots,
                self,
                self,
                |n| {
                    if n.0 == target {
                        Some(TestNode(*n))
                    } else {
                        None
                    }
                },
                false, // allow_partial_graph
            )
            .await?;
            Ok(path.map(|path| path.into_iter().map(|n| n.0.0).collect::<Vec<_>>()))
        }
    }

    impl AsyncChildVisitor<TestNode> for TestGraph {
        async fn for_each_child(
            &self,
            node: &TestNode,
            mut children: impl ChildVisitor<TestNode>,
        ) -> buck2_error::Result<()> {
            for succ in self.successors.get(&node.0.0).unwrap_or(&Vec::new()) {
                children.visit(&TestNodeKey(*succ))?;
            }
            Ok(())
        }
    }

    #[async_trait]
    impl AsyncNodeLookup<TestNode> for TestGraph {
        async fn get(&self, label: &TestNodeKey) -> buck2_error::Result<TestNode> {
            if self.errors.contains(&label.0) {
                return Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "my error"
                ));
            }
            Ok(TestNode(*label))
        }
    }

    #[allow(dead_code)]
    struct SuccessorsPlus1;

    impl AsyncChildVisitor<TestNode> for SuccessorsPlus1 {
        async fn for_each_child(
            &self,
            node: &TestNode,
            mut children: impl ChildVisitor<TestNode>,
        ) -> buck2_error::Result<()> {
            children.visit(&TestNodeKey(node.0.0 + 1))?;
            Ok(())
        }
    }

    #[allow(dead_code)]
    struct TestLookupImpl;

    #[async_trait]
    impl AsyncNodeLookup<TestNode> for TestLookupImpl {
        async fn get(&self, label: &TestNodeKey) -> buck2_error::Result<TestNode> {
            Ok(TestNode(*label))
        }
    }

    #[tokio::test]
    async fn test_async_bfs_find_path() {
        let mut g = TestGraph::default();
        g.add_edge(0, 1);
        g.add_edge(1, 2);
        g.add_edge(2, 3);
        g.add_edge(3, 4);
        g.add_edge(4, 5);

        let path = g.bfs_find_path([0], 0).await.unwrap();
        assert_eq!(Some(vec![0]), path);

        let path = g.bfs_find_path([0], 1).await.unwrap();
        assert_eq!(Some(vec![0, 1]), path);

        let path = g.bfs_find_path([0], 2).await.unwrap();
        assert_eq!(Some(vec![0, 1, 2]), path);

        let path = g.bfs_find_path([0], 3).await.unwrap();
        assert_eq!(Some(vec![0, 1, 2, 3]), path);
    }

    #[tokio::test]
    async fn test_async_bfs_find_path_branch() {
        let mut g = TestGraph::default();
        g.add_edge(0, 1);
        g.add_edge(0, 3);
        g.add_edge(1, 2);
        g.add_edge(2, 3);
        g.add_edge(3, 4);

        let path = g.bfs_find_path([0], 0).await.unwrap();
        assert_eq!(Some(vec![0]), path);

        let path = g.bfs_find_path([0], 1).await.unwrap();
        assert_eq!(Some(vec![0, 1]), path);

        let path = g.bfs_find_path([0], 2).await.unwrap();
        assert_eq!(Some(vec![0, 1, 2]), path);

        let path = g.bfs_find_path([0], 3).await.unwrap();
        assert_eq!(Some(vec![0, 3]), path);

        let path = g.bfs_find_path([0], 4).await.unwrap();
        assert_eq!(Some(vec![0, 3, 4]), path);
    }

    #[tokio::test]
    async fn test_async_bfs_find_path_error() {
        let mut g = TestGraph::default();
        g.add_edge(0, 1);
        g.add_edge(1, 2);
        g.add_edge(2, 3);
        g.add_error(3);

        let error = g.bfs_find_path([0], 9).await.unwrap_err();
        let error_stack = error.get_stack_for_debug();
        let errors = error_stack.split("\n").collect::<Vec<_>>();

        assert_eq!(
            vec![
                "CONTEXT: traversing TestNodeKey(0)",
                "CONTEXT: traversing TestNodeKey(1)",
                "CONTEXT: traversing TestNodeKey(2)",
                "CONTEXT: traversing TestNodeKey(3)",
                "CONTEXT: [Input]",
                "ROOT:",
                "\"my error\"",
                ""
            ],
            errors
        );
    }

    #[tokio::test]
    async fn test_async_bfs_find_path_multiple_starts() {
        let mut g = TestGraph::default();
        g.add_edge(0, 1);
        g.add_edge(1, 2);
        g.add_edge(2, 3);
        g.add_edge(3, 4);
        g.add_edge(10, 11);
        g.add_edge(11, 12);

        let path = g.bfs_find_path([0, 10], 12).await.unwrap();
        assert_eq!(Some(vec![10, 11, 12]), path);
    }

    #[tokio::test]
    async fn test_async_bfs_find_path_no_path() {
        let mut g = TestGraph::default();
        g.add_edge(0, 1);
        g.add_edge(1, 2);
        g.add_edge(1, 3);
        g.add_edge(1, 4);
        g.add_edge(2, 3);
        g.add_edge(3, 4);

        let path = g.bfs_find_path([0], 10).await.unwrap();
        assert_eq!(None, path);
    }
}
