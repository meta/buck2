/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! `commit` (`incrementality.md` §5.3).

use std::collections::VecDeque;

use crate::arc::Arc;
use crate::cert::Cert;
use crate::collections::KeyMap;
use crate::collections::KeySet;
use crate::env::Env;
use crate::history::Entry;
use crate::ids::BranchId;
use crate::ids::EpsilonToken;
use crate::ids::Key;
use crate::ids::Revision;
use crate::ids::Seq;
use crate::ids::Version;
use crate::resolve::Attached;
use crate::resolve::Resolved;
use crate::slot::Claim;
use crate::slot::Window;
use crate::state::CoreState;

/// One change of a commit (§2.2).
pub enum Change<T> {
    /// `key` has `revision` from this commit on.
    Assert {
        key: Key,
        revision: Revision,
        data: T,
    },
    /// `key`'s untracked input has changed.
    Dirty { key: Key, data: T },
}

enum Close<E: Env> {
    Own(Arc<Cert<E::Premises>>),
    Inherited(Claim<E>),
    NotAttached,
}

impl<E: Env> CoreState<E> {
    /// `commit(b, changes)` (§5.3). Returns the new head, which is the old one if no change had
    /// an effect.
    ///
    /// Panics if a change asserts a key that has been certified or dirties one that has been
    /// asserted, no key being both, and if a key appears in `changes` more than once.
    pub fn commit(
        &mut self,
        b: BranchId,
        changes: impl IntoIterator<Item = Change<E::AssertionData>>,
    ) -> Version {
        let head = self.head(b);
        let next = head.seq().next();
        let mut queue = VecDeque::new();
        let mut seen = KeySet::default();
        for change in changes {
            let key = match &change {
                Change::Assert { key, .. } | Change::Dirty { key, .. } => *key,
            };
            assert!(seen.insert(key), "{key:?} appears twice in one commit");
            match change {
                Change::Assert {
                    key,
                    revision,
                    data,
                } => {
                    assert!(
                        !self.slots.contains_key(&key),
                        "{key:?} is asserted after being certified or dirtied; a key is never both"
                    );
                    if let Resolved::Asserted { entry, .. } = self.resolve(key, head)
                        && entry.revision == revision
                    {
                        continue;
                    }
                    self.assertion_history_mut(key, b).push(Entry {
                        seq: next,
                        revision,
                        data,
                    });
                }
                Change::Dirty { key, data } => {
                    assert!(
                        !self.is_asserted(key),
                        "{key:?} is dirtied after being asserted; a key is never both"
                    );
                    self.slot_or_insert(key, b).untracked.push(Entry {
                        seq: next,
                        revision: EpsilonToken::mint(),
                        data,
                    });
                }
            }
            queue.push_back(key);
        }
        if !queue.is_empty() {
            self.close_dependents(b, next, queue);
            self.branch_mut(b).head = next;
        }
        self.head(b)
    }

    /// The BFS of §5.3 from all changed keys in `queue`, closing at `at` everything attached at
    /// `b` that depends on them, the changed keys included. Edges into keys the walk closes are
    /// dropped, and closed keys are removed from the edges of premises the walk did not reach,
    /// so the map stays exact.
    fn close_dependents(&mut self, b: BranchId, at: Seq, mut queue: VecDeque<Key>) {
        let first = self.branch(b).first;
        let mut visited = KeySet::default();
        let mut closed: Vec<(Key, Arc<Cert<E::Premises>>)> = Vec::new();
        while let Some(key) = queue.pop_front() {
            if !visited.insert(key) {
                continue;
            }
            let close = match self.attached(key, b) {
                Some(Attached::Own(claim)) => Close::Own(claim.cert.clone()),
                Some(Attached::Inherited { claim, .. }) => Close::Inherited(Claim {
                    cert: claim.cert.clone(),
                    window: Window::closed(first, at),
                    attribute: claim.attribute,
                }),
                None => Close::NotAttached,
            };
            let closed_cert = match close {
                Close::Own(cert) => {
                    self.detach(key, b, at);
                    Some(cert)
                }
                Close::Inherited(copy) => {
                    let cert = copy.cert.clone();
                    self.set_claim(key, b, Some(copy));
                    Some(cert)
                }
                Close::NotAttached => None,
            };
            if let Some(cert) = closed_cert {
                closed.push((key, cert));
            }
            let dependents = self.branch_mut(b).take_rdeps(key);
            queue.extend(dependents.iter().copied());
        }
        // Every visited key had its edges drained; what remains are the closed keys' entries
        // under premises the walk did not reach. Grouped by premise so each edge list is
        // scanned once per commit. In a full invalidation there are none, so this costs nothing then.
        let mut removals: KeyMap<KeySet> = KeyMap::default();
        for (key, cert) in &closed {
            for premise in cert.premises() {
                if !visited.contains(&premise.key) {
                    removals.entry(premise.key).or_default().insert(*key);
                }
            }
        }
        for (dep, dependents) in removals {
            self.branch_mut(b).remove_edges(dep, &dependents);
        }
    }
}
