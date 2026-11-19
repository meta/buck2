/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The cascade obligations of an install, `incrementality.md` §5.2 step 2, one test per rule.

use crate::BranchId;
use crate::EpsilonToken;
use crate::tests::State;
use crate::tests::StateExt;
use crate::tests::cert;
use crate::tests::k;
use crate::tests::leaf_and_dependent;
use crate::tests::r;
use crate::tests::v;

/// Root: leaf k0 = r1 at seq 2, k1 over it attached, then the leaf changes at seq 3 and closes
/// k1. The child forks from seq 2, where k1 is valid.
fn parent_with_closed_claim_and_child_at_valid_seq() -> (State, BranchId, crate::tests::TestCert) {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    let c = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&c);
    s.assert_at(root, k(0), r(2));
    (s, root, c)
}

/// 2.1: a child with its own claim is not a party to the parent's install.
#[test]
fn child_with_own_claim_is_skipped() {
    let (mut s, root, _) = parent_with_closed_claim_and_child_at_valid_seq();
    let child = s.fork_checked(v(root, 2));
    let c2 = s.assert_at(child, k(0), r(5));
    let own = cert(k(1), r(7), &[(k(0), r(5))], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&own).installed, vec![child]);
    // One claim per slot: the child's own attach replaced the closed copy it had for seq 1.
    assert_eq!(s.valid_at(k(1), c2), Some(r(7)));
    assert!(s.is_unknown_at(k(1), v(child, 1)));
    // A new certificate at the parent covering the fork seq changes nothing at the child.
    let at_parent = cert(k(1), r(2), &[(k(0), r(2))], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&at_parent).installed, vec![root]);
    assert_eq!(s.valid_at(k(1), c2), Some(r(7)));
    assert!(s.is_unknown_at(k(1), v(child, 1)));
    assert_eq!(s.valid_at(k(1), v(root, 3)), Some(r(2)));
    let at_child = s
        .introspect_key(k(1))
        .slots
        .iter()
        .find(|s| s.branch == child)
        .unwrap();
    assert!(crate::arc::Arc::ptr_eq(
        &at_child.claim.as_ref().unwrap().cert,
        &own
    ));
}

/// 2.3: a child that resolved nothing and still resolves nothing gets no slot.
#[test]
fn child_unknown_before_and_after_is_untouched() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    s.assert_at(root, k(0), r(2));
    let child = s.fork_checked(v(root, 2));
    // Covers [3, ∞) at the root: not the child's fork seq.
    let c = cert(k(1), r(2), &[(k(0), r(2))], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&c).installed, vec![root]);
    assert!(s.is_unknown_at(k(1), v(child, 1)));
    assert!(
        s.introspect_key(k(1))
            .slots
            .iter()
            .all(|s| s.branch == root)
    );
}

/// 2.6 for a re-issue: the same certificate re-attached at the parent with a window that no
/// longer covers the fork seq. The child keeps its view through its own open copy.
#[test]
fn reissuing_a_certificate_with_a_later_window_rehomes_the_child() {
    let (mut s, root, c) = parent_with_closed_claim_and_child_at_valid_seq();
    let child = s.fork_checked(v(root, 2));
    let v4 = s.assert_at(root, k(0), r(1));
    assert_eq!(s.write_checked(&c).installed, vec![root]);
    assert_eq!(s.valid_at(k(1), v4), Some(r(1)));
    assert!(s.is_unknown_at(k(1), v(root, 2)));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    let at_child = s
        .introspect_key(k(1))
        .slots
        .iter()
        .find(|slot| slot.branch == child)
        .and_then(|slot| slot.claim.as_ref())
        .expect("the child was re-homed");
    assert!(at_child.window.is_open());
    assert!(crate::arc::Arc::ptr_eq(&at_child.cert, &c));
}

