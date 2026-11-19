/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! `write`, `incrementality.md` §5.2, and certificate coverage, §4.5.

use crate::EpsilonToken;
use crate::tests::State;
use crate::tests::StateExt;
use crate::tests::cert;
use crate::tests::k;
use crate::tests::leaf_and_dependent;
use crate::tests::r;
use crate::tests::v;

#[test]
fn a_write_whose_cover_reaches_the_head_attaches() {
    let (s, root, _) = leaf_and_dependent();
    assert_eq!(s.valid_at(k(1), v(root, 2)), Some(r(1)));
    assert_eq!(s.rdeps(root, k(0)), &[k(1)]);
    let slot = &s.introspect_key(k(1)).slots[0];
    assert!(slot.claim.as_ref().unwrap().window.is_open());
}

#[test]
fn a_write_over_an_unknown_premise_installs_nowhere() {
    let mut s = State::new();
    let root = s.new_root();
    let c = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    let outcome = s.write_checked(&c);
    assert!(!outcome.installed_anywhere());
    assert!(s.is_unknown_at(k(1), v(root, 1)));
    assert!(!s.is_referenced(k(1), r(1)));
}

/// A premise that changed before the write bounds the window: the certificate is installed
/// closed, for the versions it does cover.
#[test]
fn a_write_whose_cover_stops_short_of_the_head_installs_a_closed_window() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    s.assert_at(root, k(0), r(2));
    let c = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    let outcome = s.write_checked(&c);
    assert_eq!(outcome.installed, vec![root]);
    assert_eq!(s.valid_at(k(1), v(root, 2)), Some(r(1)));
    assert!(s.is_unknown_at(k(1), v(root, 3)));
    assert!(
        s.rdeps(root, k(0)).is_empty(),
        "a closed claim owes no edges"
    );
}

/// An injected premise that went away and came back covers two separate intervals; the write
/// installs the most recent one, which reaches the head.
#[test]
fn cover_of_a_returning_injected_premise_picks_the_interval_at_the_head() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    s.assert_at(root, k(0), r(2));
    s.assert_at(root, k(0), r(1));
    let c = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&c);
    assert_eq!(s.valid_at(k(1), v(root, 4)), Some(r(1)));
    assert!(s.is_unknown_at(k(1), v(root, 3)));
    assert!(s.is_unknown_at(k(1), v(root, 2)));
    let claim = s.introspect_key(k(1)).slots[0].claim.clone().unwrap();
    assert_eq!(claim.window.from(), v(root, 4).seq());
    assert!(claim.window.is_open());
}

/// Once the premise went away, the certificate written earlier cannot be installed over the
/// middle interval: an install picks one interval and never a set.
#[test]
fn cover_of_a_returning_injected_premise_never_spans_the_gap() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    let c = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&c);
    s.assert_at(root, k(0), r(2));
    s.assert_at(root, k(0), r(1));
    // Re-issued at the head: the window restarts at the return.
    s.write_checked(&c);
    assert_eq!(s.valid_at(k(1), v(root, 4)), Some(r(1)));
    assert!(s.is_unknown_at(k(1), v(root, 3)));
}

#[test]
fn an_install_replaces_a_claim_only_if_it_reaches_further() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    s.assert_at(root, k(0), r(2));
    s.assert_at(root, k(0), r(3));
    // A certificate over r(2) covers [3, 4).
    let over_r2 = cert(k(1), r(2), &[(k(0), r(2))], EpsilonToken::INITIAL);
    assert!(s.write_checked(&over_r2).installed_anywhere());
    assert_eq!(s.valid_at(k(1), v(root, 3)), Some(r(2)));
    // One over r(1) covers [2, 3), which reaches less far: not installed.
    let over_r1 = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    assert!(!s.write_checked(&over_r1).installed_anywhere());
    assert_eq!(s.valid_at(k(1), v(root, 3)), Some(r(2)));
    assert!(s.is_unknown_at(k(1), v(root, 2)));
    // One over r(3) reaches the head: installed, and attached.
    let over_r3 = cert(k(1), r(3), &[(k(0), r(3))], EpsilonToken::INITIAL);
    assert!(s.write_checked(&over_r3).installed_anywhere());
    assert_eq!(s.valid_at(k(1), v(root, 4)), Some(r(3)));
    assert!(s.is_unknown_at(k(1), v(root, 3)));
}

/// A write of a different certificate for a key that is attached leaves it alone.
#[test]
fn an_attached_key_is_not_replaced_by_a_write() {
    let (mut s, root, _) = leaf_and_dependent();
    let other = cert(k(1), r(2), &[(k(0), r(1))], EpsilonToken::INITIAL);
    assert!(!s.write_checked(&other).installed_anywhere());
    assert_eq!(s.valid_at(k(1), v(root, 2)), Some(r(1)));
}

