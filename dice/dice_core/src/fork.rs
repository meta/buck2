/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! `fork` (`incrementality.md` §5.4), with the semi-precise partition of the candidates.

use crate::arc::Arc;
use crate::branch::Branch;
use crate::cert::Cert;
use crate::collections::KeyMap;
use crate::collections::KeySet;
use crate::env::Env;
use crate::ids::BranchId;
use crate::ids::Key;
use crate::ids::Version;
use crate::resolve::Resolved;
use crate::slot::Claim;
use crate::slot::Window;
use crate::state::CoreState;

impl<E: Env> CoreState<E> {
    /// `fork(from)` (§5.4): a new branch whose world is `from`'s.
    pub fn fork(&mut self, from: Version) -> BranchId {
        let b = from.branch();
        let s = from.seq();
        {
            let parent = self.branch(b);
            assert!(
                parent.first <= s && s <= parent.head,
                "{from} is not a version of {b:?}"
            );
        }
        let child = BranchId::from_index(self.branches.len());
        self.branches.push(Some(Branch::forked(from)));
        self.branch_mut(b).children.push((child, s));
        let first = self.branch(child).first;

        let mut candidates = KeySet::default();
        for dependents in self.branch(b).rdeps.values() {
            candidates.extend(dependents.iter().copied());
        }
        candidates.extend(self.branch(b).closed_index.iter().copied());

        let mut attached: Vec<Arc<Cert<E::Premises>>> = Vec::new();
        let mut undecided: KeyMap<Claim<E>> = KeyMap::default();
        for key in candidates {
            match self.resolve(key, from) {
                Resolved::Claim { owner, claim } if owner == b && !claim.window.is_open() => {
                    undecided.insert(key, claim.clone());
                }
                Resolved::Claim { claim, .. } => attached.push(claim.cert.clone()),
                Resolved::Asserted { .. } | Resolved::Unknown { .. } => {}
            }
        }

        let mut pending: KeyMap<usize> = KeyMap::default();
        let mut waiters: KeyMap<Vec<Key>> = KeyMap::default();
        let mut ready: Vec<Key> = Vec::new();
        for (key, claim) in &undecided {
            let mut supported = self.epsilon(*key, from) == claim.cert.epsilon;
            let mut waits = 0;
            if supported {
                for premise in claim.cert.premises() {
                    let resolved =
                        self.resolve(premise.key, from).revision() == Some(premise.revision);
                    if !resolved {
                        supported = false;
                        break;
                    }
                    if undecided.contains_key(&premise.key) {
                        waits += 1;
                        waiters.entry(premise.key).or_default().push(*key);
                    }
                }
            }
            if !supported {
                continue;
            }
            if waits == 0 {
                ready.push(*key);
            } else {
                pending.insert(*key, waits);
            }
        }
        let mut supported = KeySet::default();
        while let Some(key) = ready.pop() {
            supported.insert(key);
            for waiter in waiters.get(&key).map(|w| w.as_slice()).unwrap_or(&[]) {
                if let Some(waits) = pending.get_mut(waiter) {
                    *waits -= 1;
                    if *waits == 0 {
                        ready.push(*waiter);
                    }
                }
            }
        }

        for (key, claim) in undecided {
            if supported.contains(&key) {
                attached.push(claim.cert);
            } else {
                self.set_claim(
                    key,
                    child,
                    Some(Claim {
                        cert: claim.cert,
                        window: Window::empty(first),
                        attribute: claim.attribute,
                    }),
                );
            }
        }
        for cert in attached {
            self.register_edges(child, &cert);
        }
        child
    }
}