/// 2.4 compares traces, not allocations: a certificate equal to the one a child inherits, from a
/// different allocation, is the same certificate. Through `write` the case is only reachable
/// while the parent is not attached, i.e. after the child's fork point fell out of the window,
/// so the observable effect is the re-home not distinguishing the two allocations either.
#[test]
fn an_equal_certificate_from_another_allocation_is_the_same_certificate() {
    let (mut s, root, c) = parent_with_closed_claim_and_child_at_valid_seq();
    let child = s.fork_checked(v(root, 2));
    s.assert_at(root, k(0), r(1));
    let same_trace = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    assert!(!crate::arc::Arc::ptr_eq(&same_trace, &c));
    assert_eq!(s.write_checked(&same_trace).installed, vec![root]);
    assert_eq!(s.valid_at(k(1), v(root, 4)), Some(r(1)));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    let at_child = s
        .introspect_key(k(1))
        .slots
        .iter()
        .find(|slot| slot.branch == child)
        .and_then(|slot| slot.claim.as_ref())
        .expect("the child was re-homed");
    assert!(*at_child.cert == *same_trace);
}

/// 2.4 distinguishes certificates, not revisions: an equal-value recompute with different deps
/// is a different certificate, and the child keeps the one it has edges for.
#[test]
fn a_different_certificate_for_the_same_revision_rehomes_the_child() {
    let (mut s, root, _) = parent_with_closed_claim_and_child_at_valid_seq();
    s.assert_at(root, k(5), r(1));
    let child = s.fork_checked(v(root, 2));
    s.assert_at(root, k(0), r(1));
    // Same revision r(1), but now also over k(5).
    let c2 = cert(
        k(1),
        r(1),
        &[(k(0), r(1)), (k(5), r(1))],
        EpsilonToken::INITIAL,
    );
    assert_eq!(s.write_checked(&c2).installed, vec![root]);
    let child_slot = s
        .introspect_key(k(1))
        .slots
        .iter()
        .find(|s| s.branch == child)
        .expect("the child was re-homed");
    let claim = child_slot.claim.as_ref().unwrap();
    assert!(claim.window.is_open());
    assert!(claim.cert.premises().all(|p| p.key == k(0)));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    assert_eq!(s.rdeps(child, k(0)), &[k(1)]);
    assert!(s.rdeps(child, k(5)).is_empty());
}

/// 2.5: a child that newly resolves the certificate over its whole range is attached by edges
/// alone, and so are its own children.
#[test]
fn fill_in_attaches_child_and_grandchild_by_inheritance() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    let child = s.fork_checked(v(root, 2));
    let grandchild = s.fork_checked(v(child, 1));
    let c = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&c).installed, vec![root]);
    for b in [child, grandchild] {
        assert_eq!(s.valid_at(k(1), v(b, 1)), Some(r(1)));
        assert_eq!(s.rdeps(b, k(0)), &[k(1)]);
    }
    assert!(
        s.introspect_key(k(1))
            .slots
            .iter()
            .all(|s| s.branch == root)
    );
}

/// 2.5 falling through to 2.6: a child that newly resolves the certificate at its fork point
/// but not over its whole range is shadowed, and picks the certificate up on its own turn.
#[test]
fn partial_cover_at_the_child_shadows_then_installs_on_the_childs_turn() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    let child = s.fork_checked(v(root, 2));
    let c2 = s.assert_at(child, k(0), r(2));
    let c = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    // At the root the cover reaches the head; at the child it covers seq 1 only.
    assert_eq!(s.write_checked(&c).installed, vec![root, child]);
    assert_eq!(s.valid_at(k(1), v(root, 2)), Some(r(1)));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    assert!(s.is_unknown_at(k(1), c2));
    assert!(s.rdeps(child, k(0)).is_empty());
}

