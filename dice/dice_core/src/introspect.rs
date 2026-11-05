/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::env::Env;
use crate::history::History;
use crate::ids::BranchId;
use crate::ids::Key;
use crate::ids::Revision;
use crate::slot::Slot;
use crate::state::CoreState;

/// Everything the state holds for one key, for dumps and debugging.
pub struct KeyIntrospection<'a, E: Env> {
    pub slots: &'a [Slot<E>],
    pub assertions: &'a [(BranchId, History<Revision, E::AssertionData>)],
}

impl<E: Env> CoreState<E> {
    /// The keys the state holds anything for.
    pub fn keys(&self) -> impl Iterator<Item = Key> + '_ {
        self.slots.keys().copied().chain(
            self.assertions
                .keys()
                .copied()
                .filter(|key| !self.slots.contains_key(key)),
        )
    }

    pub fn key_count(&self) -> usize {
        self.key_count
    }

    pub fn introspect_key(&self, key: Key) -> KeyIntrospection<'_, E> {
        KeyIntrospection {
            slots: self.slots.get(&key).map(|s| s.as_slice()).unwrap_or(&[]),
            assertions: self
                .assertions
                .get(&key)
                .map(|a| a.as_slice())
                .unwrap_or(&[]),
        }
    }

    /// The keys whose attached claims at `b` depend on `key`.
    pub fn rdeps(&self, b: BranchId, key: Key) -> &[Key] {
        self.branch(b)
            .rdeps
            .get(&key)
            .map(|d| d.as_slice())
            .unwrap_or(&[])
    }
}
