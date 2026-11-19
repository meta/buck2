/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Traversals over the mixed graph of action nodes and tset projection nodes.
//!
//! The flattened `QueryTarget::deps()` view expands each action's deps to its full
//! transitive tset contents, so traversals over it are O(n^2) in a typical build graph.
//! The traversals here visit each tset projection node once instead, which is
//! O(nodes + edges) — the sharing tsets exist to provide. The visited action sets are
//! identical: an action is in the flattened deps closure iff it is reachable through the
//! mixed graph, and the flattened depth of an action equals the number of action nodes
//! entered along a shortest mixed path (passing through tset nodes is free).

use std::collections::VecDeque;

use async_trait::async_trait;
use buck2_build_api::actions::query::ActionInput;
use buck2_build_api::actions::query::ActionQueryNode;
use buck2_build_api::actions::query::ActionQueryNodeData;
use buck2_build_api::actions::query::ActionQueryNodeRef;
use buck2_build_api::actions::query::SetProjectionInputs;
use buck2_build_api::artifact_groups::TransitiveSetProjectionKey;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_query::query::environment::QueryEnvironment;
use buck2_query::query::graph::async_bfs::async_bfs_find_path;
use buck2_query::query::graph::node::LabeledNode;
use buck2_query::query::graph::node::NodeKey;
use buck2_query::query::graph::successors::AsyncChildVisitor;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_query::query::traversal::AsyncNodeLookup;
use buck2_query::query::traversal::ChildVisitor;
use buck2_query::query::traversal::async_depth_first_postorder_traversal;
use dupe::Dupe;

use crate::aquery::environment::AqueryEnvironment;

/// Computes unbounded, unfiltered `deps()`.
pub(crate) async fn aquery_deps_unbounded_unfiltered(
    env: &AqueryEnvironment<'_>,
    targets: &TargetSet<ActionQueryNode>,
) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
    let roots: Vec<AqueryMixedKey> = targets
        .iter_names()
        .map(|k| AqueryMixedKey::Action(k.dupe()))
        .collect();

    let mut deps = TargetSet::new();
    async_depth_first_postorder_traversal(
        &MixedLookup {
            roots: targets,
            env,
        },
        roots.iter(),
        MixedSuccessors,
        |node| {
            if let AqueryMixedData::Action(action) = node.data {
                deps.insert_unique_unchecked(action);
            }
            Ok(())
        },
        env.allow_partial_graph(),
    )
    .await?;
    Ok(deps)
}

/// Computes depth-bounded, unfiltered `deps()` with a level-synchronized traversal:
/// actions at flattened depth `d + 1` are exactly the direct members of the tset closure
/// of depth-`d` actions' inputs. Tset projection nodes already expanded at an earlier
/// (necessarily shallower or equal) level are skipped, since the actions they contribute
/// were already discovered at a depth no greater than they would get here.
pub(crate) async fn aquery_deps_bounded_unfiltered(
    env: &AqueryEnvironment<'_>,
    targets: &TargetSet<ActionQueryNode>,
    depth: u32,
) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
    let mut deps = TargetSet::new();
    let mut visited: BuckMutSet<ActionQueryNodeRef> = BuckMutSet::default();
    let mut expanded_tsets: BuckMutSet<SetProjectionInputs> = BuckMutSet::default();

    let mut frontier: Vec<ActionQueryNode> = Vec::new();
    for node in targets.iter() {
        if visited.insert(node.node_key().dupe()) {
            deps.insert_unique_unchecked(node.dupe());
            frontier.push(node.dupe());
        }
    }

    for _ in 0..depth {
        let mut next_keys: Vec<ActionQueryNodeRef> = Vec::new();
        for node in &frontier {
            for_each_child_action(node, &mut expanded_tsets, |key| {
                if visited.insert(key.dupe()) {
                    next_keys.push(key.dupe());
                }
            });
        }
        if next_keys.is_empty() {
            break;
        }

        frontier = buck2_util::future::try_join_all(
            next_keys
                .iter()
                .map(|key| QueryEnvironment::get_node(env, key)),
        )
        .await?;
        for node in &frontier {
            deps.insert_unique_unchecked(node.dupe());
        }
    }
    Ok(deps)
}