/// Re-issuing the candidate after an injected premise returned to its recorded revision
/// re-attaches without a recompute (§9.4 of the design notes, ABA).
#[test]
fn a_revalidated_certificate_reattaches() {
    let (mut s, root, c) = leaf_and_dependent();
    s.assert_at(root, k(0), r(2));
    let v4 = s.assert_at(root, k(0), r(1));
    assert_eq!(s.candidate_at(k(1), v4), Some(c.revision));
    assert_eq!(s.write_checked(&c).installed, vec![root]);
    assert_eq!(s.valid_at(k(1), v4), Some(r(1)));
    assert_eq!(s.rdeps(root, k(0)), &[k(1)]);
}

/// An earlier write's transaction racing a commit (G3): the write installs over the versions it
/// covers, which no longer include the head.
#[test]
fn a_write_racing_a_commit_is_retained_for_the_versions_it_covers() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    // A transaction at version 2 computes k(1); before it writes, the leaf changes.
    let c = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    s.assert_at(root, k(0), r(2));
    assert!(s.write_checked(&c).installed_anywhere());
    assert_eq!(s.valid_at(k(1), v(root, 2)), Some(r(1)));
    assert!(s.is_unknown_at(k(1), v(root, 3)));
}

/// A write installs at every branch it covers; a child that resolved nothing before and is
/// covered entirely inherits without a claim of its own (§5.2 step 2.5).
#[test]
fn a_write_covering_a_child_attaches_it_by_inheritance() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    let child = s.fork_checked(v(root, 2));
    let c = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&c).installed, vec![root]);
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    assert_eq!(s.rdeps(child, k(0)), &[k(1)]);
    assert!(
        s.introspect_key(k(1))
            .slots
            .iter()
            .all(|s| s.branch == root)
    );
    // The inherited attachment is a real one: a commit on the child closes it.
    let c2 = s.assert_at(child, k(0), r(2));
    assert!(s.is_unknown_at(k(1), c2));
    assert_eq!(s.valid_at(k(1), v(root, 2)), Some(r(1)));
}

/// A leaf certificate with no tracked premises covers every version its ε does.
#[test]
fn a_certificate_without_premises_covers_the_whole_branch() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(9), r(1));
    s.assert_at(root, k(9), r(2));
    let c = cert(k(1), r(1), &[], EpsilonToken::INITIAL);
    s.write_checked(&c);
    for seq in 1..=3 {
        assert_eq!(s.valid_at(k(1), v(root, seq)), Some(r(1)));
    }
}

/// A dependent recomputed over a dep whose claim is closed gets no further than that claim, and
/// never outlives the dep's next change.
#[test]
fn a_write_over_a_closed_premise_does_not_reattach_the_chain() {
    let mut s = State::new();
    let root = s.new_root();
    let c = cert(k(3), r(1), &[], EpsilonToken::INITIAL);
    let b = cert(k(2), r(1), &[(k(3), r(1))], EpsilonToken::INITIAL);
    let a = cert(k(1), r(1), &[(k(2), r(1))], EpsilonToken::INITIAL);
    for c in [&c, &b, &a] {
        s.write_checked(c);
    }
    let v2 = s.dirty_at(root, k(3));
    let eps = s.epsilon(k(3), v2);
    assert_eq!(
        s.write_checked(&cert(k(3), r(2), &[], eps)).installed,
        vec![root]
    );
    // A recompute of k(1) that read k(2)'s old value, k(2) never having been re-established.
    s.write_checked(&cert(k(1), r(2), &[(k(2), r(1))], EpsilonToken::INITIAL));
    assert!(s.is_unknown_at(k(1), v2));
    assert!(s.rdeps(root, k(2)).is_empty());
    let v3 = s.dirty_at(root, k(3));
    assert!(s.is_unknown_at(k(1), v3));
    assert!(s.is_unknown_at(k(2), v3));
}

/// A recompute at the head displaces a closed claim over an older version, and a transaction
/// still at that version is left Unknown, with the head's certificate as its only candidate. Its
/// own re-issue of the older certificate reaches no further than the head's claim and is not
/// installed. This is the precision given up for one claim per slot.
#[test]
fn a_write_at_the_head_displaces_a_closed_claim_over_an_older_version() {
    let (mut s, root, c1) = leaf_and_dependent();
    s.assert_at(root, k(0), r(2)); // 3
    let c2 = cert(k(1), r(2), &[(k(0), r(2))], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&c2).installed, vec![root]);
    assert!(s.is_unknown_at(k(1), v(root, 2)));
    assert_eq!(s.candidate_at(k(1), v(root, 2)), Some(r(2)));
    assert!(!s.write_checked(&c1).installed_anywhere());
    assert!(s.is_unknown_at(k(1), v(root, 2)));
    assert_eq!(s.valid_at(k(1), v(root, 3)), Some(r(2)));
}
