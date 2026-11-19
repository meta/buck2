/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Unit tests, one module per part of `incrementality.md`. Every mutating call on the test
//! state re-checks the invariants, so a test that passes has exercised the checker at each step.

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use allocative::Allocative;

use crate::Cert;
use crate::Change;
use crate::CoreState;
use crate::Env;
use crate::EpsilonToken;
use crate::Key;
use crate::Premise;
use crate::Revision;
use crate::Seq;
use crate::Version;
use crate::WriteOutcome;
use crate::arc::Arc;
use crate::ids::BranchId;

mod attributes;
mod cascade;
mod commit;
mod cycles;
mod delete;
mod epsilon;
mod fork;
mod resolution;
mod retention;
mod roots;
mod write;

pub(crate) struct TestEnv;

/// Identifies one assertion through everything the state does with it, the re-homing by
/// `delete` included. Minted in commit order, which along any chain is version order.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Allocative)]
pub(crate) struct Stamp(u64);

impl Stamp {
    pub(crate) fn mint() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Stamp(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Env for TestEnv {
    type Premises = Vec<Premise>;
    type AssertionData = Stamp;
    /// The latest assertion a derivation rests on, `None` if it rests on none.
    type Attribute = Option<Stamp>;
    const EMPTY: Option<Stamp> = None;

    fn assertion(_at: Version, stamp: Stamp) -> Option<Stamp> {
        Some(stamp)
    }

    fn combine(a: Option<Stamp>, b: Option<Stamp>) -> Option<Stamp> {
        a.max(b)
    }
}

pub(crate) type State = CoreState<TestEnv>;
pub(crate) type TestCert = Arc<Cert<Vec<Premise>>>;

pub(crate) fn k(i: u32) -> Key {
    Key { index: i }
}

pub(crate) fn r(i: u32) -> Revision {
    Revision::testing_new(i)
}

pub(crate) fn v(b: BranchId, seq: u32) -> Version {
    Version::new(b, Seq::testing_new(seq))
}

pub(crate) fn cert(
    key: Key,
    revision: Revision,
    deps: &[(Key, Revision)],
    epsilon: EpsilonToken,
) -> TestCert {
    Arc::new(Cert::new(
        key,
        revision,
        deps.iter().map(|(k, r)| Premise::new(*k, *r)).collect(),
        epsilon,
    ))
}

pub(crate) trait StateExt {
    fn assert_at(&mut self, b: BranchId, key: Key, revision: Revision) -> Version;
    fn dirty_at(&mut self, b: BranchId, key: Key) -> Version;
    fn commit_checked(&mut self, b: BranchId, changes: Vec<Change<Stamp>>) -> Version;
    fn write_checked(&mut self, cert: &TestCert) -> WriteOutcome;
    fn fork_checked(&mut self, from: Version) -> BranchId;
    fn delete_checked(&mut self, b: BranchId) -> Vec<Key>;
    fn valid_at(&self, key: Key, v: Version) -> Option<Revision>;
    fn candidate_at(&self, key: Key, v: Version) -> Option<Revision>;
    fn is_unknown_at(&self, key: Key, v: Version) -> bool;
    /// The attribute of `key`'s derivation at `v`, which must resolve `Valid`.
    fn attribute_at(&self, key: Key, v: Version) -> Option<Stamp>;
    /// The attribute of the dirty of `key` in force at `v`.
    fn untracked_attribute_at(&self, key: Key, v: Version) -> Option<Stamp>;
}

impl StateExt for State {
    fn assert_at(&mut self, b: BranchId, key: Key, revision: Revision) -> Version {
        self.commit_checked(
            b,
            vec![Change::Assert {
                key,
                revision,
                data: Stamp::mint(),
            }],
        )
    }

    fn dirty_at(&mut self, b: BranchId, key: Key) -> Version {
        self.commit_checked(
            b,
            vec![Change::Dirty {
                key,
                data: Stamp::mint(),
            }],
        )
    }

    fn commit_checked(&mut self, b: BranchId, changes: Vec<Change<Stamp>>) -> Version {
        let v = self.commit(b, changes);
        self.check_invariants();
        v
    }

    fn write_checked(&mut self, cert: &TestCert) -> WriteOutcome {
        let outcome = self.write(cert.clone());
        self.check_invariants();
        outcome
    }

    fn fork_checked(&mut self, from: Version) -> BranchId {
        let b = self.fork(from);
        self.check_invariants();
        b
    }

    fn delete_checked(&mut self, b: BranchId) -> Vec<Key> {
        let mut affected = self.delete_branch(b);
        self.check_invariants();
        affected.sort();
        affected
    }

    fn valid_at(&self, key: Key, v: Version) -> Option<Revision> {
        self.lookup(key, v).valid_revision()
    }

    fn candidate_at(&self, key: Key, v: Version) -> Option<Revision> {
        self.lookup(key, v).candidate().map(|c| c.revision)
    }

    fn is_unknown_at(&self, key: Key, v: Version) -> bool {
        !self.lookup(key, v).is_valid()
    }

    fn attribute_at(&self, key: Key, v: Version) -> Option<Stamp> {
        match self.lookup(key, v) {
            crate::Lookup::Valid { attribute, .. } => attribute,
            crate::Lookup::Unknown { .. } => panic!("{key:?} is unknown at {v}"),
        }
    }

    fn untracked_attribute_at(&self, key: Key, v: Version) -> Option<Stamp> {
        self.untracked(key, v).1
    }
}

/// A root state with injected key `k(0)` at `r(1)` (version 2) and computed key `k(1)` attached
/// at the head with a certificate over it.
pub(crate) fn leaf_and_dependent() -> (State, BranchId, TestCert) {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    let c = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&c);
    (s, root, c)
}
