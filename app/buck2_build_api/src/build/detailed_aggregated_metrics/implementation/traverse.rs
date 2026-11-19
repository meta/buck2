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

use buck2_artifact::actions::key::ActionKey;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_error::BuckErrorOptionContext;
use buck2_hash::BuckMutSet;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use dupe::Dupe;
use starlark::values::OwnedFrozen;
use starlark::values::ValueTyped;

use crate::actions::RegisteredAction;
use crate::artifact_groups::ArtifactGroup;
use crate::deferred::calculation::ActionLookup;
use crate::deferred::calculation::OwnedDeferredHolder;
use crate::interpreter::rule_defs::artifact_tagging::ArtifactTag;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::InputProperties;
use crate::interpreter::rule_defs::transitive_set::TransitiveSet;

#[derive(Clone, Dupe, Debug, Eq, PartialEq, Hash)]
enum Key {
    Action(ActionKey),
    ResolvedTransitiveSetProjection(ResolvedTransitiveSetProjection),
    MissingAnalysis(DeferredHolderKey),
}

impl Eq for ResolvedTransitiveSetProjection {}

impl PartialEq for ResolvedTransitiveSetProjection {
    fn eq(&self, other: &Self) -> bool {
        self.projection == other.projection
            && self
                .tset
                .by_ref(|s| other.tset.by_ref(|o| s.key() == o.key()))
    }
}

impl std::hash::Hash for ResolvedTransitiveSetProjection {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.tset
            .by_ref(|t| t.get_projection_key(self.projection).hash(state));
    }
}

/// Once we look up a transitive set by key once, we have a reference to the tset itself. That tset contains direct
/// references to its children, so we can use that to look up the children rather than needing to look up through
/// the key again.
#[derive(Clone, Dupe, Debug)]
struct ResolvedTransitiveSetProjection {
    projection: usize,
    tset: OwnedFrozen<ValueTyped<'static, TransitiveSet<'static>>>,
}

#[derive(Clone, Dupe, Debug)]
enum Node {
    Action(Arc<RegisteredAction>),
    /// Used when we try to look up something from an analysis that is missing. This should only
    /// happen for dynamic nodes (which would be missing if the inputs failed or the analysis of
    /// node itself failed). We wouldn't encounter this for other nodes because we wouldn't have
    /// any actions/etc that could refer to them if they are missing.
    Missing(DeferredHolderKey),
    TransitiveSetProjection(ResolvedTransitiveSetProjection),
}

impl Node {
    fn is_missing(&self) -> bool {
        matches!(self, Node::Missing(..))
    }
}

struct Graph<'a>(&'a buck2_hash::BuckMutMap<DeferredHolderKey, OwnedDeferredHolder>);

impl<'a> Graph<'a> {
    pub(crate) fn lookup_deferred(
        &self,
        holder_key: &DeferredHolderKey,
    ) -> Option<&OwnedDeferredHolder> {
        self.0.get(holder_key)
    }

    fn lookup_node(&self, key: Key) -> buck2_error::Result<Node> {
        match key {
            Key::Action(action_key) => match self.lookup_deferred(action_key.holder_key()) {
                Some(v) => match v.as_ref().lookup_action(&action_key)? {
                    ActionLookup::Action(registered_action) => Ok(Node::Action(registered_action)),
                    ActionLookup::Deferred(action_key) => self.lookup_node(Key::Action(action_key)),
                },
                None => Ok(Node::Missing(action_key.holder_key().dupe())),
            },
            Key::ResolvedTransitiveSetProjection(proj) => {
                Ok(Node::TransitiveSetProjection(proj.dupe()))
            }
            Key::MissingAnalysis(key) => Ok(Node::Missing(key)),
        }
    }

    fn lookup_artifact(&self, artifact: &ArtifactGroup) -> buck2_error::Result<Option<Key>> {
        match artifact {
            ArtifactGroup::Artifact(artifact) => {
                Ok(artifact.action_key().map(|key| Key::Action(key.dupe())))
            }
            ArtifactGroup::TransitiveSetProjection(key) => {
                let holder_key = key.key.key.holder_key();
                match self.lookup_deferred(holder_key) {
                    Some(v) => {
                        let tset = v.as_ref().lookup_transitive_set(&key.key.key)?;
                        Ok(Some(Key::ResolvedTransitiveSetProjection(
                            ResolvedTransitiveSetProjection {
                                projection: key.key.projection,
                                tset,
                            },
                        )))
                    }
                    None => Ok(Some(Key::MissingAnalysis(holder_key.dupe()))),
                }
            }
            ArtifactGroup::Promise(p) => Ok(p
                .promise_artifact
                .unwrap()
                .action_key()
                .map(|key| Key::Action(key.dupe()))),
        }
    }

