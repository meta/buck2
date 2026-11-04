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
use dupe::Dupe;

use crate::arc::Arc;
use crate::cert::Cert;
use crate::env::Env;
use crate::history::History;
use crate::ids::BranchId;
use crate::ids::EpsilonToken;
use crate::ids::Revision;
use crate::ids::Seq;

/// The seqs of one branch over which a claim holds, `[from, until)`, with `until` absent meaning
/// unbounded (`incrementality.md` §4.1). Nonempty closed windows obey Invariant 1 (§4.6).
#[derive(Copy, Clone, Dupe, Debug, PartialEq, Eq, Hash, Allocative)]
pub struct Window {
    from: Seq,
    until: Option<Seq>,
}

impl Window {
    pub(crate) fn open(from: Seq) -> Self {
        Window { from, until: None }
    }

    pub(crate) fn closed(from: Seq, until: Seq) -> Self {
        assert!(from <= until, "window [{from:?}, {until:?}) is inverted");
        Window {
            from,
            until: Some(until),
        }
    }

    pub(crate) fn empty(at: Seq) -> Self {
        Window::closed(at, at)
    }

    pub fn from(&self) -> Seq {
        self.from
    }

    pub fn until(&self) -> Option<Seq> {
        self.until
    }

    pub fn is_open(&self) -> bool {
        self.until.is_none()
    }

    pub fn is_empty(&self) -> bool {
        self.until == Some(self.from)
    }

    pub fn is_nonempty_closed(&self) -> bool {
        self.until.is_some_and(|until| until > self.from)
    }

    pub fn covers(&self, seq: Seq) -> bool {
        self.from <= seq && self.until.is_none_or(|until| seq < until)
    }

    /// Closes an open window at `until`, which must lie within it.
    pub(crate) fn close_at(&mut self, until: Seq) {
        debug_assert!(self.is_open() && self.from < until);
        self.until = Some(until);
    }

    /// Orders windows by how far they reach, open windows above every closed one; the install
    /// policy of §5.2 prefers the window that reaches further.
    pub(crate) fn reach(self) -> u64 {
        self.until.map_or(u64::MAX, |until| until.get() as u64)
    }
}

/// A key's claim on one branch (§4.1).
#[derive(Allocative)]
#[allocative(bound = "E: Env")]
pub struct Claim<E: Env> {
    pub cert: Arc<Cert<E::Premises>>,
    pub window: Window,
    /// The attribute of the derivation the claim stands for (§4.7). Meaningless for an empty
    /// window.
    pub attribute: E::Attribute,
}

impl<E: Env> Claim<E> {
    pub fn revision(&self) -> Revision {
        self.cert.revision
    }
}

impl<E: Env> Clone for Claim<E> {
    fn clone(&self) -> Self {
        Claim {
            cert: self.cert.clone(),
            window: self.window,
            attribute: self.attribute,
        }
    }
}

/// A key's state on one branch (§4.1): its claim, if any, and the assertion history of its
/// untracked input. A slot with neither is equivalent to no slot at all.
#[derive(Allocative)]
#[allocative(bound = "E: Env")]
pub struct Slot<E: Env> {
    pub branch: BranchId,
    pub claim: Option<Claim<E>>,
    pub untracked: History<EpsilonToken, E::AssertionData>,
}

impl<E: Env> Slot<E> {
    pub(crate) fn new(branch: BranchId) -> Self {
        Slot {
            branch,
            claim: None,
            untracked: History::new(),
        }
    }
}