/// Computes unfiltered `rdeps()` (and thereby `allpaths()`); `depth` of `None` is
/// unbounded.
pub(crate) async fn aquery_rdeps_unfiltered(
    env: &AqueryEnvironment<'_>,
    universe: &TargetSet<ActionQueryNode>,
    from: &TargetSet<ActionQueryNode>,
    depth: Option<u32>,
) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
    let graph = MixedGraph::build(env, universe).await?;
    let reversed = graph.reverse_children();

    let roots: Vec<u32> = from
        .iter_names()
        .filter_map(|key| {
            graph
                .key_to_index
                .get(&AqueryMixedKey::Action(key.dupe()))
                .copied()
        })
        .collect();

    let allowed = depth.map(|depth| graph.reachable_within_action_depth(&reversed, &roots, depth));

    let mut rdeps = TargetSet::new();
    graph.postorder_actions(
        &reversed,
        &roots,
        |index| {
            allowed
                .as_ref()
                .is_none_or(|allowed| allowed[index as usize])
        },
        |node| {
            rdeps.insert_unique_unchecked(node.dupe());
            Ok(())
        },
    )?;
    Ok(rdeps)
}

/// Computes unfiltered `somepath()`. The returned path visits actions connected by
/// flattened edges, in `from` to `to` order, like the flattened implementation (though not
/// necessarily the same path).
pub(crate) async fn aquery_somepath_unfiltered(
    env: &AqueryEnvironment<'_>,
    from: &TargetSet<ActionQueryNode>,
    to: &TargetSet<ActionQueryNode>,
) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
    let roots: Vec<AqueryMixedNode> = from
        .iter()
        .map(|node| AqueryMixedNode {
            key: AqueryMixedKey::Action(node.node_key().dupe()),
            data: AqueryMixedData::Action(node.dupe()),
        })
        .collect();

    let path = async_bfs_find_path(
        roots.iter(),
        MixedLookup { roots: from, env },
        MixedSuccessors,
        |key| match key {
            AqueryMixedKey::Action(action) => to.get(action).map(|node| AqueryMixedNode {
                key: key.dupe(),
                data: AqueryMixedData::Action(node.dupe()),
            }),
            AqueryMixedKey::TsetProjection(..) => None,
        },
        env.allow_partial_graph(),
    )
    .await?
    .unwrap_or_default();

    let mut result = TargetSet::new();
    for node in path {
        if let AqueryMixedData::Action(action) = node.data {
            result.insert_unique_unchecked(action);
        }
    }
    Ok(result)
}

#[derive(Debug, Clone, Dupe, Eq, PartialEq, Hash, derive_more::Display)]
enum AqueryMixedKey {
    Action(ActionQueryNodeRef),
    TsetProjection(TransitiveSetProjectionKey),
}

impl NodeKey for AqueryMixedKey {}

#[derive(Clone, Dupe)]
struct AqueryMixedNode {
    key: AqueryMixedKey,
    data: AqueryMixedData,
}

#[derive(Clone, Dupe)]
enum AqueryMixedData {
    Action(ActionQueryNode),
    TsetProjection(SetProjectionInputs),
}

impl LabeledNode for AqueryMixedNode {
    type Key = AqueryMixedKey;

    fn node_key(&self) -> &Self::Key {
        &self.key
    }
}

fn for_each_child_key(
    node: &AqueryMixedNode,
    mut f: impl FnMut(&AqueryMixedKey) -> buck2_error::Result<()>,
) -> buck2_error::Result<()> {
    match &node.data {
        AqueryMixedData::Action(action) => match action.data() {
            ActionQueryNodeData::Action(data) => {
                for input in data.inputs() {
                    match input {
                        ActionInput::ActionKey(action_key) => {
                            f(&AqueryMixedKey::Action(action_key.dupe()))?;
                        }
                        ActionInput::IndirectInputs(tset) => {
                            f(&AqueryMixedKey::TsetProjection(tset.node.key().dupe()))?;
                        }
                    }
                }
            }
            // Analysis nodes are just literals, we don't traverse their deps.
            ActionQueryNodeData::Analysis(..) => {}
        },
        AqueryMixedData::TsetProjection(inputs) => {
            for action_key in &inputs.node.direct {
                f(&AqueryMixedKey::Action(action_key.dupe()))?;
            }
            for child in &inputs.node.children {
                f(&AqueryMixedKey::TsetProjection(child.node.key().dupe()))?;
            }
        }
    }
    Ok(())
}

