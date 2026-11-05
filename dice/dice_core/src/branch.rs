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

use crate::collections::KeyMap;
use crate::collections::KeySet;
use crate::ids::BranchId;
use crate::ids::Key;
use crate::ids::Seq;
use crate::ids::Version;

/// One branch of the history (`incrementality.md` §4.2).
#[derive(Allocative)]
pub(crate) struct Branch {
    /// The version this branch was forked from; `None` for a root.
    pub(crate) parent: Option<Version>,
    /// The branches forked from this one, with the seq each was forked at.
    pub(crate) children: MiniVec<(BranchId, Seq)>,
    /// The seq of this branch's initial version.
    pub(crate) first: Seq,
    /// The seq of this branch's newest version.
    pub(crate) head: Seq,
    /// `RdepMap` (§4.2), maintained exactly (Invariant 3 as an equality). A dependent appears
    /// under a dependency more than once only if its certificate lists that dependency more than
    /// once.
    pub(crate) rdeps: KeyMap<MiniVec<Key>>,
    /// `closed_index` (§4.2).
    pub(crate) closed_index: KeySet,
}

impl Branch {
    pub(crate) fn root(first: Seq) -> Self {
        Branch {
            parent: None,
            children: MiniVec::new(),
            first,
            head: first,
            rdeps: KeyMap::default(),
            closed_index: KeySet::default(),
        }
    }

    pub(crate) fn forked(parent: Version) -> Self {
        Branch {
            parent: Some(parent),
            ..Branch::root(Seq::FIRST)
        }
    }

    /// Forgets `child`, which must be one of this branch's children. Children are unordered.
    pub(crate) fn remove_child(&mut self, child: BranchId) {
        let index = self
            .children
            .iter()
            .position(|(c, _)| *c == child)
            .unwrap_or_else(|| panic!("{child:?} is not a child of this branch"));
        let last = self.children.len() - 1;
        self.children.as_mut_slice().swap(index, last);
        self.children.pop();
    }

    pub(crate) fn add_edge(&mut self, dep: Key, dependent: Key) {
        self.rdeps.entry(dep).or_default().push(dependent);
    }

    pub(crate) fn take_rdeps(&mut self, dep: Key) -> MiniVec<Key> {
        self.rdeps.remove(&dep).unwrap_or_default()
    }

    /// Removes every occurrence of every key in `dependents` from `dep`'s edges.
    pub(crate) fn remove_edges(&mut self, dep: Key, dependents: &KeySet) {
        let Some(set) = self.rdeps.get_mut(&dep) else {
            return;
        };
        let slice = set.as_mut_slice();
        let mut write = 0;
        for read in 0..slice.len() {
            if !dependents.contains(&slice[read]) {
                slice[write] = slice[read];
                write += 1;
            }
        }
        set.truncate(write);
        if set.is_empty() {
            self.rdeps.remove(&dep);
        }
    }
}
