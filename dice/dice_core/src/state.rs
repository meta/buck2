/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use smallvec::SmallVec;

use crate::arc::Arc;
use crate::branch::Branch;
use crate::cert::Cert;
use crate::collections::KeyMap;
use crate::env::Env;
use crate::history::Entry;
use crate::history::History;
use crate::ids::BranchId;
use crate::ids::EpsilonToken;
use crate::ids::Key;
use crate::ids::Revision;
use crate::ids::Seq;
use crate::ids::Version;
use crate::resolve::Resolved;
use crate::slot::Claim;
use crate::slot::Slot;
use crate::slot::Window;

pub(crate) type KeySlots<E> = SmallVec<[Slot<E>; 1]>;
pub(crate) type KeyAssertions<E> =
    SmallVec<[(BranchId, History<Revision, <E as Env>::AssertionData>); 1]>;

/// The core state (`incrementality.md` §4): the branch tree, one slot per key and branch, and
/// the assertion histories of injected keys.
///
/// Operations are performed in some total order; this type is not itself concurrent. Every
/// operation preserves the master invariant of §4, so [`CoreState::lookup`] is sound (§3.3).
#[derive(Allocative)]
#[allocative(bound = "E: Env")]
pub struct CoreState<E: Env> {
    /// Indexed by [`BranchId`]; `None` once a branch is deleted. Ids are never reused, so a
    /// version of a deleted branch stays distinct from every version of a live one.
    pub(crate) branches: Vec<Option<Branch>>,
    /// A key's slots, one per branch it has a claim or an untracked-input history on. A key stays
    /// in the map once certified, even with no slots left after [`Self::delete_branch`]: that
    /// memory is what lets [`Self::commit`] refuse to assert a key that has been certified.
    pub(crate) slots: KeyMap<KeySlots<E>>,
    pub(crate) assertions: KeyMap<KeyAssertions<E>>,
    /// The number of distinct keys across `slots` and `assertions`. Neither map ever drops a key.
    pub(crate) key_count: usize,
}

/// The answer to a lookup (§2.2, §5.1).
pub enum Lookup<'a, E: Env> {
    /// `revision` is justified for the key at the version; `attribute` is that of the derivation
    /// (§4.7).
    Valid {
        revision: Revision,
        attribute: E::Attribute,
    },
    /// Nothing is known. `candidate` is a certificate the caller may try to re-establish: the
    /// nearest one on the resolution chain, else any claim's, provided it was stamped with the
    /// version's ε, since one that was not can never cover the version. `epsilon` is the revision
    /// of the key's untracked input at the version, for stamping the certificate the caller is
    /// about to produce, and `untracked` the attribute of its assertion, which is part of the
    /// derivation of whatever the caller produces.
    Unknown {
        candidate: Option<&'a Arc<Cert<E::Premises>>>,
        epsilon: EpsilonToken,
        untracked: E::Attribute,
    },
}

impl<'a, E: Env> Lookup<'a, E> {
    pub fn valid_revision(&self) -> Option<Revision> {
        match self {
            Lookup::Valid { revision, .. } => Some(*revision),
            Lookup::Unknown { .. } => None,
        }
    }

    pub fn is_valid(&self) -> bool {
        matches!(self, Lookup::Valid { .. })
    }

    pub fn candidate(&self) -> Option<&'a Arc<Cert<E::Premises>>> {
        match self {
            Lookup::Unknown { candidate, .. } => *candidate,
            Lookup::Valid { .. } => None,
        }
    }
}

impl<E: Env> CoreState<E> {
    /// A state with no branch; [`Self::new_root`] makes one.
    pub fn new() -> Self {
        CoreState {
            branches: Vec::new(),
            slots: KeyMap::default(),
            assertions: KeyMap::default(),
            key_count: 0,
        }
    }

    /// A new root: a branch with no parent, at its initial version. Nothing resolves there but
    /// what is asserted at it and the certificates whose premises hold there.
    pub fn new_root(&mut self) -> BranchId {
        let b = BranchId::from_index(self.branches.len());
        self.branches.push(Some(Branch::root(Seq::FIRST)));
        b
    }

