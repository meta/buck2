/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! `delete_branch` (`incrementality.md` §5.5): what a deletion forgets, what it leaves alone,
//! and what it hands down to the deleted branch's children.

use crate::EpsilonToken;
use crate::arc::Arc;
use crate::tests::State;
use crate::tests::StateExt;
use crate::tests::cert;
use crate::tests::k;
use crate::tests::leaf_and_dependent;
use crate::tests::r;
use crate::tests::v;

#[test]
fn deleting_a_branch_forgets_its_claims_and_histories() {
    let (mut s, root, c1) = leaf_and_dependent();
    let child = s.fork_checked(v(root, 2));
    // The child diverges: a new revision of the leaf, and a value of the dependent over it.
    let child_v2 = s.assert_at(child, k(0), r(2));
    let c2 = cert(k(1), r(2), &[(k(0), r(2))], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&c2).installed, vec![child]);
    assert_eq!(s.valid_at(k(1), child_v2), Some(r(2)));
    assert!(s.is_referenced(k(0), r(2)));
    assert!(s.is_referenced(k(1), r(2)));

    assert_eq!(s.delete_checked(child), vec![k(0), k(1)]);
    assert!(!s.is_live(child));
    assert_eq!(s.branches().collect::<Vec<_>>(), vec![root]);
    assert!(!s.is_referenced(k(0), r(2)));
    assert!(!s.is_referenced(k(1), r(2)));

    // The branch it was forked from is untouched.
    assert_eq!(s.valid_at(k(0), v(root, 2)), Some(r(1)));
    assert_eq!(s.valid_at(k(1), v(root, 2)), Some(r(1)));
    assert!(s.is_referenced(k(0), r(1)));
    assert!(s.is_referenced(k(1), c1.revision));
    assert_eq!(s.rdeps(root, k(0)), &[k(1)]);
}

/// A branch that only inherited never had anything of its own to forget.
#[test]
fn deleting_a_branch_that_only_inherited_affects_no_key() {
    let (mut s, root, _) = leaf_and_dependent();
    let child = s.fork_checked(v(root, 2));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    assert!(s.delete_checked(child).is_empty());
    assert!(s.is_referenced(k(1), r(1)));
}

#[test]
fn ids_of_deleted_branches_are_not_reused() {
    let mut s = State::new();
    let root = s.new_root();
    let first = s.fork_checked(v(root, 1));
    s.delete_checked(first);
    let second = s.fork_checked(v(root, 1));
    assert_ne!(first, second);
    assert!(!s.is_live(first));
    assert!(s.is_live(second));
}

/// A certificate that would only have held on the deleted branch installs nowhere.
#[test]
fn a_write_installs_nothing_on_a_deleted_branch() {
    let (mut s, root, _) = leaf_and_dependent();
    let child = s.fork_checked(v(root, 2));
    s.assert_at(child, k(0), r(2));
    s.delete_checked(child);
    let c2 = cert(k(1), r(2), &[(k(0), r(2))], EpsilonToken::INITIAL);
    assert!(s.write_checked(&c2).installed.is_empty());
    assert!(!s.is_referenced(k(1), r(2)));
}

/// A dirty on the deleted branch does not survive it, nor does the closed copy of the inherited
/// claim it made: a later fork from the same point sees the fork point again.
#[test]
fn a_dirty_on_a_deleted_branch_is_forgotten() {
    let (mut s, root, _) = leaf_and_dependent();
    let child = s.fork_checked(v(root, 2));
    let dirtied = s.dirty_at(child, k(1));
    assert_ne!(s.epsilon(k(1), dirtied), EpsilonToken::INITIAL);
    assert_eq!(s.delete_checked(child), vec![k(1)]);
    assert!(s.is_referenced(k(1), r(1)));
    let again = s.fork_checked(v(root, 2));
    assert_eq!(s.epsilon(k(1), v(again, 1)), EpsilonToken::INITIAL);
    assert_eq!(s.valid_at(k(1), v(again, 1)), Some(r(1)));
}

/// Deleting a branch with children moves them to its parent, with everything they resolved
/// through it materialized on them: an open claim where its claim covered their fork point, an
/// empty one where it did not, and the history entries in force at the fork point.
#[test]
fn deleting_a_parent_preserves_what_its_children_resolve() {
    let (mut s, root, _) = leaf_and_dependent();
    let middle = s.fork_checked(v(root, 2));
    // `early` forks into the gap between the leaf's change and the dependent's recompute at the
    // middle branch, `late` after the recompute.
    let middle_v2 = s.assert_at(middle, k(0), r(2));
    let early = s.fork_checked(middle_v2);
    let middle_v3 = s.assert_at(middle, k(0), r(3));
    let c3 = cert(k(1), r(3), &[(k(0), r(3))], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&c3).installed, vec![middle]);
    let late = s.fork_checked(middle_v3);

    // What each child sees before: `late` the middle branch's claim and its second leaf
    // revision, `early` its first leaf revision and, for the dependent, `Unknown` with the
    // middle branch's non-covering claim as the candidate.
    assert_eq!(s.valid_at(k(0), v(late, 1)), Some(r(3)));
    assert_eq!(s.valid_at(k(1), v(late, 1)), Some(r(3)));
    assert_eq!(s.valid_at(k(0), v(early, 1)), Some(r(2)));
    assert!(s.is_unknown_at(k(1), v(early, 1)));
    assert_eq!(s.candidate_at(k(1), v(early, 1)), Some(r(3)));

    assert_eq!(s.delete_checked(middle), vec![k(0), k(1)]);
    assert_eq!(s.parent(early), Some(v(root, 2)));
    assert_eq!(s.parent(late), Some(v(root, 2)));

    assert_eq!(s.valid_at(k(0), v(late, 1)), Some(r(3)));
    assert_eq!(s.valid_at(k(1), v(late, 1)), Some(r(3)));
    assert_eq!(s.rdeps(late, k(0)), &[k(1)]);
    assert_eq!(s.valid_at(k(0), v(early, 1)), Some(r(2)));
    assert!(s.is_unknown_at(k(1), v(early, 1)));
    assert_eq!(s.candidate_at(k(1), v(early, 1)), Some(r(3)));
    assert!(s.is_referenced(k(0), r(2)));
    assert!(s.is_referenced(k(0), r(3)));
    assert!(s.is_referenced(k(1), r(3)));

    // The children go on as branches of their new parent: a commit at `late` closes the
    // materialized claim like any other, and `early` gets a value of its own.
    let late_v2 = s.assert_at(late, k(0), r(4));
    assert!(s.is_unknown_at(k(1), late_v2));
    let c2 = cert(k(1), r(2), &[(k(0), r(2))], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&c2).installed, vec![early]);
    assert_eq!(s.valid_at(k(1), v(early, 1)), Some(r(2)));
}

