/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! `new_root` (`incrementality.md` §2.2): what a fresh root knows, and how it comes to share.

use crate::EpsilonToken;
use crate::tests::State;
use crate::tests::StateExt;
use crate::tests::cert;
use crate::tests::k;
use crate::tests::leaf_and_dependent;
use crate::tests::r;
use crate::tests::v;

/// A new root has no parent to resolve through: nothing holds at it until asserted or written
/// for it. Certificates from other roots are offered, and hold once their premises do.
#[test]
fn a_new_root_starts_from_nothing() {
    let (mut s, first_root, c) = leaf_and_dependent();
    let root = s.new_root();
    s.check_invariants();
    let first = s.first(root);
    assert_eq!(first, v(root, 1));
    assert_eq!(s.head(root), first);
    assert!(s.parent(root).is_none());
    assert!(s.is_unknown_at(k(0), first));
    assert!(s.is_unknown_at(k(1), first));
    assert_eq!(s.candidate_at(k(1), first), Some(c.revision));

    let v2 = s.assert_at(root, k(0), r(1));
    assert_eq!(s.write_checked(&c).installed, vec![root]);
    assert_eq!(s.valid_at(k(1), v2), Some(r(1)));

    // A certificate without premises holds at every root at once.
    let leaf = cert(k(2), r(1), &[], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&leaf).installed, vec![first_root, root]);
}

/// Roots share one ε per key until one of them dirties it: every root's untracked inputs start
/// at the initial revision, so a certificate without premises holds at every root at once, one
/// made after the certificate was computed included. Only a dirty tells roots apart for such a
/// key.
#[test]
fn roots_share_the_initial_epsilon() {
    let mut s = State::new();
    let first = s.new_root();
    // Stamped the way a compute at the first root's head would stamp it.
    let c = cert(k(0), r(1), &[], s.epsilon(k(0), s.head(first)));
    assert_eq!(c.epsilon, EpsilonToken::INITIAL);
    let second = s.new_root();
    assert_eq!(s.epsilon(k(0), s.first(second)), EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&c).installed, vec![first, second]);
    assert_eq!(s.valid_at(k(0), s.first(second)), Some(r(1)));

    // A dirty at the second root separates them: the first root's certificate no longer covers
    // it, and one stamped with its new ε covers nothing but it.
    let v2 = s.dirty_at(second, k(0));
    let eps = s.epsilon(k(0), v2);
    assert_ne!(eps, EpsilonToken::INITIAL);
    assert!(s.is_unknown_at(k(0), v2));
    assert!(!s.write_checked(&c).installed.contains(&second));
    let c2 = cert(k(0), r(2), &[], eps);
    assert_eq!(s.write_checked(&c2).installed, vec![second]);
    assert_eq!(s.valid_at(k(0), v2), Some(r(2)));
    assert_eq!(s.valid_at(k(0), s.head(first)), Some(r(1)));
}

/// A root is a branch like any other to delete, the first one included.
#[test]
fn roots_can_be_deleted() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    assert_eq!(s.delete_checked(root), vec![k(0)]);
    assert!(!s.is_live(root));
    assert_eq!(s.branches().count(), 0);
    let again = s.new_root();
    assert_ne!(again, root);
    assert_eq!(s.branches().collect::<Vec<_>>(), vec![again]);
}
