/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::VecDeque;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use starlark_map::Hashed;
use starlark_map::unordered_map;
use starlark_map::unordered_map::UnorderedMap;

use crate::query::graph::dfs::dfs_postorder_impl;
use crate::query::graph::dfs::dfs_preorder;
use crate::query::graph::node::LabeledNode;
use crate::query::graph::successors::AsyncChildVisitor;
use crate::query::graph::successors::GraphSuccessors;
use crate::query::graph::vec_as_map::VecAsMap;
use crate::query::graph::vec_as_set::VecAsSet;
use crate::query::traversal::AsyncNodeLookup;

#[derive(Clone)]
struct GraphNode<N: LabeledNode> {
    node: N,
    children: Vec<u32>,
}

/// Graph with all nodes and edges resolved and represented as integers.
///
/// This is fast to traverse.
#[derive(Clone)]
pub(crate) struct Graph<N: LabeledNode> {
    nodes: Vec<GraphNode<N>>,
    node_to_index: UnorderedMap<N::Key, u32>,
}

impl<N: LabeledNode> Graph<N> {
    pub(crate) fn get(&self, node: &N::Key) -> Option<&N> {
        self.node_to_index
            .get(node)
            .map(|index| &self.nodes[*index as usize].node)
    }
}

struct GraphBuilder<N: LabeledNode> {
    node_to_index: UnorderedMap<N::Key, u32>,
    nodes: VecAsMap<GraphNode<N>>,
}

impl<N: LabeledNode> GraphBuilder<N> {
    fn build(self) -> Graph<N> {
        assert_eq!(self.nodes.vec.len(), self.node_to_index.len());
        let nodes = self
            .nodes
            .vec
            .into_iter()
            .map(|n| n.unwrap())
            .collect::<Vec<_>>();
        Graph {
            nodes,
            node_to_index: self.node_to_index,
        }
    }

    /// Like `build` but but drops missing nodes and edges pointing to them.
    fn build_dropping_missing(self) -> Graph<N> {
        let GraphBuilder {
            mut node_to_index,
            nodes,
        } = self;

        // Old index -> new (compacted) index, for populated nodes only.
        let mut old_to_new: VecAsMap<u32> = VecAsMap::default();
        let mut new_len = 0u32;
        for old_index in 0..(nodes.vec.len() as u32) {
            if nodes.contains_key(old_index) {
                old_to_new.insert(old_index, new_len);
                new_len += 1;
            }
        }

        let new_nodes: Vec<GraphNode<N>> = nodes
            .vec
            .into_iter()
            .flatten()
            .map(|mut node| {
                node.children
                    .retain_mut(|child| match old_to_new.get(*child) {
                        Some(new_child) => {
                            *child = *new_child;
                            true
                        }
                        None => false,
                    });
                node
            })
            .collect();
        assert_eq!(new_nodes.len(), new_len as usize);

        node_to_index.retain(|_, index| match old_to_new.get(*index) {
            Some(new_index) => {
                *index = *new_index;
                true
            }
            None => false,
        });

        Graph {
            nodes: new_nodes,
            node_to_index,
        }
    }

    fn get_or_create_node(&mut self, node: &N::Key) -> u32 {
        let node = Hashed::new(node);
        let new_index = self.node_to_index.len();
        match self.node_to_index.raw_entry_mut().from_key_hashed(node) {
            unordered_map::RawEntryMut::Occupied(e) => *e.get(),
            unordered_map::RawEntryMut::Vacant(e) => {
                let new_index = new_index.try_into().unwrap();
                e.insert((*node.key()).clone(), new_index);
                new_index
            }
        }
    }

    fn insert(&mut self, index: u32, node: N) {
        let prev = self.nodes.insert(
            index,
            GraphNode {
                node,
                children: Vec::new(),
            },
        );
        assert!(prev.is_none());
    }
}

