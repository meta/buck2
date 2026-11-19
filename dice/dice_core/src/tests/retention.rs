/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! What the state retains: certificates pinned by claims (`incrementality.md` §4.1), the
//! revisions they name.

use crate::EpsilonToken;
use crate::arc::Arc;
use crate::tests::StateExt;
use crate::tests::cert;
use crate::tests::k;
use crate::tests::leaf_and_dependent;
use crate::tests::r;
use crate::tests::v;

#[test]
fn a_revision_is_referenced_while_some_claim_names_it() {
    let (mut s, root, c1) = leaf_and_dependent();
    assert!(s.is_referenced(k(1), r(1)));
    s.assert_at(root, k(0), r(2));
    let c2 = cert(k(1), r(2), &[(k(0), r(2))], EpsilonToken::INITIAL);
    s.write_checked(&c2);
    assert!(s.is_referenced(k(1), r(2)));
    assert!(!s.is_referenced(k(1), r(1)));
    assert!(s.pinned_certs(k(1)).all(|c| Arc::ptr_eq(c, &c2)));
    assert!(!s.pinned_certs(k(1)).any(|c| Arc::ptr_eq(c, &c1)));
}

#[test]
fn a_child_that_still_resolves_a_replaced_claim_keeps_its_revision_referenced() {
    let (mut s, root, c1) = leaf_and_dependent();
    let child = s.fork_checked(v(root, 2));
    s.assert_at(root, k(0), r(2));
    let c2 = cert(k(1), r(2), &[(k(0), r(2))], EpsilonToken::INITIAL);
    s.write_checked(&c2);
    assert!(s.is_referenced(k(1), r(1)));
    assert!(s.is_referenced(k(1), r(2)));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(c1.revision));
}

#[test]
fn pinned_certificates_are_reported_per_claim() {
    let (mut s, root, c) = leaf_and_dependent();
    let child = s.fork_checked(v(root, 2));
    // The child's commit gives it a closed copy of the same certificate.
    s.assert_at(child, k(0), r(2));
    assert_eq!(s.introspect_key(k(1)).slots.len(), 2);
    assert_eq!(s.pinned_certs(k(1)).count(), 2);
    assert!(s.pinned_certs(k(1)).all(|p| Arc::ptr_eq(p, &c)));
}
