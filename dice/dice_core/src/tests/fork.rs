/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! `fork`, `incrementality.md` §5.4: the candidate set, the support filter and the child's map.

use crate::EpsilonToken;
use crate::tests::State;
use crate::tests::StateExt;
use crate::tests::cert;
use crate::tests::k;
use crate::tests::leaf_and_dependent;
use crate::tests::r;
use crate::tests::v;

#[test]
fn a_fork_inherits_attached_keys_with_their_edges() {
    let (mut s, root, _) = leaf_and_dependent();
    let c2 = cert(k(2), r(1), &[(k(1), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&c2);
    let child = s.fork_checked(v(root, 2));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    assert_eq!(s.valid_at(k(2), v(child, 1)), Some(r(1)));
    assert_eq!(s.rdeps(child, k(0)), &[k(1)]);
    assert_eq!(s.rdeps(child, k(1)), &[k(2)]);
}

#[test]
fn a_fork_from_an_older_seq_sees_that_seq() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    s.assert_at(root, k(0), r(2));
    let c = cert(k(1), r(2), &[(k(0), r(2))], EpsilonToken::INITIAL);
    s.write_checked(&c);
    let child = s.fork_checked(v(root, 2));
    assert_eq!(s.valid_at(k(0), v(child, 1)), Some(r(1)));
    assert!(s.is_unknown_at(k(1), v(child, 1)));
    assert!(s.rdeps(child, k(0)).is_empty());
}

/// A closed claim covering the fork seq is inherited with its edges, so a commit at the child
/// can find and close it.
#[test]
fn a_closed_claim_covering_the_fork_seq_is_inherited_with_edges() {
    let (mut s, root, _) = leaf_and_dependent();
    s.assert_at(root, k(0), r(2));
    assert!(s.is_unknown_at(k(1), v(root, 3)));
    let child = s.fork_checked(v(root, 2));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    assert_eq!(s.rdeps(child, k(0)), &[k(1)]);
    let c2 = s.assert_at(child, k(0), r(3));
    assert!(s.is_unknown_at(k(1), c2));
}

/// The round-6 counterexample: a closed claim whose dep re-attached with a window starting after
/// the fork seq is not supported there, and the child gets a shadow instead of an open claim.
#[test]
fn an_unsupported_closed_claim_is_shadowed_at_the_child() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    let d = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    let key = cert(k(2), r(1), &[(k(1), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&d);
    s.write_checked(&key);
    // The leaf changes and changes back; only d is revalidated, so its window restarts at 4
    // while k(2) keeps [2, 3).
    s.assert_at(root, k(0), r(2));
    s.assert_at(root, k(0), r(1));
    s.write_checked(&d);
    assert_eq!(s.valid_at(k(1), v(root, 4)), Some(r(1)));
    assert!(s.is_unknown_at(k(1), v(root, 2)));
    assert_eq!(s.valid_at(k(2), v(root, 2)), Some(r(1)));

    let child = s.fork_checked(v(root, 2));
    assert!(s.is_unknown_at(k(2), v(child, 1)));
    assert_eq!(s.candidate_at(k(2), v(child, 1)), Some(r(1)));
    assert!(s.is_unknown_at(k(1), v(child, 1)));
    assert!(s.rdeps(child, k(1)).is_empty());
    // Re-establishing d at the child makes k(2) installable there again.
    assert_eq!(s.write_checked(&d).installed, vec![child]);
    // ... and, since d is valid at the root's head again too, k(2) attaches there as well.
    assert_eq!(s.write_checked(&key).installed, vec![root, child]);
    assert_eq!(s.valid_at(k(2), v(child, 1)), Some(r(1)));
    assert_eq!(s.valid_at(k(2), v(root, 4)), Some(r(1)));
}

/// Closed claims that support each other are all inherited, in dependency order.
#[test]
fn supported_closed_chains_are_inherited_together() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    for c in [
        cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL),
        cert(k(2), r(1), &[(k(1), r(1))], EpsilonToken::INITIAL),
        cert(
            k(3),
            r(1),
            &[(k(2), r(1)), (k(0), r(1))],
            EpsilonToken::INITIAL,
        ),
    ] {
        s.write_checked(&c);
    }
    s.assert_at(root, k(0), r(2));
    let child = s.fork_checked(v(root, 2));
    for key in [k(1), k(2), k(3)] {
        assert_eq!(s.valid_at(key, v(child, 1)), Some(r(1)), "{key:?}");
    }
    let c2 = s.assert_at(child, k(0), r(3));
    for key in [k(1), k(2), k(3)] {
        assert!(s.is_unknown_at(key, c2), "{key:?}");
    }
}