/// Deleting a root with children makes them roots.
#[test]
fn deleting_a_root_with_children_makes_them_roots() {
    let mut s = State::new();
    let root = s.new_root();
    let root_v2 = s.assert_at(root, k(0), r(1));
    let child = s.fork_checked(root_v2);
    assert_eq!(s.delete_checked(root), vec![k(0)]);
    assert_eq!(s.parent(child), None);
    assert_eq!(s.valid_at(k(0), v(child, 1)), Some(r(1)));
    assert_eq!(s.branches().collect::<Vec<_>>(), vec![child]);
    assert!(s.is_referenced(k(0), r(1)));
}

/// The untracked input's revision a child inherited is kept for it too, together with the
/// closed claim it made the deleted branch shadow.
#[test]
fn a_child_keeps_the_dirty_it_inherited() {
    let (mut s, root, c1) = leaf_and_dependent();
    let middle = s.fork_checked(v(root, 2));
    let dirtied = s.dirty_at(middle, k(1));
    let eps = s.epsilon(k(1), dirtied);
    let child = s.fork_checked(dirtied);
    assert_eq!(s.epsilon(k(1), v(child, 1)), eps);
    assert!(s.is_unknown_at(k(1), v(child, 1)));

    assert_eq!(s.delete_checked(middle), vec![k(1)]);
    assert_eq!(s.epsilon(k(1), v(child, 1)), eps);
    assert!(s.is_unknown_at(k(1), v(child, 1)));
    // A certificate under the old ε still cannot hold at the child; one under the inherited ε
    // can.
    assert!(s.write_checked(&c1).installed.is_empty());
    let recomputed = cert(k(1), r(1), &[(k(0), r(1))], eps);
    assert_eq!(s.write_checked(&recomputed).installed, vec![child]);
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
}

/// What deletion costs: a claim that the children shared through inheritance becomes one copy
/// per child. What resolves where does not change.
#[test]
fn deleting_a_parent_leaves_each_child_its_own_copy_of_a_shared_claim() {
    let (mut s, root, _) = leaf_and_dependent();
    let middle = s.fork_checked(v(root, 2));
    let middle_v2 = s.assert_at(middle, k(0), r(2));
    let c2 = cert(k(1), r(2), &[(k(0), r(2))], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&c2).installed, vec![middle]);
    let left = s.fork_checked(middle_v2);
    let right = s.fork_checked(middle_v2);
    // One claim at the middle branch serves both children.
    assert_eq!(s.introspect_key(k(1)).slots.len(), 2);
    assert_eq!(s.pinned_certs(k(1)).count(), 2);

    s.delete_checked(middle);
    for child in [left, right] {
        assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(2)));
        assert_eq!(s.rdeps(child, k(0)), &[k(1)]);
    }
    // Now each child holds a copy of its own, of the same certificate.
    assert_eq!(s.introspect_key(k(1)).slots.len(), 3);
    assert_eq!(
        s.pinned_certs(k(1)).filter(|c| Arc::ptr_eq(c, &c2)).count(),
        2
    );
}

// With panic=abort the panic kills the in-process Miri harness.
#[cfg(not(all(miri, panic = "abort")))]
#[test]
#[should_panic(expected = "has been deleted")]
fn deleting_a_branch_twice_panics() {
    let mut s = State::new();
    let root = s.new_root();
    let child = s.fork_checked(v(root, 1));
    s.delete_checked(child);
    s.delete_branch(child);
}

// With panic=abort the panic kills the in-process Miri harness.
#[cfg(not(all(miri, panic = "abort")))]
#[test]
#[should_panic(expected = "has been deleted")]
fn committing_to_a_deleted_branch_panics() {
    let mut s = State::new();
    let root = s.new_root();
    let child = s.fork_checked(v(root, 1));
    s.delete_checked(child);
    s.dirty_at(child, k(1));
}

// With panic=abort the panic kills the in-process Miri harness.
#[cfg(not(all(miri, panic = "abort")))]
#[test]
#[should_panic(expected = "has been deleted")]
fn forking_from_a_deleted_branch_panics() {
    let mut s = State::new();
    let root = s.new_root();
    let child = s.fork_checked(v(root, 1));
    s.delete_checked(child);
    s.fork_checked(v(child, 1));
}

// With panic=abort the panic kills the in-process Miri harness.
#[cfg(not(all(miri, panic = "abort")))]
#[test]
#[should_panic(expected = "has been deleted")]
fn looking_up_at_a_deleted_branch_panics() {
    let mut s = State::new();
    let root = s.new_root();
    let child = s.fork_checked(v(root, 1));
    s.delete_checked(child);
    s.lookup(k(1), v(child, 1));
}
