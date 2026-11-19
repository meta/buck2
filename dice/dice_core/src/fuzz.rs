/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Randomized tests of the whole interface against the oracle of `oracle.rs`, with the
//! invariants checked after every operation.
//!
//! Two generators. The first writes arbitrary certificates: random keys, revisions, premises and
//! ε. Soundness (§3.3) is unconditional, so every `Valid` the state returns must still be
//! justified by the recorded history, and this mode reaches states an honest environment never
//! produces. The second is an honest environment: injected leaves, computed keys with fixed deps
//! and a deterministic value function, driven the way dice's worker drives the core (lookup,
//! re-establish the candidate or compute bottom-up, write). Besides soundness it checks
//! from-scratch consistency (§3.4), that a write at a head attaches, and that a commit leaves
//! keys outside the changed keys' support valid.
//!
//! Failing cases are shrunk by deleting operations and printed with the seed. Environment
//! variables `DICE_CORE_FUZZ_CASES` and `DICE_CORE_FUZZ_SEED` select the run.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Debug;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;

use crate::Cert;
use crate::Change;
use crate::EpsilonToken;
use crate::Key;
use crate::Premise;
use crate::Revision;
use crate::Seq;
use crate::Version;
use crate::arc::Arc;
use crate::ids::BranchId;
use crate::oracle::Atom;
use crate::oracle::Model;
use crate::oracle::eps_of;
use crate::oracle::rev_of;
use crate::tests::Stamp;
use crate::tests::State;
use crate::tests::TestCert;

const KEYS: usize = 12;
const INJECTED: usize = 4;
const MAX_BRANCHES: usize = 4;

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