impl<T: LabeledNode> Graph<T> {
    /// Build the graph by traversing the nodes in `root` and their children.
    ///
    /// Resulting graph have node indices assigned non-deterministically.
    ///
    /// With `allow_partial_graph`, a node that fails to load (or whose
    /// children cannot be enumerated) is skipped instead of aborting and the
    /// returned graph omits those nodes and any edges pointing at them.
    pub(crate) async fn build(
        nodes: &impl AsyncNodeLookup<T>,
        root: impl IntoIterator<Item = T::Key>,
        successors: impl AsyncChildVisitor<T>,
        allow_partial_graph: bool,
    ) -> buck2_error::Result<Graph<T>> {
        let mut graph = GraphBuilder::<T> {
            nodes: VecAsMap::default(),
            node_to_index: UnorderedMap::default(),
        };
        // Set when `allow_partial_graph` skips a node, so we know to drop the
        // resulting holes (and edges into them) when finalizing the graph.
        let mut had_failures = false;

        // Map from node to parent node.
        let mut visited: VecAsMap<Option<u32>> = VecAsMap::default();
        let mut push = |queue: &mut FuturesUnordered<_>,
                        target_ref: &T::Key,
                        target_index: u32,
                        parent: Option<u32>,
                        requester: Option<&T::Key>| {
            if visited.contains_key(target_index) {
                return;
            }

            visited.insert(target_index, parent);

            let target_ref = target_ref.clone();
            let requester = requester.cloned();

            queue.push(async move {
                let result = nodes.get_from(&target_ref, requester.as_ref()).await;
                (target_index, result)
            })
        };

        let mut queue = FuturesUnordered::new();

        for target in root {
            let index = graph.get_or_create_node(&target);
            push(&mut queue, &target, index, None, None);
        }

        // TODO(cjhopman): FuturesOrdered/Unordered interacts poorly with tokio cooperative scheduling
        // (see https://github.com/rust-lang/futures-rs/issues/2053). Clean this up once a good
        // solution there exists.
        while let Some((target_index, node)) = tokio::task::unconstrained(queue.next()).await {
            let result: buck2_error::Result<_> = try {
                let node = node?;

                graph.insert(target_index, node.clone());

                successors
                    .for_each_child(&node, &mut |child: &T::Key| {
                        let child_index = graph.get_or_create_node(child);
                        graph
                            .nodes
                            .get_mut(target_index)
                            .unwrap()
                            .children
                            .push(child_index);
                        push(
                            &mut queue,
                            child,
                            child_index,
                            Some(target_index),
                            Some(node.node_key()),
                        );
                        Ok(())
                    })
                    .await?;
                graph
                    .nodes
                    .get_mut(target_index)
                    .unwrap()
                    .children
                    .shrink_to_fit();
            };
            if let Err(mut e) = result {
                if allow_partial_graph {
                    had_failures = true;
                    tracing::trace!("query allow-partial-graph: skipping node due to error: {e:#}");
                    continue;
                }
                let mut target = target_index;
                while let Some(Some(parent_index)) = visited.get(target) {
                    match graph.nodes.get(*parent_index) {
                        None => {
                            return Err(e.context(format!(
                                "Node {parent_index} has not node assigned (internal error)"
                            )));
                        }
                        Some(parent) => {
                            e = e.context(format!(
                                "Error traversing children of {}",
                                parent.node.node_key()
                            ));
                            target = *parent_index;
                        }
                    }
                }
                return Err(e);
            }
        }

        if had_failures {
            Ok(graph.build_dropping_missing())
        } else {
            Ok(graph.build())
        }
    }

    /// Build graph with nodes laid out in stable DFS order.
    pub(crate) async fn build_stable_dfs(
        nodes: &impl AsyncNodeLookup<T>,
        root: impl IntoIterator<Item = T::Key>,
        successors: impl AsyncChildVisitor<T>,
        allow_partial_graph: bool,
    ) -> buck2_error::Result<Graph<T>> {
        let root = root.into_iter().collect::<Vec<_>>();
        let graph =
            Self::build(nodes, root.iter().cloned(), successors, allow_partial_graph).await?;
        // A root may have been dropped from the graph if it failed to load
        // under `allow_partial_graph`; skip it. Every node still in the graph
        // remains reachable from a surviving root, so the post-DFS length
        // assert still holds.
        let root = root
            .into_iter()
            .filter_map(|n| graph.node_to_index.get(&n).copied());
        let mut old_to_new: VecAsMap<u32> = VecAsMap::default();

        let mut new_index = 0;
        graph.dfs_preorder_indices(root, |old_index| {
            let prev = old_to_new.insert(old_index, new_index);
            assert!(prev.is_none());
            new_index += 1;
        });

        assert_eq!(graph.nodes.len(), new_index as usize);

        Ok(graph.index_remap(|old_index| *old_to_new.get(old_index).unwrap()))
    }

    fn dfs_preorder_indices(&self, roots: impl IntoIterator<Item = u32>, visitor: impl FnMut(u32)) {
        dfs_preorder(roots, GraphSuccessorsImpl { graph: self }, visitor)
    }

