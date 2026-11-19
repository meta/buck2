/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use dice_core::BranchId;
use itertools::Itertools;

use crate::HashMap;
use crate::HashSet;
use crate::arc::Arc;
use crate::core::graph::VersionedGraph;
use crate::introspection::graph::CellHistory;
use crate::introspection::graph::GraphNodeKind;
use crate::introspection::graph::KeyID;
use crate::introspection::graph::SerializedGraphNode;
use crate::introspection::graph::VersionNumber;
use crate::key::DiceKey;

pub struct VersionedGraphIntrospectable {
    /// One node per live branch and key with a claim or an assertion on it.
    pub nodes: HashMap<(BranchId, DiceKey), SerializedGraphNode>,
    /// The keys a key's claims depend on, on every live branch together.
    pub edges: HashMap<DiceKey, Arc<Vec<DiceKey>>>,
}

fn key_id(key: DiceKey) -> KeyID {
    KeyID(key.index as usize)
}

impl VersionedGraph {
    /// Every live branch's view of the graph, in the dump format: a key's claim on a branch as
    /// its one valid range, its dirties, its certificate's deps and its rdeps. A key gets no
    /// node on a branch it has neither a claim nor an assertion on.
    pub(crate) fn introspect(&self) -> VersionedGraphIntrospectable {
        let mut nodes = HashMap::default();
        let mut edges: HashMap<DiceKey, Vec<DiceKey>> = HashMap::default();
        for key in self.core().keys() {
            let introspection = self.core().introspect_key(key);
            for branch in self.core().branches() {
                let slot = introspection
                    .slots
                    .iter()
                    .find(|slot| slot.branch == branch);
                let assertions = introspection
                    .assertions
                    .iter()
                    .find_map(|(b, history)| (*b == branch).then_some(history));
                let (valid_ranges, deps): (Vec<_>, Vec<DiceKey>) =
                    match (slot.and_then(|s| s.claim.as_ref()), assertions) {
                        (Some(claim), _) => (
                            vec![(
                                VersionNumber(claim.window.from().get() as usize),
                                claim
                                    .window
                                    .until()
                                    .map(|s| VersionNumber(s.get() as usize)),
                            )],
                            claim.cert.premises().map(|p| p.key).collect(),
                        ),
                        (None, Some(history)) => match history.last() {
                            Some(last) => (
                                vec![(VersionNumber(last.seq.get() as usize), None)],
                                Vec::new(),
                            ),
                            None => continue,
                        },
                        (None, None) => continue,
                    };
                let rdeps: Vec<KeyID> = self
                    .core()
                    .rdeps(branch, key)
                    .iter()
                    .copied()
                    .unique()
                    .map(key_id)
                    .collect();
                let force_dirtied_at = slot
                    .map(|s| {
                        s.untracked
                            .entries()
                            .iter()
                            .map(|e| VersionNumber(e.seq.get() as usize))
                            .collect()
                    })
                    .unwrap_or_default();
                nodes.insert(
                    (branch, key),
                    SerializedGraphNode {
                        node_id: key_id(key),
                        kind: GraphNodeKind::Occupied,
                        history: CellHistory {
                            valid_ranges,
                            force_dirtied_at,
                        },
                        deps: deps.iter().map(|d| key_id(*d)).collect::<HashSet<_>>(),
                        rdeps,
                    },
                );
                edges.entry(key).or_default().extend(deps);
            }
        }
        let edges = edges
            .into_iter()
            .map(|(key, deps)| (key, Arc::new(deps.into_iter().unique().collect())))
            .collect();
        VersionedGraphIntrospectable { nodes, edges }
    }
}
