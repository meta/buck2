/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use futures::StreamExt;
use futures::stream::FuturesOrdered;

use crate::query::graph::graph::Graph;
use crate::query::graph::node::LabeledNode;
use crate::query::graph::successors::AsyncChildVisitor;

pub trait ChildVisitor<T: LabeledNode>: Send {
    fn visit(&mut self, node: &T::Key) -> buck2_error::Result<()>;
}

impl<F, T: LabeledNode> ChildVisitor<T> for F
where
    F: FnMut(&T::Key) -> buck2_error::Result<()>,
    F: Send,
{
    fn visit(&mut self, node: &T::Key) -> buck2_error::Result<()> {
        self(node)
    }
}

pub trait NodeLookup<T: LabeledNode> {
    // TODO(cjhopman): Maybe this should be `&mut self` since we only need the one reference to it.
    fn get(&self, label: &T::Key) -> buck2_error::Result<T>;
}

#[async_trait]
pub trait AsyncNodeLookup<T: LabeledNode>: Send + Sync {
    async fn get(&self, label: &T::Key) -> buck2_error::Result<T>;

    /// `get`, told which node's expansion is asking. Lookups that load on
    /// demand can credit the load to the requester.
    async fn get_from(
        &self,
        label: &T::Key,
        _requester: Option<&T::Key>,
    ) -> buck2_error::Result<T> {
        self.get(label).await
    }
}

/// Node lookup when node key is the same as the node.
pub struct NodeLookupId;

impl<T: LabeledNode<Key = T>> NodeLookup<T> for NodeLookupId {
    fn get(&self, key: &T::Key) -> buck2_error::Result<T> {
        Ok(key.dupe())
    }
}

#[async_trait]
impl<T: LabeledNode<Key = T>> AsyncNodeLookup<T> for NodeLookupId {
    async fn get(&self, key: &T::Key) -> buck2_error::Result<T> {
        Ok(key.dupe())
    }
}

#[async_trait]
impl<T: LabeledNode, A: AsyncNodeLookup<T>> AsyncNodeLookup<T> for &A {
    async fn get(&self, label: &T::Key) -> buck2_error::Result<T> {
        (*self).get(label).await
    }

    async fn get_from(&self, label: &T::Key, requester: Option<&T::Key>) -> buck2_error::Result<T> {
        (*self).get_from(label, requester).await
    }
}

/// Implements a depth-first postorder traversal. A node will be visited only after all of its
/// dependencies have been visited. Unlike `async_depth_first_postorder_traversal`, this will
/// only perform a single traversal, however that means that it will wait on each node lookup.
/// This should be used when the node lookups are cheap, if node lookups may be expensive, the
/// non-fast version should be used.
// TODO(cjhopman): Figure out how to implement this traversal in a way that has good performance
// in both cases.
pub async fn async_fast_depth_first_postorder_traversal<
    T: LabeledNode,
    RootIter: IntoIterator<Item = T::Key>,
>(
    nodes: &impl NodeLookup<T>,
    root: RootIter,
    successors: impl AsyncChildVisitor<T>,
    mut visit: impl FnMut(T) -> buck2_error::Result<()>,
) -> buck2_error::Result<()> {
    // This implementation simply performs a dfs. We maintain a work stack here.
    // When visiting a node, we first add an item to the work stack to call
    // post_visit for that node, and then add items to visit all the
    // children. While a work item for a child will not be added if it has
    // already been visited, if there's an item in the stack for that child
    // it will still be added. When popping the visit, if the node had been
    // visited, it's ignored. This ensures that a node's children are all
    // visited before we do PostVisit for that node.
    enum WorkItem<T: LabeledNode> {
        PostVisit(T),
        Visit(T::Key),
    }

    // TODO(cjhopman): There's a couple of things that could be improved about this.
    // 1. it would be better to hold a stack of iterators through children, but I
    // couldn't figure out quite a good way to do that in rust. I think it would
    // mean changing the delegate's for_each_children to return an iterator,
    // but idk.

    let mut visited: BuckMutSet<T::Key> = BuckMutSet::default();
    let mut work: Vec<WorkItem<T>> = root.into_iter().map(|t| WorkItem::Visit(t)).collect();

    while let Some(curr) = work.pop() {
        match curr {
            WorkItem::Visit(target) => {
                if visited.contains(&target) {
                    continue;
                }

                let node = nodes.get(&target)?;
                visited.insert(target);
                work.push(WorkItem::PostVisit(node.dupe()));

                successors
                    .for_each_child(&node, &mut |child: &T::Key| {
                        if !visited.contains(child) {
                            work.push(WorkItem::Visit(child.clone()));
                        }
                        Ok(())
                    })
                    .await?;
            }
            WorkItem::PostVisit(target) => {
                visit(target)?;
            }
        }
    }

    Ok(())
}

