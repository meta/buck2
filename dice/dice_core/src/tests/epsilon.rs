/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Untracked inputs, `incrementality.md` §2.1: a certificate covers only versions whose ε is the
//! one it was stamped with, in either direction, however its tracked premises fare.

use crate::BranchId;
use crate::EpsilonToken;
use crate::tests::State;
use crate::tests::StateExt;
use crate::tests::cert;
use crate::tests::k;
use crate::tests::r;
use crate::tests::v;

fn with_injected_leaf() -> (State, BranchId) {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    (s, root)
}

/// A certificate stamped before a dirty does not cover versions after it, and is not offered
/// for re-establishment there either, though it remains the nearest certificate.
#[test]
fn a_dirty_ends_the_cover_of_earlier_certificates() {
    let (mut s, root) = with_injected_leaf();
    let c = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&c);
    let v3 = s.dirty_at(root, k(1));
    let v4 = s.dirty_at(root, k(9));
    for at in [v3, v4] {
        assert!(s.is_unknown_at(k(1), at));
        assert_eq!(s.candidate_at(k(1), at), None);
        assert_eq!(
            s.nearest_certificate(k(1), at).map(|c| c.revision),
            Some(r(1))
        );
        assert_ne!(s.epsilon(k(1), at), c.epsilon);
    }
    assert!(!s.write_checked(&c).installed_anywhere());
}

/// A certificate stamped after a dirty does not cover versions before it.
#[test]
fn a_certificate_under_a_later_epsilon_does_not_cover_earlier_versions() {
    let (mut s, root) = with_injected_leaf();
    let v3 = s.dirty_at(root, k(1));
    let v4 = s.dirty_at(root, k(9));
    let c = cert(k(1), r(1), &[(k(0), r(1))], s.epsilon(k(1), v4));
    s.write_checked(&c);
    assert!(s.is_unknown_at(k(1), v(root, 2)));
    assert_eq!(s.valid_at(k(1), v3), Some(r(1)));
    assert_eq!(s.valid_at(k(1), v4), Some(r(1)));
}

/// Unchanged tracked premises do not carry a window across a dirty.
#[test]
fn unchanged_premises_do_not_extend_a_window_across_a_dirty() {
    let (mut s, root) = with_injected_leaf();
    let v3 = s.dirty_at(root, k(1));
    let v4 = s.dirty_at(root, k(9));
    let c = cert(k(1), r(1), &[(k(0), r(1))], s.epsilon(k(1), v3));
    s.write_checked(&c);
    let claim = s.introspect_key(k(1)).slots[0].claim.clone().unwrap();
    assert_eq!(claim.window.from(), v3.seq());
    assert!(claim.window.is_open());
    assert!(s.is_unknown_at(k(1), v(root, 2)));
    assert_eq!(s.valid_at(k(1), v4), Some(r(1)));
}

/// Many dirties later, a certificate stamped with the initial ε still covers only the versions
/// before the first of them; with one claim per branch it is not installed when a more recent
/// claim exists.
#[test]
fn dirties_are_not_forgotten_by_later_writes() {
    let (mut s, root) = with_injected_leaf();
    for _ in 0..100 {
        s.dirty_at(root, k(1));
    }
    let head = s.head(root);
    let recent = cert(k(1), r(1), &[(k(0), r(1))], s.epsilon(k(1), head));
    s.write_checked(&recent);
    let initial = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    assert!(!s.write_checked(&initial).installed_anywhere());
    assert_eq!(s.valid_at(k(1), head), Some(r(1)));
    for seq in 2..=50 {
        assert!(s.is_unknown_at(k(1), v(root, seq)));
    }
    // Without the recent claim in the way, the initial certificate covers exactly [2, 3).
    let (mut fresh, fresh_root) = with_injected_leaf();
    fresh.dirty_at(fresh_root, k(1));
    assert!(fresh.write_checked(&initial).installed_anywhere());
    assert_eq!(fresh.valid_at(k(1), v(fresh_root, 2)), Some(r(1)));
    assert!(fresh.is_unknown_at(k(1), v(fresh_root, 3)));
}