    /// Remap the indices of the graph.
    fn index_remap(self, remap: impl Fn(u32) -> u32) -> Self {
        let node_count = self.nodes.len().try_into().unwrap();
        self.index_remap_opt(|i| Some(remap(i)), node_count)
    }

    /// Remap the indices of the graph.
    ///
    /// `remap` function must map populate the range `0..count`, otherwise this function will panic.
    fn index_remap_opt(self, remap: impl Fn(u32) -> Option<u32>, count: u32) -> Self {
        let Graph {
            nodes,
            mut node_to_index,
        } = self;

        let mut new_nodes: VecAsMap<GraphNode<T>> = VecAsMap::default();

        for (i, mut node) in nodes.into_iter().enumerate() {
            let old_id: u32 = i.try_into().unwrap();
            let Some(new_id) = remap(old_id) else {
                continue;
            };
            assert!(new_id < count);

            node.children.retain_mut(|node| {
                if let Some(new_node) = remap(*node) {
                    *node = new_node;
                    true
                } else {
                    false
                }
            });
            let prev = new_nodes.insert(new_id, node);
            assert!(prev.is_none());
        }

        node_to_index.retain(|_, index| {
            if let Some(new_index) = remap(*index) {
                *index = new_index;
                true
            } else {
                false
            }
        });

        let new_nodes = new_nodes.vec.into_iter().map(|n| n.unwrap()).collect();
        Graph {
            nodes: new_nodes,
            node_to_index,
        }
    }

    /// Reverse the edges.
    pub(crate) fn reverse(self) -> Graph<T> {
        let Graph {
            mut nodes,
            node_to_index,
        } = self;
        let mut new_edges: Vec<Vec<u32>> = (0..nodes.len()).map(|_| Vec::new()).collect();
        for node in nodes.iter().enumerate() {
            for child in &node.1.children {
                new_edges[*child as usize].push(node.0 as u32);
            }
        }
        for (node, new_edges) in nodes.iter_mut().zip(new_edges) {
            node.children = new_edges;
        }
        Graph {
            nodes,
            node_to_index,
        }
    }

    pub(crate) fn depth_first_postorder_traversal<RootIter: IntoIterator<Item = T::Key>>(
        &self,
        root: RootIter,
        mut visitor: impl FnMut(&T) -> buck2_error::Result<()>,
    ) -> buck2_error::Result<()> {
        dfs_postorder_impl::<_, VecAsSet>(
            // A root absent from the graph (e.g. dropped under
            // `allow_partial_graph`) is skipped rather than panicking on a
            // missing index.
            root.into_iter()
                .filter_map(|root| self.node_to_index.get(&root).copied()),
            GraphSuccessorsImpl { graph: self },
            |index| visitor(&self.nodes[index as usize].node),
        )
    }

    /// Create a graph from the given roots up to the given max depth.
    ///
    /// Zero depth means only the roots.
    pub(crate) fn take_max_depth(
        self,
        roots: impl IntoIterator<Item = T::Key>,
        max_depth: u32,
    ) -> Graph<T> {
        // Map from old index to new index.
        let mut visited: VecAsMap<u32> = VecAsMap::default();
        let mut ids_to_keep = Vec::new();
        let mut edge: VecDeque<u32> = VecDeque::new();

        for root in roots {
            let root = self.node_to_index[&root];
            if !visited.contains_key(root) {
                let new_index = ids_to_keep.len().try_into().unwrap();
                let prev = visited.insert(root, new_index);
                assert!(prev.is_none());
                ids_to_keep.push(root);
                edge.push_back(root);
            }
        }

        for _ in 0..max_depth {
            for _ in 0..edge.len() {
                let node = edge.pop_front().unwrap();
                for &succ in &self.nodes[node as usize].children {
                    if !visited.contains_key(succ) {
                        let new_index = ids_to_keep.len().try_into().unwrap();
                        let prev = visited.insert(succ, new_index);
                        assert!(prev.is_none());
                        ids_to_keep.push(succ);
                        edge.push_back(succ);
                    }
                }
            }
        }

        if self.nodes.len() == ids_to_keep.len() {
            // We visited everything. Skip expensive remap.
            return self;
        }

        self.index_remap_opt(
            |i| visited.get(i).copied(),
            ids_to_keep.len().try_into().unwrap(),
        )
    }
}

struct GraphSuccessorsImpl<'a, N: LabeledNode> {
    graph: &'a Graph<N>,
}