    /// The branch's newest version.
    pub fn head(&self, b: BranchId) -> Version {
        Version::new(b, self.branch(b).head)
    }

    /// The branch's initial version.
    pub fn first(&self, b: BranchId) -> Version {
        Version::new(b, self.branch(b).first)
    }

    /// The version the branch was forked from; `None` for a root.
    pub fn parent(&self, b: BranchId) -> Option<Version> {
        self.branch(b).parent
    }

    /// All live branches, parents before children.
    pub fn branches(&self) -> impl Iterator<Item = BranchId> + '_ {
        self.branches
            .iter()
            .enumerate()
            .filter_map(|(index, branch)| branch.as_ref().map(|_| BranchId::from_index(index)))
    }

    /// Whether `b` has not been deleted.
    pub fn is_live(&self, b: BranchId) -> bool {
        self.branches.get(b.index()).is_some_and(Option::is_some)
    }

    /// `lookup(k, v)` (§5.1).
    pub fn lookup(&self, key: Key, v: Version) -> Lookup<'_, E> {
        match self.resolve(key, v) {
            Resolved::Claim { claim, .. } => Lookup::Valid {
                revision: claim.revision(),
                attribute: claim.attribute,
            },
            Resolved::Asserted { owner, entry } => Lookup::Valid {
                revision: entry.revision,
                attribute: E::assertion(Version::new(owner, entry.seq), entry.data),
            },
            Resolved::Unknown { candidate } => {
                let (epsilon, untracked) = self.untracked(key, v);
                Lookup::Unknown {
                    candidate: candidate.filter(|cert| cert.epsilon == epsilon),
                    epsilon,
                    untracked,
                }
            }
        }
    }

    /// The revision of `key`'s untracked input at `v` and the attribute of its assertion:
    /// [`Env::EMPTY`] before the key is first dirtied.
    pub fn untracked(&self, key: Key, v: Version) -> (EpsilonToken, E::Attribute) {
        match self.resolve_untracked(key, v) {
            Some((owner, entry)) => (
                entry.revision,
                E::assertion(Version::new(owner, entry.seq), entry.data),
            ),
            None => (EpsilonToken::INITIAL, E::EMPTY),
        }
    }

    /// The certificate a lookup of `key` at `v` would offer as its candidate if the untracked
    /// input's revision were not held against it: for environments whose value layer wants that
    /// certificate's value regardless, e.g. to compare a fresh compute against it.
    pub fn nearest_certificate(&self, key: Key, v: Version) -> Option<&Arc<Cert<E::Premises>>> {
        match self.resolve(key, v) {
            Resolved::Unknown { candidate } => candidate,
            Resolved::Claim { .. } | Resolved::Asserted { .. } => None,
        }
    }

    /// Whether `key` has ever been asserted, i.e. is an injected key.
    pub fn is_asserted(&self, key: Key) -> bool {
        self.assertions.contains_key(&key)
    }

    /// The certificates the state retains for `key`: the witnessing certificates of its claims,
    /// one per claim, so a certificate shared by several branches is yielded several times.
    pub fn pinned_certs(&self, key: Key) -> impl Iterator<Item = &Arc<Cert<E::Premises>>> + '_ {
        self.slots
            .get(&key)
            .into_iter()
            .flat_map(|slots| slots.iter())
            .filter_map(|slot| slot.claim.as_ref())
            .map(|claim| &claim.cert)
    }

    /// Whether some claim of `key`, or some entry of its assertion histories, names `revision`.
    /// An environment retaining values by revision can release any that is not.
    pub fn is_referenced(&self, key: Key, revision: Revision) -> bool {
        self.pinned_certs(key).any(|cert| cert.revision == revision)
            || self.assertions.get(&key).is_some_and(|histories| {
                histories
                    .iter()
                    .any(|(_, history)| history.entries().iter().any(|e| e.revision == revision))
            })
    }

    /// Deletes `b`, forgetting its claims, its edges and its histories (§5.5). The branches
    /// forked from `b` move to `b`'s parent, or become roots, and keep resolving exactly what
    /// they resolved before. `b`'s id is never reused, and every operation but [`Self::write`]
    /// panics when given it or one of its versions afterwards; `write` installs nothing there.
    ///
    /// Returns the keys that had a claim or an assertion history on `b`: the only keys for which
    /// [`Self::is_referenced`] may have changed.
    pub fn delete_branch(&mut self, b: BranchId) -> Vec<Key> {
        let branch = self.branches[b.index()]
            .take()
            .unwrap_or_else(|| panic!("{b:?} has been deleted"));
        for &(child, fork_seq) in branch.children.iter() {
            self.materialize_inheritance(b, fork_seq, child);
            self.branch_mut(child).parent = branch.parent;
            if let Some(parent) = branch.parent {
                self.branch_mut(parent.branch())
                    .children
                    .push((child, parent.seq()));
            }
        }
        if let Some(parent) = branch.parent {
            self.branch_mut(parent.branch()).remove_child(b);
        }

        let mut affected = Vec::new();
        for (key, slots) in self.slots.iter_mut() {
            if let Some(index) = slots.iter().position(|slot| slot.branch == b) {
                if slots[index].claim.is_some() {
                    affected.push(*key);
                }
                slots.remove(index);
            }
        }
        for (key, histories) in self.assertions.iter_mut() {
            if let Some(index) = histories.iter().position(|(hb, _)| *hb == b) {
                histories.remove(index);
                affected.push(*key);
            }
        }
        affected
    }

    /// Gives `child`, forked from `b` at `fork_seq`, its own copy of everything it resolved
    /// through `b` (§5.5), so that `b` can go without `child`'s resolution changing: `b`'s claim
    /// of each key `child` has no claim of, open if it covered the fork point and empty
    /// otherwise, and the entry in force at the fork point of each of `b`'s histories, at
    /// `child`'s first seq. A key `child` has a claim of, and a history `child` owns from its
    /// first seq on, never delegated to `b` and need nothing.
    fn materialize_inheritance(&mut self, b: BranchId, fork_seq: Seq, child: BranchId) {
        let first = self.branch(child).first;
        let keys: Vec<Key> = self
            .slots
            .iter()
            .filter(|(_, slots)| slots.iter().any(|slot| slot.branch == b))
            .map(|(key, _)| *key)
            .collect();
        for key in keys {
            let slot = self.slot(key, b).expect("filtered above");
            let claim = slot.claim.as_ref().map(|claim| Claim {
                cert: claim.cert.clone(),
                window: if claim.window.covers(fork_seq) {
                    Window::open(first)
                } else {
                    Window::empty(first)
                },
                attribute: claim.attribute,
            });
            let dirty = slot.untracked.at(fork_seq).map(|entry| Entry {
                seq: first,
                revision: entry.revision,
                data: entry.data,
            });
            if let Some(claim) = claim
                && self
                    .slot(key, child)
                    .is_none_or(|slot| slot.claim.is_none())
            {
                self.set_claim(key, child, Some(claim));
            }
            if let Some(entry) = dirty {
                let untracked = &mut self.slot_or_insert(key, child).untracked;
                if untracked.first().is_none_or(|own| own.seq > first) {
                    untracked.insert_first(entry);
                }
            }
        }

        let asserted: Vec<Key> = self
            .assertions
            .iter()
            .filter(|(_, histories)| histories.iter().any(|(hb, _)| *hb == b))
            .map(|(key, _)| *key)
            .collect();
        for key in asserted {
            let inherited = self.assertions[&key]
                .iter()
                .find(|(hb, _)| *hb == b)
                .and_then(|(_, history)| history.at(fork_seq))
                .map(|entry| Entry {
                    seq: first,
                    revision: entry.revision,
                    data: entry.data,
                });
            if let Some(entry) = inherited {
                let history = self.assertion_history_mut(key, child);
                if history.first().is_none_or(|own| own.seq > first) {
                    history.insert_first(entry);
                }
            }
        }
    }

    pub(crate) fn branch(&self, b: BranchId) -> &Branch {
        self.branches[b.index()]
            .as_ref()
            .unwrap_or_else(|| panic!("{b:?} has been deleted"))
    }

    pub(crate) fn branch_mut(&mut self, b: BranchId) -> &mut Branch {
        self.branches[b.index()]
            .as_mut()
            .unwrap_or_else(|| panic!("{b:?} has been deleted"))
    }

    pub(crate) fn slot(&self, key: Key, b: BranchId) -> Option<&Slot<E>> {
        self.slots.get(&key)?.iter().find(|slot| slot.branch == b)
    }

    pub(crate) fn slot_mut(&mut self, key: Key, b: BranchId) -> Option<&mut Slot<E>> {
        self.slots
            .get_mut(&key)?
            .iter_mut()
            .find(|slot| slot.branch == b)
    }

    pub(crate) fn slot_or_insert(&mut self, key: Key, b: BranchId) -> &mut Slot<E> {
        let slots = self.slots.entry(key).or_insert_with(|| {
            if !self.assertions.contains_key(&key) {
                self.key_count += 1;
            }
            KeySlots::default()
        });
        let index = match slots.iter().position(|slot| slot.branch == b) {
            Some(index) => index,
            None => {
                slots.push(Slot::new(b));
                slots.len() - 1
            }
        };
        &mut slots[index]
    }

    pub(crate) fn assertion_history_mut(
        &mut self,
        key: Key,
        b: BranchId,
    ) -> &mut History<Revision, E::AssertionData> {
        let histories = self.assertions.entry(key).or_insert_with(|| {
            if !self.slots.contains_key(&key) {
                self.key_count += 1;
            }
            KeyAssertions::<E>::default()
        });
        let index = match histories.iter().position(|(hb, _)| *hb == b) {
            Some(index) => index,
            None => {
                histories.push((b, History::new()));
                histories.len() - 1
            }
        };
        &mut histories[index].1
    }

    /// The one place a key's claim at a branch is written, so that the branch's `closed_index`
    /// stays exact.
    pub(crate) fn set_claim(&mut self, key: Key, b: BranchId, claim: Option<Claim<E>>) {
        let was_closed = self
            .slot(key, b)
            .and_then(|slot| slot.claim.as_ref())
            .is_some_and(|c| c.window.is_nonempty_closed());
        let now_closed = claim
            .as_ref()
            .is_some_and(|c| c.window.is_nonempty_closed());
        match claim {
            Some(claim) => self.slot_or_insert(key, b).claim = Some(claim),
            None => {
                if let Some(slot) = self.slot_mut(key, b) {
                    slot.claim = None;
                }
            }
        }
        let closed_index = &mut self.branch_mut(b).closed_index;
        if now_closed {
            closed_index.insert(key);
        } else if was_closed {
            closed_index.remove(&key);
        }
    }

    /// Closes `key`'s open claim at `b` at seq `at` (§4.6, "detaching").
    pub(crate) fn detach(&mut self, key: Key, b: BranchId, at: Seq) {
        let claim = self
            .slot_mut(key, b)
            .and_then(|slot| slot.claim.as_mut())
            .expect("detach of a key without a claim");
        claim.window.close_at(at);
        self.branch_mut(b).closed_index.insert(key);
    }

    /// Registers the edges of `cert` in `b`'s reverse-dependency map (§4.6, "attaching").
    pub(crate) fn register_edges(&mut self, b: BranchId, cert: &Cert<E::Premises>) {
        let branch = self.branch_mut(b);
        for premise in cert.premises() {
            branch.add_edge(premise.key, cert.key);
        }
    }
}

impl<E: Env> Default for CoreState<E> {
    fn default() -> Self {
        Self::new()
    }
}