/// Force-dirty and early cutoff compose (§5.3): a dirtied key that recomputes to an equal value
/// re-attaches under the new ε, and its dependents revalidate by revision without a recompute.
#[test]
fn an_equal_recompute_after_a_dirty_lets_dependents_revalidate() {
    let (mut s, root) = with_injected_leaf();
    let mid = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    let top = cert(k(2), r(1), &[(k(1), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&mid);
    s.write_checked(&top);
    let v3 = s.dirty_at(root, k(1));
    assert!(s.is_unknown_at(k(2), v3));
    let mid_again = cert(k(1), r(1), &[(k(0), r(1))], s.epsilon(k(1), v3));
    assert_eq!(s.write_checked(&mid_again).installed, vec![root]);
    assert_eq!(s.valid_at(k(1), v3), Some(r(1)));
    // The dependent's certificate names (k1, r1), not k1's ε: it re-establishes as is.
    assert_eq!(s.write_checked(&top).installed, vec![root]);
    assert_eq!(s.valid_at(k(2), v3), Some(r(1)));
}

#[test]
fn the_dirty_in_force_is_the_latest_at_or_before_the_version() {
    let (mut s, root) = with_injected_leaf();
    let v3 = s.dirty_at(root, k(1));
    let v4 = s.dirty_at(root, k(9));
    assert_ne!(s.epsilon(k(1), v3), EpsilonToken::INITIAL);
    assert_eq!(s.epsilon(k(1), v4), s.epsilon(k(1), v3));
    assert_eq!(s.epsilon(k(9), v3), EpsilonToken::INITIAL);
}

/// Dirties of one key on two branches mint distinct untracked revisions, so neither branch's
/// recompute installs on the other.
#[test]
fn dirties_on_two_branches_are_distinct_untracked_revisions() {
    let mut s = State::new();
    let root = s.new_root();
    s.write_checked(&cert(k(1), r(1), &[], EpsilonToken::INITIAL));
    let child = s.fork_checked(v(root, 1));
    let root_v2 = s.dirty_at(root, k(1));
    let child_v2 = s.dirty_at(child, k(1));
    let (eps_root, eps_child) = (s.epsilon(k(1), root_v2), s.epsilon(k(1), child_v2));
    assert_ne!(eps_root, eps_child);
    assert_ne!(eps_root, EpsilonToken::INITIAL);
    assert_ne!(eps_child, EpsilonToken::INITIAL);
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));

    let at_root = s.write_checked(&cert(k(1), r(2), &[], eps_root));
    assert_eq!(at_root.installed, vec![root]);
    assert_eq!(s.valid_at(k(1), root_v2), Some(r(2)));
    assert!(s.is_unknown_at(k(1), child_v2));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));

    let at_child = s.write_checked(&cert(k(1), r(3), &[], eps_child));
    assert_eq!(at_child.installed, vec![child]);
    assert_eq!(s.valid_at(k(1), child_v2), Some(r(3)));
    assert_eq!(s.valid_at(k(1), root_v2), Some(r(2)));
}

/// A claim re-homed onto a child brings no untracked-input history with it: the child's ε for
/// the key is read in the child's own seq space.
#[test]
fn a_rehomed_claim_carries_no_dirty_history() {
    let mut s = State::new();
    let root = s.new_root();
    s.write_checked(&cert(k(1), r(1), &[], EpsilonToken::INITIAL));
    let child = s.fork_checked(v(root, 1));
    for _ in 0..5 {
        s.dirty_at(root, k(9));
    }
    let root_v7 = s.dirty_at(root, k(1));
    let eps_root = s.epsilon(k(1), root_v7);
    let c2 = cert(k(1), r(2), &[], eps_root);
    assert_eq!(s.write_checked(&c2).installed, vec![root]);
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));

    let mut child_head = v(child, 1);
    for _ in 0..7 {
        child_head = s.dirty_at(child, k(9));
    }
    assert_eq!(s.epsilon(k(1), child_head), EpsilonToken::INITIAL);
    assert_eq!(s.valid_at(k(1), child_head), Some(r(1)));
    assert!(!s.write_checked(&c2).installed.contains(&child));
    assert_eq!(s.valid_at(k(1), child_head), Some(r(1)));
}