impl<'a, N: LabeledNode> GraphSuccessors<u32> for GraphSuccessorsImpl<'a, N> {
    fn for_each_successor(&self, node: &u32, mut cb: impl FnMut(&u32)) {
        for child in &self.graph.nodes[*node as usize].children {
            cb(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use buck2_query::query::traversal::ChildVisitor;
    use dupe::Dupe;

    use crate::query::graph::bfs::bfs_preorder;
    use crate::query::graph::graph::Graph;
    use crate::query::graph::graph::GraphSuccessorsImpl;
    use crate::query::graph::node::LabeledNode;
    use crate::query::graph::node::NodeKey;
    use crate::query::graph::successors::AsyncChildVisitor;
    use crate::query::traversal::AsyncNodeLookup;

    #[derive(Clone, Copy, Dupe, Eq, PartialEq, Hash, derive_more::Display, Debug)]
    #[display("{}", _0)]
    struct Ref(u32);

    #[derive(Clone, Dupe)]
    struct Node(Ref);

    impl NodeKey for Ref {}

    impl LabeledNode for Node {
        type Key = Ref;

        fn node_key(&self) -> &Self::Key {
            &self.0
        }
    }

    async fn build_graph(start: &[u32], edges: &[(u32, u32)]) -> Graph<Node> {
        struct Lookup;

        #[async_trait]
        impl AsyncNodeLookup<Node> for Lookup {
            async fn get(&self, label: &Ref) -> buck2_error::Result<Node> {
                Ok(Node(label.dupe()))
            }
        }

        struct Successors {
            edges: Vec<(u32, u32)>,
        }

        impl AsyncChildVisitor<Node> for Successors {
            async fn for_each_child(
                &self,
                node: &Node,
                mut children: impl ChildVisitor<Node>,
            ) -> buck2_error::Result<()> {
                for (from, to) in &self.edges {
                    if node.0.0 == *from {
                        children.visit(&Ref(*to))?;
                    }
                }
                Ok(())
            }
        }

        Graph::build(
            &Lookup,
            start.iter().copied().map(Ref),
            Successors {
                edges: edges.to_vec(),
            },
            false, // allow_partial_graph
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_build_then_dfs_postorder() {
        let graph = build_graph(&[10], &[(10, 20), (10, 30)]).await;

        let mut visited = Vec::new();
        graph
            .depth_first_postorder_traversal([Ref(10)], |node| {
                visited.push(node.0.0);
                Ok(())
            })
            .unwrap();

        // TODO(nga): should be `[30, 10, 20]`.
        assert_eq!(vec![30, 20, 10], visited);
    }

    fn bfs(graph: &Graph<Node>, start: &[u32]) -> Vec<u32> {
        let mut visited = Vec::new();
        bfs_preorder(
            start.iter().map(|i| graph.node_to_index[&Ref(*i)]),
            GraphSuccessorsImpl { graph },
            |node| {
                visited.push(graph.nodes[node as usize].node.0.0);
            },
        );
        visited
    }

    #[tokio::test]
    async fn test_take_max_depth() {
        let graph = build_graph(&[10, 30], &[(10, 20), (10, 30), (20, 30), (30, 40)]).await;

        let graph0 = graph.clone().take_max_depth([Ref(10)], 0);
        assert_eq!(vec![10], bfs(&graph0, &[10]));

        let graph1 = graph.clone().take_max_depth([Ref(10)], 1);
        assert_eq!(vec![10, 20, 30], bfs(&graph1, &[10]));

        let graph2 = graph.clone().take_max_depth([Ref(10)], 2);
        assert_eq!(vec![10, 20, 30, 40], bfs(&graph2, &[10]));

        let graph3 = graph.clone().take_max_depth([Ref(10)], 3);
        assert_eq!(vec![10, 20, 30, 40], bfs(&graph3, &[10]));

        let graph4 = graph.clone().take_max_depth([Ref(10)], 4);
        assert_eq!(vec![10, 20, 30, 40], bfs(&graph4, &[10]));

        let graph_2_0 = graph.clone().take_max_depth([Ref(10), Ref(30)], 0);
        assert_eq!(vec![10, 30], bfs(&graph_2_0, &[10, 30]));

        let graph_2_1 = graph.clone().take_max_depth([Ref(10), Ref(30)], 1);
        assert_eq!(vec![10, 30, 20, 40], bfs(&graph_2_1, &[10, 30]));

        graph.take_max_depth([], 100);
    }
}