    fn visit_deps(&self, node: &Node, mut visit_dep: impl FnMut(Key)) -> buck2_error::Result<()> {
        match node {
            Node::Action(action) => {
                for artifact in action.inputs()?.iter() {
                    if let Some(v) = self.lookup_artifact(artifact)? {
                        visit_dep(v);
                    }
                }
                if action.key().holder_key().is_dynamic() {
                    // TODO(cjhopman): Should traverse to the dynamic node's inputs and parent dynamic node.
                }
            }
            Node::TransitiveSetProjection(proj_node) => {
                for idx in 0..proj_node.tset.by_ref(|t| t.children.len()) {
                    visit_dep(Key::ResolvedTransitiveSetProjection(
                        ResolvedTransitiveSetProjection {
                            projection: proj_node.projection,
                            tset: proj_node.tset.dupe().try_map(|v| {
                                ValueTyped::new(v.children.read()?[idx])
                                    .internal_error("tset children should be tsets")
                            })?,
                        },
                    ))
                }
                struct Visitor<'a, 'b, F: FnMut(Key)>(F, &'a Graph<'b>, buck2_error::Result<()>);
                impl<'v, F: FnMut(Key)> CommandLineArtifactVisitor<'v> for Visitor<'_, '_, F> {
                    fn visit_input(
                        &mut self,
                        input: ArtifactGroup,
                        _props: InputProperties,
                        _tags: Vec<&ArtifactTag>,
                    ) {
                        if self.2.is_err() {
                            return;
                        }

                        match self.1.lookup_artifact(&input) {
                            Ok(Some(v)) => {
                                self.0(v);
                            }
                            Ok(None) => {}
                            Err(e) => {
                                self.2 = Err(e);
                            }
                        }
                    }

                    fn visit_declared_output(
                        &mut self,
                        _artifact: OutputArtifact<'v>,
                        _tags: Vec<&ArtifactTag>,
                    ) {
                    }

                    fn visit_frozen_output(
                        &mut self,
                        _artifact: Artifact,
                        _tags: Vec<&ArtifactTag>,
                    ) {
                    }
                }
                let mut visitor = Visitor(&mut visit_dep, self, Ok(()));
                proj_node.tset.by_ref(|t| {
                    t.visit_projection_direct_inputs(proj_node.projection, &mut visitor)
                })?;
                visitor.2?;
            }
            Node::Missing(_holder_key) => {
                // TODO(cjhopman): Should traverse to the dynamic node's inputs and parent dynamic node.
            }
        }
        Ok(())
    }

    fn traverse(
        &self,
        roots: impl IntoIterator<Item = Key>,
        mut visit: impl FnMut(&Self, &Node, &mut TraversalState) -> buck2_error::Result<()>,
    ) -> buck2_error::Result<bool> {
        let mut state = TraversalState::new();
        let mut complete = true;

        for root in roots {
            state.enqueue(root);
        }

        // Several keys can resolve to one action: each output of a dynamic_output is
        // its own key bound to the action that produces it.
        let mut visited_actions = BuckMutSet::default();
        while let Some(item) = state.pop() {
            let node = self.lookup_node(item)?;
            complete = complete && !node.is_missing();
            if let Node::Action(action) = &node
                && !visited_actions.insert(action.key().dupe())
            {
                continue;
            }
            visit(self, &node, &mut state)?;
        }

        Ok(complete)
    }

    fn root_keys<'b>(
        &self,
        root_artifacts: impl IntoIterator<Item = &'b ArtifactGroup>,
    ) -> buck2_error::Result<Vec<Key>> {
        root_artifacts
            .into_iter()
            .filter_map(|a| self.lookup_artifact(a).transpose())
            .collect()
    }
}

struct TraversalState {
    visited: BuckMutSet<Key>,
    queue: Vec<Key>,
}

impl TraversalState {
    fn new() -> Self {
        Self {
            visited: BuckMutSet::default(),
            queue: Vec::new(),
        }
    }

    fn enqueue(&mut self, item: Key) {
        if !self.visited.contains(&item) {
            self.visited.insert(item.dupe());
            self.queue.push(item);
        }
    }

    fn pop(&mut self) -> Option<Key> {
        self.queue.pop()
    }
}

pub fn traverse_partial_action_graph<'a>(
    root_artifacts: impl IntoIterator<Item = &'a ArtifactGroup>,
    state: &buck2_hash::BuckMutMap<DeferredHolderKey, OwnedDeferredHolder>,
) -> buck2_error::Result<(bool, BuckMutSet<ActionKey>)> {
    let mut actions = BuckMutSet::default();

    let graph = Graph(state);
    let roots = graph.root_keys(root_artifacts)?;

    let complete = graph.traverse(roots, |graph, node, state| {
        // It's required that we add this on visiting an action node rather than an action key.
        // dynamic outputs produces action keys that point to other action keys,
        // and we don't want to add those.
        if let Node::Action(action) = node {
            actions.insert(action.key().dupe());
        }
        graph.visit_deps(node, |dep| state.enqueue(dep))
    })?;

    Ok((complete, actions))
}

pub fn traverse_target_graph(
    root: &ConfiguredTargetNode,
    mut visitor: impl FnMut(&ConfiguredTargetLabel),
) {
    let mut visited = BuckMutSet::default();
    let mut queue = Vec::new();
    visited.insert(root.label());
    queue.push(root);
    while let Some(v) = queue.pop() {
        visitor(v.label());
        for dep in v.deps() {
            if visited.insert(dep.label()) {
                queue.push(dep);
            }
        }
    }
}
