/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Derivation attributes, `incrementality.md` §4.7: the attribute of a claim is that of the
//! derivation it stands for at its install site, not anything the writer knew.

use crate::EpsilonToken;
use crate::Lookup;
use crate::tests::State;
use crate::tests::StateExt;
use crate::tests::cert;
use crate::tests::k;
use crate::tests::r;
use crate::tests::v;

/// The attribute of a claim is the latest assertion its derivation rests on: an injected
/// premise's, or a dirty of a transitive premise once the dependents re-establish over it.
#[test]
fn a_claim_carries_the_latest_assertion_of_its_derivation() {
    let mut s = State::new();
    let root = s.new_root();
    let v2 = s.assert_at(root, k(0), r(1));
    let injected = s.attribute_at(k(0), v2);
    assert!(injected.is_some());
    let mid = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    let top = cert(k(2), r(1), &[(k(1), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&mid);
    s.write_checked(&top);
    assert_eq!(s.attribute_at(k(1), v2), injected);
    assert_eq!(s.attribute_at(k(2), v2), injected);

    let leaf = cert(k(3), r(1), &[], EpsilonToken::INITIAL);
    s.write_checked(&leaf);
    assert_eq!(s.attribute_at(k(3), v2), None);

    let v3 = s.dirty_at(root, k(1));
    let dirty = s.untracked_attribute_at(k(1), v3);
    assert!(dirty > injected);
    let mid_again = cert(k(1), r(1), &[(k(0), r(1))], s.epsilon(k(1), v3));
    s.write_checked(&mid_again);
    // The dependent's certificate is re-issued as is; its derivation now runs through the dirty.
    s.write_checked(&top);
    assert_eq!(s.attribute_at(k(1), v3), dirty);
    assert_eq!(s.attribute_at(k(2), v3), dirty);
}

/// An unknown lookup hands out the attribute of the key's own dirty, which the caller folds
/// into whatever it computes.
#[test]
fn an_unknown_lookup_reports_the_dirty_in_force() {
    let mut s = State::new();
    let root = s.new_root();
    let v2 = s.dirty_at(root, k(1));
    let dirty = s.untracked_attribute_at(k(1), v2);
    assert!(dirty.is_some());
    for (key, expected) in [(k(1), dirty), (k(5), None)] {
        match s.lookup(key, v2) {
            Lookup::Unknown { untracked, .. } => assert_eq!(untracked, expected),
            Lookup::Valid { .. } => panic!("{key:?} was never written"),
        }
    }
}

/// A child resolving a claim by inheritance gets its attribute unchanged.
#[test]
fn an_inherited_claim_keeps_its_attribute() {
    let mut s = State::new();
    let root = s.new_root();
    let v2 = s.assert_at(root, k(0), r(1));
    let mid = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&mid);
    let child = s.fork_checked(v2);
    assert_eq!(s.attribute_at(k(1), v(child, 1)), s.attribute_at(k(1), v2));
}

/// One write installs on every branch its premises hold on, and on each the claim's attribute
/// is that of the derivation there: two branches that reached one revision of a premise by
/// different assertions give the same certificate two attributes.
#[test]
fn a_certificate_installed_on_two_branches_gets_two_attributes() {
    let mut s = State::new();
    let root = s.new_root();
    let v2 = s.assert_at(root, k(0), r(1));
    let injected = s.attribute_at(k(0), v2);
    let mid = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&mid);
    let child = s.fork_checked(v2);
    // On the child, the premise is dirtied and recomputes to the same revision under the new ε.
    let c1 = s.dirty_at(child, k(1));
    let dirty = s.untracked_attribute_at(k(1), c1);
    let mid_on_child = cert(k(1), r(1), &[(k(0), r(1))], s.epsilon(k(1), c1));
    assert_eq!(s.write_checked(&mid_on_child).installed, vec![child]);
    assert_eq!(s.attribute_at(k(1), v2), injected);
    assert_eq!(s.attribute_at(k(1), c1), dirty);

    let top = cert(k(2), r(1), &[(k(1), r(1))], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&top).installed, vec![root, child]);
    assert_eq!(s.attribute_at(k(2), v2), injected);
    assert_eq!(s.attribute_at(k(2), c1), dirty);
}

/// `delete` moves the assertions a child inherited to the child's first version; the attributes
/// of the claims it re-homes are unchanged, as the assertions they rest on are the same.
#[test]
fn deleting_a_branch_keeps_the_attributes_of_what_it_passes_on() {
    let mut s = State::new();
    let root = s.new_root();
    let v2 = s.assert_at(root, k(0), r(1));
    let injected = s.attribute_at(k(0), v2);
    let v3 = s.dirty_at(root, k(1));
    let mid = cert(k(1), r(1), &[(k(0), r(1))], s.epsilon(k(1), v3));
    s.write_checked(&mid);
    let child = s.fork_checked(v3);
    let dirty = s.attribute_at(k(1), v3);
    assert_eq!(dirty, s.untracked_attribute_at(k(1), v3));
    s.delete_checked(root);
    assert_eq!(s.attribute_at(k(0), v(child, 1)), injected);
    assert_eq!(s.attribute_at(k(1), v(child, 1)), dirty);
}
