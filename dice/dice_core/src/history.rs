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
use mini_vec::MiniVec;

use crate::ids::Seq;

/// One assertion: at `seq` of the owning branch, the asserted thing took `revision`.
#[derive(Copy, Clone, Debug, Allocative)]
pub struct Entry<R, T> {
    pub seq: Seq,
    pub revision: R,
    pub data: T,
}

/// The assertion history of one asserted thing on one branch (`incrementality.md` §4.3): the
/// revisions the branch's own commits asserted for it, in seq order. Injected keys and untracked
/// inputs both keep one per branch; the revision type is the only difference.
///
/// The history says nothing about seqs before its first entry. Resolution there falls through to
/// the branch's parent at the fork point, and a history is never copied from one branch to
/// another: its seqs are the owning branch's.
#[derive(Allocative)]
pub struct History<R, T> {
    entries: MiniVec<Entry<R, T>>,
}

impl<R, T> History<R, T> {
    pub const fn new() -> Self {
        History {
            entries: MiniVec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[Entry<R, T>] {
        self.entries.as_slice()
    }

    /// The entry in force at `seq`: the last one at or before it.
    pub fn at(&self, seq: Seq) -> Option<&Entry<R, T>> {
        self.entries().iter().rev().find(|entry| entry.seq <= seq)
    }

    pub fn first(&self) -> Option<&Entry<R, T>> {
        self.entries().first()
    }

    pub fn last(&self) -> Option<&Entry<R, T>> {
        self.entries().last()
    }

    /// Prepends an assertion, which must be earlier than every existing one.
    pub(crate) fn insert_first(&mut self, entry: Entry<R, T>) {
        if let Some(first) = self.first() {
            assert!(
                entry.seq < first.seq,
                "a prepended assertion precedes the history ({:?} before {:?})",
                entry.seq,
                first.seq
            );
        }
        self.entries.insert(0, entry);
    }

    /// Appends an assertion, which must be later than every existing one.
    pub(crate) fn push(&mut self, entry: Entry<R, T>) {
        if let Some(last) = self.last() {
            assert!(
                last.seq < entry.seq,
                "assertions are appended in seq order ({:?} after {:?})",
                entry.seq,
                last.seq
            );
        }
        self.entries.push(entry);
    }
}

impl<R, T> Default for History<R, T> {
    fn default() -> Self {
        Self::new()
    }
}