/// Enumerates the flattened first-order child actions of `node`, mirroring
/// `iter_action_inputs` except that tset projection nodes in `expanded_tsets` are skipped
/// and newly walked ones are added to it.
fn for_each_child_action(
    node: &ActionQueryNode,
    expanded_tsets: &mut BuckMutSet<SetProjectionInputs>,
    mut f: impl FnMut(&ActionQueryNodeRef),
) {
    let data = match node.data() {
        ActionQueryNodeData::Action(data) => data,
        ActionQueryNodeData::Analysis(..) => return,
    };

    for input in data.inputs() {
        if let ActionInput::ActionKey(action_key) = input {
            f(action_key);
        }
    }

    let mut queue: VecDeque<&SetProjectionInputs> = data
        .inputs()
        .iter()
        .filter_map(|input| match input {
            ActionInput::ActionKey(..) => None,
            ActionInput::IndirectInputs(tset) => Some(tset),
        })
        .filter(|tset| expanded_tsets.insert((*tset).dupe()))
        .collect();

    while let Some(tset) = queue.pop_front() {
        for action_key in &tset.node.direct {
            f(action_key);
        }
        for child in &tset.node.children {
            if expanded_tsets.insert(child.dupe()) {
                queue.push_back(child);
            }
        }
    }
}

struct MixedSuccessors;

impl AsyncChildVisitor<AqueryMixedNode> for MixedSuccessors {
    async fn for_each_child(
        &self,
        node: &AqueryMixedNode,
        mut children: impl ChildVisitor<AqueryMixedNode>,
    ) -> buck2_error::Result<()> {
        for_each_child_key(node, |key| children.visit(key))
    }
}

struct MixedLookup<'a, 'c> {
    roots: &'a TargetSet<ActionQueryNode>,
    env: &'a AqueryEnvironment<'c>,
}

#[async_trait]
impl AsyncNodeLookup<AqueryMixedNode> for MixedLookup<'_, '_> {
    async fn get(&self, key: &AqueryMixedKey) -> buck2_error::Result<AqueryMixedNode> {
        let data = match key {
            AqueryMixedKey::Action(node_ref) => {
                // Lookup in `roots` first since `env.get_node` cannot look up analysis nodes,
                // and roots may be analysis nodes.
                let node = match self.roots.get(node_ref) {
                    Some(v) => v.dupe(),
                    None => QueryEnvironment::get_node(self.env, node_ref).await?,
                };
                AqueryMixedData::Action(node)
            }
            AqueryMixedKey::TsetProjection(projection_key) => AqueryMixedData::TsetProjection(
                self.env.delegate.get_tset_node(projection_key).await?,
            ),
        };
        Ok(AqueryMixedNode {
            key: key.dupe(),
            data,
        })
    }
}

/// The mixed graph of a universe's closure, with nodes and edges resolved to indices.
struct MixedGraph {
    nodes: Vec<AqueryMixedNode>,
    key_to_index: BuckMutMap<AqueryMixedKey, u32>,
    children: Vec<Vec<u32>>,
}