/// 2.6: a child that resolved the outgoing claim keeps resolving it after the parent replaces
/// it, through its own copy, open as it always saw it.
#[test]
fn replacing_a_claim_the_child_inherits_rehomes_it() {
    let (mut s, root, _) = parent_with_closed_claim_and_child_at_valid_seq();
    let child = s.fork_checked(v(root, 2));
    let c2 = cert(k(1), r(2), &[(k(0), r(2))], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&c2).installed, vec![root]);
    assert_eq!(s.valid_at(k(1), v(root, 3)), Some(r(2)));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    let c3 = s.dirty_at(child, k(9));
    assert_eq!(s.valid_at(k(1), c3), Some(r(1)));
    assert!(s.is_referenced(k(1), r(1)));
    // And the re-homed claim is a real attachment at the child.
    let c4 = s.assert_at(child, k(0), r(3));
    assert!(s.is_unknown_at(k(1), c4));
}

/// The worked example of design.md §9.5: a dirty at the parent after the fork must not let a
/// certificate stamped with the new ε reach the child, whose world still has the old input.
#[test]
fn a_dirty_at_the_parent_does_not_leak_into_a_child_forked_before_it() {
    let mut s = State::new();
    let root = s.new_root();
    let leaf = cert(k(0), r(1), &[], EpsilonToken::INITIAL);
    s.write_checked(&leaf);
    let child = s.fork_checked(v(root, 1));
    let v2 = s.dirty_at(root, k(0));
    let new_eps = s.epsilon(k(0), v2);
    assert_ne!(new_eps, EpsilonToken::INITIAL);
    assert_eq!(s.epsilon(k(0), v(child, 1)), EpsilonToken::INITIAL);
    // The leaf recomputes to a different value under the new ε.
    let leaf2 = cert(k(0), r(2), &[], new_eps);
    assert_eq!(s.write_checked(&leaf2).installed, vec![root]);
    assert_eq!(s.valid_at(k(0), v2), Some(r(2)));
    assert_eq!(s.valid_at(k(0), v(child, 1)), Some(r(1)));
    // An equal-value recompute under the new ε is a no-op everywhere: attached at the root
    // already, and the child's world still has the old input.
    let leaf_same = cert(k(0), r(1), &[], new_eps);
    assert!(!s.write_checked(&leaf_same).installed_anywhere());
    assert_eq!(s.valid_at(k(0), v(child, 1)), Some(r(1)));
    assert_eq!(s.valid_at(k(0), v2), Some(r(2)));
}

/// A write from an older version whose window closes at the parent still fills in a child forked
/// inside that window, by edges alone.
#[test]
fn a_closed_install_at_the_parent_fills_in_a_child_forked_inside_its_window() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1)); // 2
    let child = s.fork_checked(v(root, 2));
    s.assert_at(root, k(0), r(2)); // 3
    let c = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    assert!(s.write_checked(&c).installed.contains(&root));
    assert!(s.is_unknown_at(k(1), v(root, 3)));
    assert_eq!(s.valid_at(k(1), v(root, 2)), Some(r(1)));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    assert_eq!(s.rdeps(child, k(0)), &[k(1)]);
    assert!(
        s.introspect_key(k(1))
            .slots
            .iter()
            .all(|slot| slot.branch == root)
    );
    let child_v2 = s.assert_at(child, k(0), r(3));
    assert!(s.is_unknown_at(k(1), child_v2));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
}

/// A child's view of a key is frozen at its fork point even if the parent dirties the key and
/// then changes its dep; the child's own edges close it when the child changes the dep.
#[test]
fn a_parent_dirtying_then_changing_the_dep_leaves_the_childs_view_intact() {
    let (mut s, root, _) = leaf_and_dependent();
    let child = s.fork_checked(v(root, 2));
    s.dirty_at(root, k(1)); // 3
    s.assert_at(root, k(0), r(2)); // 4
    assert_eq!(s.valid_at(k(0), v(child, 1)), Some(r(1)));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    assert_eq!(s.rdeps(child, k(0)), &[k(1)]);
    let child_v2 = s.assert_at(child, k(0), r(3));
    assert!(s.is_unknown_at(k(1), child_v2));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
}
