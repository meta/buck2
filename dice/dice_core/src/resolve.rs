/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Resolution (`incrementality.md` §4.4) and certificate coverage (§4.5).

use smallvec::SmallVec;
use smallvec::smallvec;

use crate::arc::Arc;
use crate::cert::Cert;
use crate::env::Env;
use crate::env::Premise;
use crate::history::Entry;
use crate::ids::BranchId;
use crate::ids::EpsilonToken;
use crate::ids::Key;
use crate::ids::Revision;
use crate::ids::Seq;
use crate::ids::Version;
use crate::slot::Claim;
use crate::slot::Window;
use crate::state::CoreState;

/// What a key resolves to at a version.
pub(crate) enum Resolved<'a, E: Env> {
    /// `Valid` through a claim; `owner` is the branch holding it, which is the queried branch
    /// or an ancestor the query delegated to.
    Claim {
        owner: BranchId,
        claim: &'a Claim<E>,
    },
    /// `Valid` through an assertion history entry, for an asserted key.
    Asserted {
        owner: BranchId,
        entry: &'a Entry<Revision, E::AssertionData>,
    },
    /// `Unknown`, with the certificate a caller may try to re-establish: the nearest claim on the
    /// resolution chain whatever its window, else any claim the key has on another branch.
    Unknown {
        candidate: Option<&'a Arc<Cert<E::Premises>>>,
    },
}

impl<'a, E: Env> Resolved<'a, E> {
    pub(crate) fn revision(&self) -> Option<Revision> {
        match self {
            Resolved::Claim { claim, .. } => Some(claim.revision()),
            Resolved::Asserted { entry, .. } => Some(entry.revision),
            Resolved::Unknown { .. } => None,
        }
    }
}

/// How a key is attached at a branch's head (§4.6).
pub(crate) enum Attached<'a, E: Env> {
    /// Through the branch's own open claim.
    Own(&'a Claim<E>),
    /// Through inheritance from an ancestor's claim, which the branch's own commits have not
    /// diverged from.
    Inherited { claim: &'a Claim<E> },
}

impl<'a, E: Env> Attached<'a, E> {
    pub(crate) fn claim(&self) -> &'a Claim<E> {
        match self {
            Attached::Own(claim) | Attached::Inherited { claim, .. } => claim,
        }
    }
}

/// A half-open range of seqs `[from, until)` of one branch, `until` absent meaning unbounded.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Interval {
    pub(crate) from: Seq,
    pub(crate) until: Option<Seq>,
}

impl Interval {
    /// Whether the interval holds no seq, `until` being at or before `from`.
    fn is_empty(self) -> bool {
        self.until.is_some_and(|until| until <= self.from)
    }

    fn contains(self, seq: Seq) -> bool {
        self.from <= seq && self.until.is_none_or(|until| seq < until)
    }
}

/// A piece of a cover (§4.5): seqs of one branch at which some premises resolve to their recorded
/// revisions, with the attribute (§4.7) of their derivations there, which is the same at every
/// seq of the piece.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Piece<A> {
    pub(crate) interval: Interval,
    pub(crate) attribute: A,
}

/// Disjoint, nonempty pieces in increasing order. Adjacent pieces are kept apart, as their
/// attributes may differ.
pub(crate) type Pieces<A> = SmallVec<[Piece<A>; 2]>;

fn push<A>(out: &mut Pieces<A>, piece: Piece<A>) {
    if !piece.interval.is_empty() {
        out.push(piece);
    }
}

fn intersect<E: Env>(a: &[Piece<E::Attribute>], b: &[Piece<E::Attribute>]) -> Pieces<E::Attribute> {
    let mut out = Pieces::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let (x, y) = (a[i].interval, b[j].interval);
        let from = x.from.max(y.from);
        let until = match (x.until, y.until) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (Some(x), None) | (None, Some(x)) => Some(x),
            (None, None) => None,
        };
        push(
            &mut out,
            Piece {
                interval: Interval { from, until },
                attribute: E::combine(a[i].attribute, b[j].attribute),
            },
        );
        // Advance whichever interval ends first; an unbounded one never does.
        match (x.until, y.until) {
            (Some(x), Some(y)) if x <= y => i += 1,
            (Some(_), Some(_)) => j += 1,
            (Some(_), None) => i += 1,
            (None, Some(_)) => j += 1,
            (None, None) => break,
        }
    }
    out
}