pub async fn async_depth_limited_traversal<
    'a,
    T: LabeledNode + 'static,
    RootIter: IntoIterator<Item = &'a T::Key>,
>(
    nodes: &impl AsyncNodeLookup<T>,
    root: RootIter,
    successors: impl AsyncChildVisitor<T>,
    mut visit: impl FnMut(T) -> buck2_error::Result<()>,
    max_depth: u32,
    allow_partial_graph: bool,
) -> buck2_error::Result<()> {
    let mut visited: BuckMutMap<_, _> = BuckMutMap::default();
    let mut push =
        |queue: &mut FuturesOrdered<_>, target: &T::Key, parent: Option<T::Key>, depth: u32| {
            if visited.contains_key(target) {
                return;
            }
            visited.insert(target.clone(), parent.clone());
            let target = target.clone();
            queue.push_back(async move {
                let result = nodes.get_from(&target, parent.as_ref()).await;
                (target, depth, result)
            })
        };

    let mut queue = FuturesOrdered::new();

    for target in root {
        push(&mut queue, target, None, 0);
    }

    // TODO(cjhopman): FuturesOrdered/Unordered interacts poorly with tokio cooperative scheduling
    // (see https://github.com/rust-lang/futures-rs/issues/2053). Clean this up once a good
    // solution there exists.
    while let Some((target, depth, node)) = tokio::task::unconstrained(queue.next()).await {
        // `allow_partial_graph` only skips graph load / enumeration errors: a node that does not
        // load is dropped, and a node whose children cannot be enumerated is kept without them,
        // as `Graph::build` does. A `visit` error is the caller's own error and always
        // propagates.
        let node = match node {
            Ok(node) => node,
            Err(mut e) => {
                if allow_partial_graph {
                    tracing::trace!(
                        "query allow-partial-graph: skipping `{target}` due to error: {e:#}"
                    );
                    continue;
                }
                let mut target = target;
                while let Some(Some(parent)) = visited.get(&target) {
                    e = e.context(format!("Error traversing children of {parent}"));
                    target = parent.clone();
                }
                return Err(e);
            }
        };

        if depth != max_depth {
            let depth = depth + 1;
            let children = successors
                .for_each_child(&node, &mut |child: &T::Key| {
                    push(&mut queue, child, Some(target.clone()), depth);
                    Ok(())
                })
                .await;
            if let Err(mut e) = children {
                if allow_partial_graph {
                    tracing::trace!(
                        "query allow-partial-graph: skipping children of `{target}` due to error: {e:#}"
                    );
                } else {
                    let mut target = target;
                    while let Some(Some(parent)) = visited.get(&target) {
                        e = e.context(format!("Error traversing children of {parent}"));
                        target = parent.clone();
                    }
                    return Err(e);
                }
            }
        }

        visit(node)?;
    }

    Ok(())
}

/// Implements a depth-first postorder traversal. A node will be visited only after all of its
/// dependencies have been visited.
// TODO(cjhopman): Accept a generic iterator for the roots. We need to iterate over it twice and it's only used with this specific iterator so it was easier to not be generic.
pub async fn async_depth_first_postorder_traversal<
    'a,
    T: LabeledNode,
    Iter: IntoIterator<Item = &'a T::Key> + Clone,