fn cases() -> u64 {
    std::env::var("DICE_CORE_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        // Interpreted, a case takes tens of seconds.
        .unwrap_or(if cfg!(miri) { 2 } else { 40 })
}

fn seed() -> u64 {
    std::env::var("DICE_CORE_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x5eed)
}

fn injected(i: usize) -> Key {
    Key {
        index: (i % INJECTED) as u32,
    }
}

fn computed(i: usize) -> Key {
    Key {
        index: (INJECTED + i % (KEYS - INJECTED)) as u32,
    }
}

fn any_key(i: usize) -> Key {
    Key {
        index: (i % KEYS) as u32,
    }
}

/// Picks an existing version from generator indices, so that a shrunk case stays meaningful.
fn version_of(state: &State, branch: usize, seq: usize) -> Version {
    let branches: Vec<BranchId> = state.branches().collect();
    let b = branches[branch % branches.len()];
    let first = state.first(b).seq().get();
    let head = state.head(b).seq().get();
    let seq = first + (seq as u32) % (head - first + 1);
    Version::new(b, Seq::testing_new(seq))
}

/// Shared bookkeeping of both modes: the state, the oracle's history, and the ε tokens minted.
struct Run {
    state: State,
    model: Model,
    eps_seen: HashMap<Key, Vec<EpsilonToken>>,
}

impl Run {
    /// A run with one root.
    fn new() -> (Self, BranchId) {
        let mut run = Run {
            state: State::new(),
            model: Model::new(),
            eps_seen: HashMap::new(),
        };
        let root = run.new_root();
        (run, root)
    }

    /// One of the tokens `key` has had, the initial one included.
    fn eps_for(&self, key: Key, i: usize) -> EpsilonToken {
        match self.eps_seen.get(&key) {
            Some(seen) if !seen.is_empty() && !i.is_multiple_of(4) => seen[i % seen.len()],
            _ => EpsilonToken::INITIAL,
        }
    }

    /// Commits, records the new version if one was minted, and returns it.
    fn commit(&mut self, b: BranchId, changes: Vec<Change<Stamp>>) -> Option<Version> {
        let before = self.state.head(b);
        let mut asserted = Vec::new();
        let mut dirtied = Vec::new();
        for change in &changes {
            match change {
                Change::Assert { key, revision, .. } => asserted.push((*key, *revision)),
                Change::Dirty { key, .. } => dirtied.push(*key),
            }
        }
        let after = self.state.commit(b, changes);
        self.state.check_invariants();
        if after == before {
            assert!(
                dirtied.is_empty(),
                "a commit with a dirty must mint a version"
            );
            return None;
        }
        let mut assertions: Vec<(Atom, u64)> = asserted
            .iter()
            .map(|(k, r)| (Atom::Key(*k), rev_of(*r)))
            .collect();
        for key in dirtied {
            let eps = self.state.epsilon(key, after);
            assertions.push((Atom::Untracked(key), eps_of(eps)));
            self.eps_seen.entry(key).or_default().push(eps);
        }
        self.model.record_version(after, Some(before), assertions);
        Some(after)
    }

    fn fork(&mut self, from: Version) -> BranchId {
        let child = self.state.fork(from);
        self.state.check_invariants();
        self.model
            .record_version(self.state.head(child), Some(from), Vec::new());
        child
    }

    fn new_root(&mut self) -> BranchId {
        let root = self.state.new_root();
        self.state.check_invariants();
        self.model
            .record_version(self.state.head(root), None, Vec::new());
        root
    }

    /// Deletes one of the live branches, chosen by `index`, unless it is the last one. The oracle
    /// keeps the deleted versions: the facts about them stay true, the surviving branches still
    /// descend from them there, and they are just never asked about again.
    fn delete(&mut self, index: usize) {
        let live: Vec<BranchId> = self.state.branches().collect();
        if live.len() < 2 {
            return;
        }
        self.state.delete_branch(live[index % live.len()]);
        self.state.check_invariants();
    }

    fn write(&mut self, cert: &TestCert) {
        self.state.write(cert.clone());
        self.state.check_invariants();
        self.model.record_write(cert);
    }

    /// Soundness of one lookup against the judgment.
    fn check_lookup(&self, key: Key, v: Version) {
        if let Some(revision) = self.state.lookup(key, v).valid_revision() {
            assert!(
                self.model.is_justified(key, revision, v),
                "unsound: {key:?} resolves to {revision:?} at {v}, which is not justified"
            );
        }
    }

    /// Soundness of every key at every version.
    fn check_everything(&self) {
        for b in self.state.branches() {
            let first = self.state.first(b).seq().get();
            let head = self.state.head(b).seq().get();
            for seq in first..=head {
                let v = Version::new(b, Seq::testing_new(seq));
                if !self.model.has_version(v) {
                    continue;
                }
                let facts = self.model.justified(v);
                for key in (0..KEYS).map(any_key) {
                    if let Some(revision) = self.state.lookup(key, v).valid_revision() {
                        assert!(
                            facts.contains(&(Atom::Key(key), rev_of(revision))),
                            "unsound: {key:?} resolves to {revision:?} at {v}, which is not justified"
                        );
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Arbitrary certificates.

#[derive(Clone, Debug)]
enum GenChange {
    Assert { key: usize, revision: u32 },
    Dirty { key: usize },
}

#[derive(Clone, Debug)]
enum Op {
    Commit {
        branch: usize,
        changes: Vec<GenChange>,
    },
    Fork {
        branch: usize,
        seq: usize,
    },
    Write {
        key: usize,
        revision: u32,
        premises: Vec<(usize, u32)>,
        epsilon: usize,
    },
    Lookup {
        key: usize,
        branch: usize,
        seq: usize,
    },
    Delete {
        branch: usize,
    },
    NewRoot,
}

fn gen_ops(rng: &mut Rng, count: usize) -> Vec<Op> {
    (0..count)
        .map(|_| match rng.below(100) {
            0..=24 => Op::Commit {
                branch: rng.below(MAX_BRANCHES),
                changes: (0..1 + rng.below(3))
                    .map(|_| {
                        if rng.chance(60) {
                            GenChange::Assert {
                                key: rng.below(INJECTED),
                                revision: 1 + rng.below(3) as u32,
                            }
                        } else {
                            GenChange::Dirty {
                                key: rng.below(KEYS),
                            }
                        }
                    })
                    .collect(),
            },
            25..=30 => Op::Fork {
                branch: rng.below(MAX_BRANCHES),
                seq: rng.below(16),
            },
            31 => Op::NewRoot,
            32..=71 => Op::Write {
                key: rng.below(KEYS),
                revision: 1 + rng.below(3) as u32,
                premises: (0..rng.below(4))
                    .map(|_| (rng.below(KEYS), 1 + rng.below(3) as u32))
                    .collect(),
                epsilon: rng.below(4),
            },
            72..=95 => Op::Lookup {
                key: rng.below(KEYS),
                branch: rng.below(MAX_BRANCHES),
                seq: rng.below(16),
            },
            _ => Op::Delete {
                branch: rng.below(MAX_BRANCHES),
            },
        })
        .collect()
}

fn run_ops(_seed: u64, ops: &[Op]) {
    let (mut run, _) = Run::new();
    for (i, op) in ops.iter().enumerate() {
        match op {
            Op::Commit { branch, changes } => {
                let b = version_of(&run.state, *branch, 0).branch();
                let mut seen = HashSet::new();
                let changes: Vec<Change<Stamp>> = changes
                    .iter()
                    .filter_map(|c| match c {
                        GenChange::Assert { key, revision } => {
                            let key = injected(*key);
                            seen.insert(key).then_some(Change::Assert {
                                key,
                                revision: Revision::testing_new(*revision),
                                data: Stamp::mint(),
                            })
                        }
                        GenChange::Dirty { key } => {
                            let key = computed(*key);
                            seen.insert(key).then_some(Change::Dirty {
                                key,
                                data: Stamp::mint(),
                            })
                        }
                    })
                    .collect();
                run.commit(b, changes);
            }
            Op::Fork { branch, seq } => {
                if run.state.branches().count() < MAX_BRANCHES {
                    let from = version_of(&run.state, *branch, *seq);
                    run.fork(from);
                }
            }
            Op::Write {
                key,
                revision,
                premises,
                epsilon,
            } => {
                let key = computed(*key);
                let cert = Arc::new(Cert::new(
                    key,
                    Revision::testing_new(*revision),
                    premises
                        .iter()
                        .map(|(k, r)| Premise {
                            key: any_key(*k),
                            revision: Revision::testing_new(*r),
                        })
                        .collect(),
                    run.eps_for(key, *epsilon),
                ));
                run.write(&cert);
            }
            Op::Lookup { key, branch, seq } => {
                let v = version_of(&run.state, *branch, *seq);
                run.check_lookup(any_key(*key), v);
            }
            Op::Delete { branch } => run.delete(*branch),
            Op::NewRoot => {
                if run.state.branches().count() < MAX_BRANCHES {
                    run.new_root();
                }
            }
        }
        if i % 8 == 7 {
            run.check_everything();
        }
    }
    run.check_everything();
}

// ---------------------------------------------------------------------------------------------
// An honest environment.

#[derive(Clone, Debug)]
enum HonestChange {
    Assert { key: usize, value: u64 },
    Dirty { key: usize, content: u64 },
}

#[derive(Clone, Debug)]
enum HonestOp {
    Commit {
        branch: usize,
        changes: Vec<HonestChange>,
    },
    Fork {
        branch: usize,
        seq: usize,
    },
    Query {
        key: usize,
        branch: usize,
        seq: usize,
    },
    Delete {
        branch: usize,
    },
    NewRoot,
}

fn gen_honest_ops(rng: &mut Rng, count: usize) -> Vec<HonestOp> {
    (0..count)
        .map(|_| match rng.below(100) {
            0..=29 => HonestOp::Commit {
                branch: rng.below(MAX_BRANCHES),
                changes: (0..1 + rng.below(3))
                    .map(|_| {
                        if rng.chance(65) {
                            HonestChange::Assert {
                                key: rng.below(INJECTED),
                                value: 1 + rng.below(3) as u64,
                            }
                        } else {
                            HonestChange::Dirty {
                                key: rng.below(KEYS - INJECTED),
                                content: rng.below(3) as u64,
                            }
                        }
                    })
                    .collect(),
            },
            30..=36 => HonestOp::Fork {
                branch: rng.below(MAX_BRANCHES),
                seq: rng.below(16),
            },
            37 => HonestOp::NewRoot,
            38..=95 => HonestOp::Query {
                key: rng.below(KEYS),
                branch: rng.below(MAX_BRANCHES),
                seq: rng.below(16),
            },
            _ => HonestOp::Delete {
                branch: rng.below(MAX_BRANCHES),
            },
        })
        .collect()
}

/// The hidden compute functions: fixed deps over lower-numbered keys, and a value that is a
/// small function of the deps' values and of the key's untracked content, so that equal values
/// recur.
struct Honest {
    run: Run,
    deps: Vec<Vec<Key>>,
    next_revision: HashMap<Key, Revision>,
    /// The interning of values to revisions, per key, and its inverse.
    revisions: HashMap<(Key, u64), Revision>,
    values: HashMap<(Key, Revision), u64>,
    /// The untracked content behind each ε token a dirty minted; `INITIAL` is 0.
    content: HashMap<EpsilonToken, u64>,
}

impl Honest {
    fn new(rng: &mut Rng) -> Self {
        let mut deps = vec![Vec::new(); KEYS];
        for (key, key_deps) in deps.iter_mut().enumerate().skip(INJECTED) {
            let n = 1 + rng.below(3);
            for _ in 0..n {
                key_deps.push(any_key(rng.below(key)));
            }
            key_deps.sort();
            key_deps.dedup();
        }
        let (run, root) = Run::new();
        let mut honest = Honest {
            run,
            deps,
            next_revision: HashMap::new(),
            revisions: HashMap::new(),
            values: HashMap::new(),
            content: HashMap::new(),
        };
        honest.seed(root);
        honest
    }

    /// Asserts every injected key at a root's first commit, so that no lookup ever asks for a
    /// value before there is one.
    fn seed(&mut self, root: BranchId) {
        let changes = (0..INJECTED)
            .map(|i| Change::Assert {
                key: injected(i),
                revision: self.intern(injected(i), 1),
                data: Stamp::mint(),
            })
            .collect();
        self.run.commit(root, changes);
    }

    /// Whether `v` is the initial version of a root, where no injected key has a value yet.
    fn is_bare(&self, v: Version) -> bool {
        self.run.state.parent(v.branch()).is_none() && v.seq() == Seq::FIRST
    }

    fn intern(&mut self, key: Key, value: u64) -> Revision {
        if let Some(r) = self.revisions.get(&(key, value)) {
            return *r;
        }
        let next = self.next_revision.entry(key).or_insert(Revision::FIRST);
        let r = *next;
        *next = next.next();
        self.revisions.insert((key, value), r);
        self.values.insert((key, r), value);
        r
    }

    fn value_of(&self, key: Key, revision: Revision) -> u64 {
        self.values[&(key, revision)]
    }

    fn compute(&self, key: Key, dep_values: &[u64], content: u64) -> u64 {
        (dep_values.iter().sum::<u64>() + content + key.index as u64) % 4
    }

    /// The value a from-scratch computation of `key` at `v` produces, from the recorded history.
    fn expected_value(&self, key: Key, v: Version) -> u64 {
        if (key.index as usize) < INJECTED {
            let r = self
                .run
                .model
                .asserted_revision(key, v)
                .expect("every injected key is asserted at every version");
            return self.value_of(key, Revision::testing_new(r as u32));
        }
        let dep_values: Vec<u64> = self.deps[key.index as usize]
            .iter()
            .map(|d| self.expected_value(*d, v))
            .collect();
        let eps = self.run.model.asserted_epsilon(key, v);
        let content = self.content.get(&eps).copied().unwrap_or(0);
        self.compute(key, &dep_values, content)
    }

    /// Serves `key` at `v` the way dice's worker does, and returns its revision.
    fn ensure(&mut self, key: Key, v: Version) -> Revision {
        let expected = self.expected_value(key, v);
        if let Some(revision) = self.run.state.lookup(key, v).valid_revision() {
            assert_eq!(
                self.value_of(key, revision),
                expected,
                "{key:?} at {v} resolves to a value a from-scratch computation would not produce"
            );
            return revision;
        }
        assert!(
            (key.index as usize) >= INJECTED,
            "injected {key:?} is unknown at {v}"
        );
        let (candidate, epsilon) = match self.run.state.lookup(key, v) {
            crate::Lookup::Unknown {
                candidate, epsilon, ..
            } => (candidate.cloned(), epsilon),
            crate::Lookup::Valid { .. } => unreachable!(),
        };
        if let Some(candidate) = candidate
            && candidate.epsilon == epsilon
        {
            let premises: Vec<Premise> = candidate.premises().collect();
            let holds = premises.iter().all(|p| self.ensure(p.key, v) == p.revision);
            if holds {
                self.run.write(&candidate);
                self.check_attached_if_head(key, v, candidate.revision);
                return candidate.revision;
            }
        }
        let deps = self.deps[key.index as usize].clone();
        let dep_revisions: Vec<(Key, Revision)> =
            deps.iter().map(|d| (*d, self.ensure(*d, v))).collect();
        let dep_values: Vec<u64> = dep_revisions
            .iter()
            .map(|(d, r)| self.value_of(*d, *r))
            .collect();
        let content = self.content.get(&epsilon).copied().unwrap_or(0);
        let value = self.compute(key, &dep_values, content);
        assert_eq!(value, expected, "the harness disagrees with itself");
        let revision = self.intern(key, value);
        let cert = Arc::new(Cert::new(
            key,
            revision,
            dep_revisions
                .iter()
                .map(|(d, r)| Premise::new(*d, *r))
                .collect(),
            epsilon,
        ));
        self.run.write(&cert);
        self.check_attached_if_head(key, v, revision);
        revision
    }

    /// A write whose premises all hold at a branch's head attaches there (§5.2).
    fn check_attached_if_head(&self, key: Key, v: Version, revision: Revision) {
        if v == self.run.state.head(v.branch()) {
            assert_eq!(
                self.run.state.lookup(key, v).valid_revision(),
                Some(revision),
                "{key:?} was written at the head {v} with every premise valid, but did not attach"
            );
        }
    }

    /// The keys `key`'s from-scratch computation reads, transitively, `key` included.
    fn support_of(&self, key: Key) -> HashSet<Key> {
        let mut out = HashSet::new();
        let mut stack = vec![key];
        while let Some(k) = stack.pop() {
            if out.insert(k) {
                stack.extend(self.deps[k.index as usize].iter().copied());
            }
        }
        out
    }

    fn commit(&mut self, branch: usize, changes: &[HonestChange]) {
        let b = version_of(&self.run.state, branch, 0).branch();
        let head = self.run.state.head(b);
        let mut seen = HashSet::new();
        let mut changed: HashSet<Key> = HashSet::new();
        let mut core_changes = Vec::new();
        let mut minted_content = Vec::new();
        for change in changes {
            match change {
                HonestChange::Assert { key, value } => {
                    let key = injected(*key);
                    if !seen.insert(key) {
                        continue;
                    }
                    let revision = self.intern(key, *value);
                    if self.run.state.lookup(key, head).valid_revision() != Some(revision) {
                        changed.insert(key);
                    }
                    core_changes.push(Change::Assert {
                        key,
                        revision,
                        data: Stamp::mint(),
                    });
                }
                HonestChange::Dirty { key, content } => {
                    let key = computed(*key);
                    if !seen.insert(key) {
                        continue;
                    }
                    changed.insert(key);
                    core_changes.push(Change::Dirty {
                        key,
                        data: Stamp::mint(),
                    });
                    minted_content.push((key, *content));
                }
            }
        }
        let valid_before: Vec<(Key, Revision)> = (INJECTED..KEYS)
            .map(any_key)
            .filter_map(|k| {
                self.run
                    .state
                    .lookup(k, head)
                    .valid_revision()
                    .map(|r| (k, r))
            })
            .collect();
        let Some(new_head) = self.run.commit(b, core_changes) else {
            return;
        };
        for (key, content) in minted_content {
            let eps = self.run.state.epsilon(key, new_head);
            self.content.insert(eps, content);
        }
        // Keys outside the changed keys' support stay valid at the new head (§5.3's theorem
        // read as a completeness promise: the BFS closes what depends on a change, no more).
        for (key, revision) in valid_before {
            if self.support_of(key).is_disjoint(&changed) {
                assert_eq!(
                    self.run.state.lookup(key, new_head).valid_revision(),
                    Some(revision),
                    "{key:?} does not depend on {changed:?} but was closed by the commit at {new_head}"
                );
            }
        }
    }
}

fn run_honest_ops(seed: u64, ops: &[HonestOp]) {
    // The environment (dep graph and value function) is drawn from the case seed, so it varies
    // across cases and stays fixed while a case shrinks.
    let mut rng = Rng::new(seed ^ 0xa5a5_a5a5_a5a5_a5a5);
    let mut honest = Honest::new(&mut rng);
    for (i, op) in ops.iter().enumerate() {
        match op {
            HonestOp::Commit { branch, changes } => honest.commit(*branch, changes),
            HonestOp::Fork { branch, seq } => {
                if honest.run.state.branches().count() < MAX_BRANCHES {
                    let from = version_of(&honest.run.state, *branch, *seq);
                    // A root's initial version has no injected values; never fork it.
                    if !honest.is_bare(from) {
                        honest.run.fork(from);
                    }
                }
            }
            HonestOp::Query { key, branch, seq } => {
                let v = version_of(&honest.run.state, *branch, *seq);
                if !honest.is_bare(v) {
                    honest.ensure(any_key(*key), v);
                }
            }
            HonestOp::Delete { branch } => honest.run.delete(*branch),
            HonestOp::NewRoot => {
                if honest.run.state.branches().count() < MAX_BRANCHES {
                    let root = honest.run.new_root();
                    honest.seed(root);
                }
            }
        }
        if i % 8 == 7 {
            honest.run.check_everything();
        }
    }
    honest.run.check_everything();
}

// ---------------------------------------------------------------------------------------------
// Driver.

fn fails<O>(seed: u64, ops: &[O], run: fn(u64, &[O])) -> bool {
    catch_unwind(AssertUnwindSafe(|| run(seed, ops))).is_err()
}

/// Deletes operations one at a time while the case keeps failing.
fn shrink<O: Clone>(seed: u64, mut ops: Vec<O>, run: fn(u64, &[O])) -> Vec<O> {
    let mut progress = true;
    while progress {
        progress = false;
        let mut i = ops.len();
        while i > 0 {
            i -= 1;
            let mut candidate = ops.clone();
            candidate.remove(i);
            if fails(seed, &candidate, run) {
                ops = candidate;
                progress = true;
            }
        }
    }
    ops
}

fn fuzz<O: Clone + Debug>(name: &str, generate: fn(&mut Rng, usize) -> Vec<O>, run: fn(u64, &[O])) {
    let seed = seed();
    for case in 0..cases() {
        let case_seed = seed.wrapping_add(case);
        let mut rng = Rng::new(case_seed);
        let ops = generate(&mut rng, 60);
        if fails(case_seed, &ops, run) {
            let quiet = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            let small = shrink(case_seed, ops, run);
            std::panic::set_hook(quiet);
            eprintln!("{name}: failing case for seed {case_seed}, shrunk to {small:#?}");
            run(case_seed, &small);
            unreachable!("the shrunk case must fail like the original");
        }
    }
}

#[test]
fn arbitrary_certificates_are_never_unsoundly_valid() {
    fuzz("arbitrary certificates", gen_ops, run_ops);
}

#[test]
fn an_honest_environment_gets_sound_and_complete_answers() {
    fuzz("honest environment", gen_honest_ops, run_honest_ops);
}

impl Model {
    /// `asserted(k, v)` for a key, as the honest environment needs it.
    pub(crate) fn asserted_revision(&self, key: Key, v: Version) -> Option<u64> {
        self.asserted_atom(Atom::Key(key), v)
    }

    /// `asserted(k′, v)`.
    pub(crate) fn asserted_epsilon(&self, key: Key, v: Version) -> EpsilonToken {
        let e = self
            .asserted_atom(Atom::Untracked(key), v)
            .expect("untracked inputs always resolve");
        EpsilonToken::testing_from_u64(e)
    }
}
