/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Resolution, `incrementality.md` §4.4: own claim, own claim not covering, delegation.

use crate::EpsilonToken;
use crate::Lookup;
use crate::tests::State;
use crate::tests::StateExt;
use crate::tests::cert;
use crate::tests::k;
use crate::tests::leaf_and_dependent;
use crate::tests::r;
use crate::tests::v;

#[test]
fn unknown_key_is_unknown_with_the_initial_epsilon() {
    let mut s = State::new();
    let root = s.new_root();
    match s.lookup(k(7), v(root, 1)) {
        Lookup::Unknown {
            candidate, epsilon, ..
        } => {
            assert!(candidate.is_none());
            assert_eq!(epsilon, EpsilonToken::INITIAL);
        }
        Lookup::Valid { .. } => panic!("nothing was ever written"),
    }
}

/// Rule 1: a claim answers exactly over its window; rule 2: outside it, the claim is offered as
/// the candidate and no ancestor is consulted.
#[test]
fn own_claim_answers_over_its_window_only() {
    let (mut s, root, c) = leaf_and_dependent();
    assert_eq!(s.valid_at(k(1), v(root, 2)), Some(r(1)));
    // Changing the leaf closes the dependent at the new seq.
    let v3 = s.assert_at(root, k(0), r(2));
    assert_eq!(v3, v(root, 3));
    assert_eq!(s.valid_at(k(1), v(root, 2)), Some(r(1)));
    assert!(s.is_unknown_at(k(1), v3));
    assert_eq!(s.candidate_at(k(1), v3), Some(c.revision));
}

/// Rule 3: a branch without a claim resolves as its parent did at the fork point, at every one
/// of its seqs, whatever the parent does afterwards.
#[test]
fn branch_without_a_claim_resolves_as_its_parent_did_at_the_fork_point() {
    let (mut s, root, _) = leaf_and_dependent();
    let child = s.fork_checked(v(root, 2));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    // The parent moves on and closes its own claim; the child is forked before that.
    s.assert_at(root, k(0), r(2));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    assert_eq!(s.valid_at(k(0), v(child, 1)), Some(r(1)));
    // A commit on the child that does not touch the leaf leaves it valid there too.
    let c2 = s.dirty_at(child, k(9));
    assert_eq!(s.valid_at(k(1), c2), Some(r(1)));
}

/// Rule 2 on a child: once the child has its own claim, it alone speaks for the child.
#[test]
fn own_claim_on_a_child_blocks_inheritance() {
    let (mut s, root, _) = leaf_and_dependent();
    let child = s.fork_checked(v(root, 2));
    let c2 = s.assert_at(child, k(0), r(3));
    assert!(s.is_unknown_at(k(1), c2));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    // The parent is untouched by the child's divergence.
    assert_eq!(s.valid_at(k(1), v(root, 2)), Some(r(1)));
    assert_eq!(s.valid_at(k(0), v(root, 2)), Some(r(1)));
}

/// Injected keys resolve through their per-branch histories, falling through to the parent
/// before the branch's first own assertion (§4.3).
#[test]
fn asserted_key_falls_through_to_the_parent_before_its_first_own_assertion() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    let child = s.fork_checked(v(root, 2));
    let c2 = s.assert_at(child, k(0), r(2));
    assert_eq!(s.valid_at(k(0), v(child, 1)), Some(r(1)));
    assert_eq!(s.valid_at(k(0), c2), Some(r(2)));
    assert_eq!(s.valid_at(k(0), v(root, 2)), Some(r(1)));
    assert!(s.is_unknown_at(k(0), v(root, 1)));
}

/// With no claim on the resolution chain, the candidate is any claim the key has elsewhere.
#[test]
fn candidate_falls_back_to_a_claim_on_another_branch() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    let child = s.fork_checked(v(root, 2));
    s.assert_at(child, k(0), r(2));
    let c = cert(k(1), r(5), &[(k(0), r(2))], EpsilonToken::INITIAL);
    let outcome = s.write_checked(&c);
    assert_eq!(outcome.installed, vec![child]);
    assert!(s.is_unknown_at(k(1), v(root, 2)));
    assert_eq!(s.candidate_at(k(1), v(root, 2)), Some(r(5)));
}