>(
    nodes: &impl AsyncNodeLookup<T>,
    root: Iter,
    successors: impl AsyncChildVisitor<T>,
    mut visit: impl FnMut(T) -> buck2_error::Result<()>,
    allow_partial_graph: bool,
) -> buck2_error::Result<()> {
    let graph = Graph::build(
        nodes,
        root.clone().into_iter().cloned(),
        successors,
        allow_partial_graph,
    )
    .await?;

    graph.depth_first_postorder_traversal(root.into_iter().cloned(), |node| visit(node.dupe()))
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use buck2_core::build_file_path::BuildFilePath;
    use buck2_core::cells::cell_path::CellPath;
    use buck2_hash::BuckMutMap;
    use derive_more::Display;
    use dupe::Dupe;
    use dupe::IterDupedExt;

    use super::*;
    use crate::query::environment::QueryTarget;
    use crate::query::graph::node::NodeKey;
    use crate::query::syntax::simple::eval::set::TargetSet;

    #[derive(Debug, Clone)]
    struct Node(Ref, Vec<Ref>);

    // For tests, we don't care that this Dupe impl is slow.
    impl Dupe for Node {}

    #[derive(Debug, Clone, Dupe, Hash, Display, PartialEq, Eq, PartialOrd, Ord)]
    struct Ref(i64);

    impl NodeKey for Ref {}

    #[derive(Debug, Display)]
    struct Attr(String);

    impl LabeledNode for Node {
        type Key = Ref;

        fn node_key(&self) -> &Self::Key {
            &self.0
        }
    }

    impl QueryTarget for Node {
        type Attr<'a> = Attr;

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
            self.1.iter()
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

        fn attr_any_matches(
            _attr: &Self::Attr<'_>,
            _filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
        ) -> buck2_error::Result<bool> {
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

        fn inputs_for_each<E: From<buck2_error::Error>, F: FnMut(CellPath) -> Result<(), E>>(
            &self,
            _func: F,
        ) -> Result<(), E> {
            unimplemented!()
        }

        fn exec_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
            let _iterator: Box<dyn Iterator<Item = &'a Self::Key> + Send + 'a>;
            unimplemented!();
            #[allow(unreachable_code)]
            _iterator
        }

        fn target_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
            let _iterator: Box<dyn Iterator<Item = &'a Self::Key> + Send + 'a>;
            unimplemented!();
            #[allow(unreachable_code)]
            _iterator
        }

        fn configuration_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
            let _iterator: Box<dyn Iterator<Item = &'a Self::Key> + Send + 'a>;
            unimplemented!();
            #[allow(unreachable_code)]
            _iterator
        }

        fn toolchain_deps<'a>(&'a self) -> impl Iterator<Item = &'a Self::Key> + Send + 'a {
            let _iterator: Box<dyn Iterator<Item = &'a Self::Key> + Send + 'a>;
            unimplemented!();
            #[allow(unreachable_code)]
            _iterator
        }
    }

    struct Graph(BuckMutMap<Ref, Node>);

    impl Graph {
        fn child_visitor<'a>(&self) -> impl AsyncChildVisitor<Node> + use<'a> {
            struct ChildVisitorImpl;

            impl AsyncChildVisitor<Node> for ChildVisitorImpl {
                async fn for_each_child(
                    &self,
                    target: &Node,
                    mut func: impl ChildVisitor<Node>,
                ) -> buck2_error::Result<()> {
                    for child in &target.1 {
                        func.visit(child)?;
                    }
                    Ok(())
                }
            }

            ChildVisitorImpl
        }

        fn async_node_lookup(&self) -> impl AsyncNodeLookup<Node> + '_ {
            struct Lookup<'a>(&'a Graph);

            #[async_trait]
            impl<'a> AsyncNodeLookup<Node> for Lookup<'a> {
                async fn get(&self, label: &Ref) -> buck2_error::Result<Node> {
                    self.0.get(label)
                }
            }

            Lookup(self)
        }
    }

    impl NodeLookup<Node> for Graph {
        fn get(&self, label: &Ref) -> buck2_error::Result<Node> {
            self.0
                .get(label)
                .ok_or_else(|| {
                    buck2_error::buck2_error!(buck2_error::ErrorTag::Tier0, "missing node")
                })
                .map(|v| v.dupe())
        }
    }

    fn make_graph(nodes: &[(i64, &[i64])]) -> buck2_error::Result<Graph> {
        let mut map = BuckMutMap::default();
        for (n, deps) in nodes {
            map.insert(
                Ref(*n),
                Node(Ref(*n), deps.iter().map(|v| Ref(*v)).collect::<Vec<_>>()),
            );
        }
        Ok(Graph(map))
    }

    #[tokio::test]
    async fn test_async_depth_first_postorder_traversal() -> buck2_error::Result<()> {
        let graph = make_graph(&[
            (0, &[1, 2]),
            (1, &[2, 3, 4]),
            (2, &[3, 4]),
            (3, &[4]),
            (4, &[]),
        ])?;
        let mut targets = TargetSet::new();
        targets.insert(graph.get(&Ref(0))?);

        let mut results = Vec::new();
        {
            let child_visitor = graph.child_visitor();
            async_depth_first_postorder_traversal(
                &graph.async_node_lookup(),
                targets.iter_names(),
                child_visitor,
                |n| {
                    results.push(n.0);
                    Ok(())
                },
                false, // allow_partial_graph
            )
            .await?;
        }

        assert_eq!(results, vec![Ref(4), Ref(3), Ref(2), Ref(1), Ref(0)]);

        Ok(())
    }

    #[tokio::test]
    async fn test_async_depth_limited_traversal() -> buck2_error::Result<()> {
        let graph = make_graph(&[
            (0, &[1, 2]),
            (1, &[2, 3, 4]),
            (2, &[3, 4]),
            (3, &[4]),
            (4, &[]),
        ])?;
        let mut targets = TargetSet::new();
        targets.insert(graph.get(&Ref(0))?);

        let mut results0 = Vec::new();
        {
            let delegate = graph.child_visitor();
            async_depth_limited_traversal(
                &graph.async_node_lookup(),
                targets.iter_names(),
                delegate,
                |n| {
                    results0.push(n.0);
                    Ok(())
                },
                0,
                false, // allow_partial_graph
            )
            .await?;
        }
        assert_eq!(results0, vec![Ref(0)]);

        let mut results1 = Vec::new();
        {
            let delegate = graph.child_visitor();
            async_depth_limited_traversal(
                &graph.async_node_lookup(),
                targets.iter_names(),
                delegate,
                |n| {
                    results1.push(n.0);
                    Ok(())
                },
                1,
                false, // allow_partial_graph
            )
            .await?;
        }
        assert_eq!(results1, vec![Ref(0), Ref(1), Ref(2)]);

        Ok(())
    }

    #[tokio::test]
    async fn test_async_depth_limited_traversal_visit_error_propagates() -> buck2_error::Result<()>
    {
        // A `visit` error is the caller's own error, not a graph-load failure, so it must
        // propagate even when `allow_partial_graph` is set.
        let graph = make_graph(&[(0, &[1]), (1, &[])])?;
        let mut targets = TargetSet::new();
        targets.insert(graph.get(&Ref(0))?);

        let delegate = graph.child_visitor();
        let result = async_depth_limited_traversal(
            &graph.async_node_lookup(),
            targets.iter_names(),
            delegate,
            |_n| -> buck2_error::Result<()> {
                Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "visit error"
                ))
            },
            1,
            true, // allow_partial_graph
        )
        .await;

        let err = result
            .expect_err("visit error should propagate, not be swallowed by allow_partial_graph");
        assert!(
            format!("{err:#}").contains("visit error"),
            "unexpected error: {err:#}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_async_fast_depth_first_postorder_traversal() -> buck2_error::Result<()> {
        let graph = make_graph(&[
            (0, &[1, 2]),
            (1, &[2, 3, 4]),
            (2, &[3, 4]),
            (3, &[4]),
            (4, &[]),
        ])?;
        let mut targets = TargetSet::new();
        targets.insert(graph.get(&Ref(0))?);

        let mut results = Vec::new();
        {
            async_fast_depth_first_postorder_traversal(
                &graph,
                targets.iter_names().duped(),
                graph.child_visitor(),
                |n| {
                    results.push(n.0);
                    Ok(())
                },
            )
            .await?;
        }
        assert_eq!(results, vec![Ref(4), Ref(3), Ref(2), Ref(1), Ref(0)]);

        Ok(())
    }
}