impl MixedGraph {
    async fn build(
        env: &AqueryEnvironment<'_>,
        universe: &TargetSet<ActionQueryNode>,
    ) -> buck2_error::Result<MixedGraph> {
        let root_keys: Vec<AqueryMixedKey> = universe
            .iter_names()
            .map(|k| AqueryMixedKey::Action(k.dupe()))
            .collect();

        let mut nodes: Vec<AqueryMixedNode> = Vec::new();
        let mut key_to_index: BuckMutMap<AqueryMixedKey, u32> = BuckMutMap::default();
        async_depth_first_postorder_traversal(
            &MixedLookup {
                roots: universe,
                env,
            },
            root_keys.iter(),
            MixedSuccessors,
            |node| {
                key_to_index.insert(
                    node.key.dupe(),
                    nodes
                        .len()
                        .try_into()
                        .expect("should never see a mixed graph with more than u32::MAX nodes"),
                );
                nodes.push(node);
                Ok(())
            },
            env.allow_partial_graph(),
        )
        .await?;

        let children = nodes
            .iter()
            .map(|node| {
                let mut children = Vec::new();
                for_each_child_key(node, |key| {
                    // A child can be missing only if `allow_partial_graph` dropped it.
                    if let Some(index) = key_to_index.get(key) {
                        children.push(*index);
                    }
                    Ok(())
                })?;
                Ok(children)
            })
            .collect::<buck2_error::Result<Vec<_>>>()?;

        Ok(MixedGraph {
            nodes,
            key_to_index,
            children,
        })
    }

    fn reverse_children(&self) -> Vec<Vec<u32>> {
        let mut reversed: Vec<Vec<u32>> = vec![Vec::new(); self.nodes.len()];
        for (index, children) in self.children.iter().enumerate() {
            for child in children {
                reversed[*child as usize].push(index as u32);
            }
        }
        reversed
    }

    fn action_node(&self, index: u32) -> Option<&ActionQueryNode> {
        match &self.nodes[index as usize].data {
            AqueryMixedData::Action(action) => Some(action),
            AqueryMixedData::TsetProjection(..) => None,
        }
    }

    /// Marks the nodes reachable from `roots` (which must be action nodes) over `edges`
    /// within `depth` flattened hops: entering an action node costs one hop, passing
    /// through tset projection nodes is free.
    fn reachable_within_action_depth(
        &self,
        edges: &[Vec<u32>],
        roots: &[u32],
        depth: u32,
    ) -> Vec<bool> {
        let mut visited = vec![false; self.nodes.len()];
        let mut frontier: Vec<u32> = Vec::new();
        for &root in roots {
            if !visited[root as usize] {
                visited[root as usize] = true;
                frontier.push(root);
            }
        }

        for _ in 0..depth {
            if frontier.is_empty() {
                break;
            }
            let mut next: Vec<u32> = Vec::new();
            let mut stack = std::mem::take(&mut frontier);
            while let Some(index) = stack.pop() {
                for &child in &edges[index as usize] {
                    if visited[child as usize] {
                        continue;
                    }
                    visited[child as usize] = true;
                    if self.action_node(child).is_some() {
                        next.push(child);
                    } else {
                        stack.push(child);
                    }
                }
            }
            frontier = next;
        }
        visited
    }

    /// Depth-first postorder over `edges` from `roots`, restricted to `allowed` nodes,
    /// visiting only the action nodes.
    fn postorder_actions(
        &self,
        edges: &[Vec<u32>],
        roots: &[u32],
        allowed: impl Fn(u32) -> bool,
        mut visit: impl FnMut(&ActionQueryNode) -> buck2_error::Result<()>,
    ) -> buck2_error::Result<()> {
        enum Visit {
            Enter(u32),
            Exit(u32),
        }

        let mut visited = vec![false; self.nodes.len()];
        let mut stack: Vec<Visit> = Vec::new();
        for &root in roots {
            stack.push(Visit::Enter(root));
            while let Some(item) = stack.pop() {
                match item {
                    Visit::Enter(index) => {
                        if visited[index as usize] || !allowed(index) {
                            continue;
                        }
                        visited[index as usize] = true;
                        stack.push(Visit::Exit(index));
                        for &child in &edges[index as usize] {
                            if !visited[child as usize] && allowed(child) {
                                stack.push(Visit::Enter(child));
                            }
                        }
                    }
                    Visit::Exit(index) => {
                        if let Some(action) = self.action_node(index) {
                            visit(action)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
