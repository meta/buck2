/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::HashSet;
use std::fmt::Debug;

use crate::cert::Cert;
use crate::collections::KeySet;
use crate::env::Env;
use crate::history::Entry;
use crate::ids::BranchId;
use crate::ids::Key;
use crate::ids::Version;
use crate::resolve::Attached;
use crate::resolve::Resolved;
use crate::state::CoreState;

impl<E: Env> CoreState<E>
where
    E::Attribute: PartialEq + Debug,
{
    /// Checks, by full scan, everything the state promises about itself: the branch tree, the
    /// histories, Invariants 1 to 3 of `incrementality.md` §4.6 (3 as an equality of edge
    /// sets), that every attached claim carries the attribute of its derivation (§4.7), the
    /// exactness of every `closed_index` and of the key count, that no key is both asserted and
    /// certified, and that nothing refers to a deleted branch. Panics with the first
    /// violation found. Meant for tests and fuzzers after every operation; the master invariant
    /// itself is not a property of the state alone and is checked against the recorded history by
    /// the test oracle instead.
    pub fn check_invariants(&self) {
        self.check_branches();
        self.check_slots();
        self.check_assertions();
        assert_eq!(
            self.key_count(),
            self.keys().count(),
            "key count is inexact"
        );
        for b in self.branches() {
            self.check_attachment(b);
        }
    }

    fn check_branches(&self) {
        for b in self.branches() {
            let branch = self.branch(b);
            assert!(branch.first <= branch.head, "{b:?} has head before first");
            if let Some(parent) = branch.parent {
                let pb = self.branch(parent.branch());
                assert!(
                    pb.first <= parent.seq() && parent.seq() <= pb.head,
                    "{b:?} was forked from {parent}, which is not a version of its parent"
                );
                assert!(
                    pb.children.iter().any(|c| *c == (b, parent.seq())),
                    "{b:?} is missing from its parent's children"
                );
            }
            for (child, seq) in branch.children.iter() {
                assert!(self.is_live(*child), "{b:?} lists deleted {child:?}");
                assert_eq!(
                    self.branch(*child).parent,
                    Some(Version::new(b, *seq)),
                    "{child:?} does not agree with {b:?} about its fork point"
                );
            }
        }
    }

    fn check_history<R, T>(&self, what: &str, b: BranchId, entries: &[Entry<R, T>]) {
        let branch = self.branch(b);
        for pair in entries.windows(2) {
            assert!(pair[0].seq < pair[1].seq, "{what} at {b:?} is out of order");
        }
        if let Some(first) = entries.first() {
            assert!(
                first.seq >= branch.first,
                "{what} at {b:?} predates the branch"
            );
        }
        if let Some(last) = entries.last() {
            assert!(
                last.seq <= branch.head,
                "{what} at {b:?} is ahead of the head"
            );
        }
    }

    fn check_slots(&self) {
        let mut closed: Vec<KeySet> = (0..self.branches.len())
            .map(|_| KeySet::default())
            .collect();
        for (key, slots) in &self.slots {
            if self.assertions.contains_key(key) {
                assert!(
                    slots.iter().all(|slot| slot.claim.is_none()),
                    "{key:?} is both asserted and certified"
                );
            }
            let mut branches = HashSet::new();
            for slot in slots {
                assert!(
                    branches.insert(slot.branch),
                    "{key:?} has two slots at {:?}",
                    slot.branch
                );
                assert!(
                    self.is_live(slot.branch),
                    "{key:?} has a slot on deleted {:?}",
                    slot.branch
                );
                let branch = self.branch(slot.branch);
                self.check_history(
                    &format!("untracked-input history of {key:?}"),
                    slot.branch,
                    slot.untracked.entries(),
                );
                let Some(claim) = &slot.claim else {
                    continue;
                };
                assert_eq!(
                    claim.cert.key, *key,
                    "claim of {key:?} holds another key's cert"
                );
                assert!(
                    claim.window.from() >= branch.first,
                    "window of {key:?} at {:?} predates the branch",
                    slot.branch
                );
                if claim.window.is_nonempty_closed() {
                    assert!(
                        claim
                            .window
                            .until()
                            .expect("a nonempty closed window has an end")
                            <= branch.head,
                        "Invariant 1: closed window of {key:?} at {:?} reaches the head",
                        slot.branch
                    );
                    closed[slot.branch.index()].insert(*key);
                }
            }
        }
        for b in self.branches() {
            assert!(
                self.branch(b).closed_index == closed[b.index()],
                "closed_index of {b:?} is inexact"
            );
        }
    }

    fn check_assertions(&self) {
        for (key, histories) in &self.assertions {
            let mut branches = HashSet::new();
            for (b, history) in histories {
                assert!(branches.insert(*b), "{key:?} has two histories at {b:?}");
                assert!(self.is_live(*b), "{key:?} has a history on deleted {b:?}");
                self.check_history(&format!("history of {key:?}"), *b, history.entries());
            }
        }
    }

    /// Invariant 2 and Invariant 3 at `b`.
    fn check_attachment(&self, b: BranchId) {
        let head = self.head(b);
        let mut expected: HashSet<(Key, Key)> = HashSet::new();
        for key in self.slots.keys().copied() {
            let Some(attached) = self.attached(key, b) else {
                continue;
            };
            let claim = attached.claim();
            let own_from = match &attached {
                Attached::Own(claim) => Some(claim.window.from()),
                Attached::Inherited { .. } => None,
            };
            assert_eq!(
                self.epsilon(key, head),
                claim.cert.epsilon,
                "Invariant 2: {key:?} is attached at {b:?} under another ε"
            );
            if let (Some(from), Some((owner, entry))) =
                (own_from, self.resolve_untracked(key, head))
                && owner == b
            {
                assert!(
                    entry.seq <= from,
                    "Invariant 2: window of {key:?} at {b:?} spans a dirty"
                );
            }
            for premise in claim.cert.premises() {
                let resolved = self.resolve(premise.key, head);
                assert_eq!(
                    resolved.revision(),
                    Some(premise.revision),
                    "Invariant 2: premise {premise:?} of {key:?} does not hold at the head of {b:?}"
                );
                if let Some(from) = own_from {
                    let premise_from = match resolved {
                        Resolved::Claim { owner, claim } if owner == b => Some(claim.window.from()),
                        Resolved::Asserted { owner, entry } if owner == b => Some(entry.seq),
                        _ => None,
                    };
                    if let Some(premise_from) = premise_from {
                        assert!(
                            premise_from <= from,
                            "Invariant 2: window of {key:?} at {b:?} starts before its premise {premise:?}"
                        );
                    }
                }
                expected.insert((premise.key, key));
            }
            assert_eq!(
                Some(claim.attribute),
                self.derivation_attribute(&claim.cert, head),
                "attribute of {key:?} at {b:?} is not that of its derivation at the head"
            );
        }
        let actual: HashSet<(Key, Key)> = self
            .branch(b)
            .rdeps
            .iter()
            .flat_map(|(dep, dependents)| dependents.iter().map(move |k| (*dep, *k)))
            .collect();
        assert!(
            actual == expected,
            "Invariant 3 at {b:?}: edges {:?} present but not owed, {:?} owed but absent",
            actual.difference(&expected).collect::<Vec<_>>(),
            expected.difference(&actual).collect::<Vec<_>>(),
        );
    }

    /// The attribute of the derivation of `cert` at `v` as resolution finds it, `None` if some
    /// premise does not resolve to its recorded revision there.
    fn derivation_attribute(&self, cert: &Cert<E::Premises>, v: Version) -> Option<E::Attribute> {
        let (epsilon, mut attribute) = self.untracked(cert.key, v);
        if epsilon != cert.epsilon {
            return None;
        }
        for premise in cert.premises() {
            let of_premise = match self.resolve(premise.key, v) {
                Resolved::Claim { claim, .. } if claim.revision() == premise.revision => {
                    claim.attribute
                }
                Resolved::Asserted { owner, entry } if entry.revision == premise.revision => {
                    E::assertion(Version::new(owner, entry.seq), entry.data)
                }
                _ => return None,
            };
            attribute = E::combine(attribute, of_premise);
        }
        Some(attribute)
    }
}