/// A closed claim is a candidate at a fork only if its window covers the fork seq; the windows
/// on either side of a dirty are separate claims in time, and only the one in force is seen.
#[test]
fn a_closed_claim_not_covering_the_fork_seq_is_not_a_candidate() {
    let mut s = State::new();
    let root = s.new_root();
    let leaf = cert(k(0), r(1), &[], EpsilonToken::INITIAL);
    s.write_checked(&leaf);
    let v2 = s.dirty_at(root, k(0));
    // Recompute to the same value under the new ε, then dirty again so the claim is closed.
    s.write_checked(&cert(k(0), r(1), &[], s.epsilon(k(0), v2)));
    s.dirty_at(root, k(0));
    assert_eq!(s.valid_at(k(0), v2), Some(r(1)));
    // Forked before the first dirty, the window [2, 3) does not cover seq 1; forked at 2, the
    // claim is inherited with the ε it was stamped under.
    let before = s.fork_checked(v(root, 1));
    assert!(s.is_unknown_at(k(0), v(before, 1)));
    let at = s.fork_checked(v2);
    assert_eq!(s.valid_at(k(0), v(at, 1)), Some(r(1)));
}

// With panic=abort the panic kills the in-process Miri harness.
#[cfg(not(all(miri, panic = "abort")))]
#[test]
#[should_panic(expected = "not a version")]
fn forking_from_a_version_beyond_the_head_panics() {
    let mut s = State::new();
    let root = s.new_root();
    s.fork_checked(v(root, 2));
}

/// The support check compares the recorded revision, not just validity: a premise that resolves
/// to another revision at the fork point does not support the candidate.
#[test]
fn a_closed_claim_whose_premise_holds_another_revision_is_not_inherited() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1)); // 2
    let d1 = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    let key = cert(k(2), r(1), &[(k(1), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&d1);
    s.write_checked(&key);
    s.assert_at(root, k(0), r(2)); // 3: both close at [2, 3)
    // A premise-less certificate for k(1)@2 covers the whole branch, so k(1) now resolves to 2
    // at seq 2, where k(2)'s closed claim recorded 1.
    let d2 = cert(k(1), r(2), &[], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&d2).installed, vec![root]);
    assert_eq!(s.valid_at(k(1), v(root, 2)), Some(r(2)));

    let child = s.fork_checked(v(root, 2));
    assert!(s.is_unknown_at(k(2), v(child, 1)));
    assert_eq!(s.candidate_at(k(2), v(child, 1)), Some(r(1)));
    assert!(s.rdeps(child, k(1)).is_empty());
}

/// A grandchild forked below a shadow inherits the shadow, not the root's window behind it, and
/// follows the child once the child re-establishes the key.
#[test]
fn a_grandchild_below_a_shadow_follows_the_child() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    let d = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    let key = cert(k(2), r(1), &[(k(1), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&d);
    s.write_checked(&key);
    s.assert_at(root, k(0), r(2));
    s.assert_at(root, k(0), r(1));
    s.write_checked(&d);
    let child = s.fork_checked(v(root, 2));
    let grandchild = s.fork_checked(v(child, 1));
    assert!(s.is_unknown_at(k(2), v(grandchild, 1)));
    assert!(s.is_unknown_at(k(1), v(grandchild, 1)));
    assert!(s.rdeps(grandchild, k(1)).is_empty());

    assert!(s.write_checked(&d).installed.contains(&child));
    assert_eq!(s.valid_at(k(1), v(grandchild, 1)), Some(r(1)));
    let outcome = s.write_checked(&key);
    assert!(outcome.installed.contains(&root) && outcome.installed.contains(&child));
    assert_eq!(s.valid_at(k(2), v(grandchild, 1)), Some(r(1)));
    assert_eq!(s.rdeps(grandchild, k(1)), &[k(2)]);
}