impl<E: Env> CoreState<E> {
    /// Resolves `key` at `v` (§4.4): the branch's own claim or assertion history if it has one,
    /// else the parent's at the fork point, and so on up to a root.
    pub(crate) fn resolve(&self, key: Key, v: Version) -> Resolved<'_, E> {
        if let Some(histories) = self.assertions.get(&key) {
            let mut cur = v;
            loop {
                if let Some(history) = histories
                    .iter()
                    .find_map(|(b, h)| (*b == cur.branch()).then_some(h))
                    && let Some(entry) = history.at(cur.seq())
                {
                    return Resolved::Asserted {
                        owner: cur.branch(),
                        entry,
                    };
                }
                match self.branch(cur.branch()).parent {
                    Some(parent) => cur = parent,
                    None => return Resolved::Unknown { candidate: None },
                }
            }
        }
        let slots = self.slots.get(&key).map(|s| s.as_slice()).unwrap_or(&[]);
        let mut cur = v;
        loop {
            if let Some(slot) = slots.iter().find(|s| s.branch == cur.branch())
                && let Some(claim) = &slot.claim
            {
                if claim.window.covers(cur.seq()) {
                    return Resolved::Claim {
                        owner: cur.branch(),
                        claim,
                    };
                }
                return Resolved::Unknown {
                    candidate: Some(&claim.cert),
                };
            }
            match self.branch(cur.branch()).parent {
                Some(parent) => cur = parent,
                None => {
                    return Resolved::Unknown {
                        candidate: slots.iter().find_map(|s| s.claim.as_ref().map(|c| &c.cert)),
                    };
                }
            }
        }
    }

    /// The assertion of `key`'s untracked input in force at `v`, or `None` for the root default
    /// [`EpsilonToken::INITIAL`].
    pub(crate) fn resolve_untracked(
        &self,
        key: Key,
        v: Version,
    ) -> Option<(BranchId, &Entry<EpsilonToken, E::AssertionData>)> {
        let slots = self.slots.get(&key)?;
        let mut cur = v;
        loop {
            if let Some(slot) = slots.iter().find(|s| s.branch == cur.branch())
                && let Some(entry) = slot.untracked.at(cur.seq())
            {
                return Some((cur.branch(), entry));
            }
            cur = self.branch(cur.branch()).parent?;
        }
    }

    /// The revision of `key`'s untracked input at `v`.
    pub fn epsilon(&self, key: Key, v: Version) -> EpsilonToken {
        self.resolve_untracked(key, v)
            .map_or(EpsilonToken::INITIAL, |(_, entry)| entry.revision)
    }

    /// Whether, and how, `key` is attached at branch `b` (§4.6): resolves `Valid` at the head
    /// through an own open claim or through inheritance. During a commit, a claim the commit has
    /// already closed at the new seq still covers the old head; it is not attached.
    pub(crate) fn attached(&self, key: Key, b: BranchId) -> Option<Attached<'_, E>> {
        match self.resolve(key, self.head(b)) {
            Resolved::Claim { owner, claim } if owner == b => {
                claim.window.is_open().then_some(Attached::Own(claim))
            }
            Resolved::Claim { claim, .. } => Some(Attached::Inherited { claim }),
            Resolved::Asserted { .. } | Resolved::Unknown { .. } => None,
        }
    }

    /// The attribute of the assertion `entry` of a history owned by `b`.
    fn entry_attribute<R>(b: BranchId, entry: &Entry<R, E::AssertionData>) -> E::Attribute {
        E::assertion(Version::new(b, entry.seq), entry.data)
    }

    /// The seqs of `b` at which an assertion history resolves to `revision`, given the branch's
    /// own entries and, if the parent resolves to `revision` at the fork point, the attribute of
    /// the assertion it resolves through (§4.5).
    fn history_pieces<R: Copy + Eq>(
        &self,
        b: BranchId,
        own: &[Entry<R, E::AssertionData>],
        parent: Option<E::Attribute>,
        revision: R,
    ) -> Pieces<E::Attribute> {
        let mut out = Pieces::new();
        if let Some(attribute) = parent {
            push(
                &mut out,
                Piece {
                    interval: Interval {
                        from: self.branch(b).first,
                        until: own.first().map(|e| e.seq),
                    },
                    attribute,
                },
            );
        }
        for (i, entry) in own.iter().enumerate() {
            if entry.revision == revision {
                push(
                    &mut out,
                    Piece {
                        interval: Interval {
                            from: entry.seq,
                            until: own.get(i + 1).map(|e| e.seq),
                        },
                        attribute: Self::entry_attribute(b, entry),
                    },
                );
            }
        }
        out
    }

    /// The seqs of `b` at which `premise` resolves to its recorded revision.
    fn premise_pieces(&self, b: BranchId, premise: Premise) -> Pieces<E::Attribute> {
        let revision = premise.revision;
        let parent = self.branch(b).parent;
        if let Some(histories) = self.assertions.get(&premise.key) {
            let own = histories
                .iter()
                .find_map(|(hb, h)| (*hb == b).then_some(h.entries()))
                .unwrap_or(&[]);
            let parent = parent.and_then(|p| match self.resolve(premise.key, p) {
                Resolved::Asserted { owner, entry } if entry.revision == revision => {
                    Some(Self::entry_attribute(owner, entry))
                }
                _ => None,
            });
            return self.history_pieces(b, own, parent, revision);
        }
        match self.slot(premise.key, b).and_then(|s| s.claim.as_ref()) {
            Some(claim) => {
                if claim.revision() == revision && !claim.window.is_empty() {
                    smallvec![Piece {
                        interval: Interval {
                            from: claim.window.from(),
                            until: claim.window.until(),
                        },
                        attribute: claim.attribute,
                    }]
                } else {
                    Pieces::new()
                }
            }
            None => {
                let inherited = parent.and_then(|p| match self.resolve(premise.key, p) {
                    Resolved::Claim { claim, .. } if claim.revision() == revision => {
                        Some(claim.attribute)
                    }
                    _ => None,
                });
                match inherited {
                    Some(attribute) => smallvec![Piece {
                        interval: Interval {
                            from: self.branch(b).first,
                            until: None,
                        },
                        attribute,
                    }],
                    None => Pieces::new(),
                }
            }
        }
    }

    /// The seqs of `b` at which `key`'s untracked input has revision `epsilon`.
    fn untracked_pieces(
        &self,
        b: BranchId,
        key: Key,
        epsilon: EpsilonToken,
    ) -> Pieces<E::Attribute> {
        let own = self
            .slot(key, b)
            .map(|s| s.untracked.entries())
            .unwrap_or(&[]);
        let parent = match self.branch(b).parent {
            Some(p) => match self.resolve_untracked(key, p) {
                Some((owner, entry)) => {
                    (entry.revision == epsilon).then(|| Self::entry_attribute(owner, entry))
                }
                None => (epsilon == EpsilonToken::INITIAL).then_some(E::EMPTY),
            },
            None => (epsilon == EpsilonToken::INITIAL).then_some(E::EMPTY),
        };
        self.history_pieces(b, own, parent, epsilon)
    }

    /// `cover_b(cert)` (§4.5): the seqs of `b` at which every premise of `cert`, the untracked
    /// input included, resolves to its recorded revision, in pieces carrying the attribute of
    /// the certificate's derivation there (§4.7).
    pub(crate) fn cover(&self, b: BranchId, cert: &Cert<E::Premises>) -> Pieces<E::Attribute> {
        let mut cover: Pieces<E::Attribute> = smallvec![Piece {
            interval: Interval {
                from: self.branch(b).first,
                until: None,
            },
            attribute: E::EMPTY,
        }];
        cover = intersect::<E>(&cover, &self.untracked_pieces(b, cert.key, cert.epsilon));
        for premise in cert.premises() {
            if cover.is_empty() {
                break;
            }
            cover = intersect::<E>(&cover, &self.premise_pieces(b, premise));
        }
        cover
    }

    /// The window a write installs from a nonempty cover (§4.5, §5.2), with its attribute: the
    /// piece containing the head, as an open window, if there is one, otherwise the most recent
    /// piece.
    pub(crate) fn pick_window(
        &self,
        b: BranchId,
        cover: &[Piece<E::Attribute>],
    ) -> Option<(Window, E::Attribute)> {
        let last = cover.last()?;
        let head = self.branch(b).head;
        let window = if last.interval.contains(head) {
            debug_assert!(
                last.interval.until.is_none(),
                "an interval reaching the head is unbounded: nothing closes at or after it"
            );
            Window::open(last.interval.from)
        } else {
            let until = last
                .interval
                .until
                .expect("an unbounded interval contains the head");
            debug_assert!(until <= head);
            Window::closed(last.interval.from, until)
        };
        Some((window, last.attribute))
    }
}
