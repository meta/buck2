/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Certificates that premise each other (`incrementality.md` Appendix C).

use crate::EpsilonToken;
use crate::tests::State;
use crate::tests::StateExt;
use crate::tests::cert;
use crate::tests::k;
use crate::tests::r;
use crate::tests::v;

/// Covers are read off the state, never off the certificate being written.
#[test]
fn a_self_premised_certificate_installs_nowhere() {
    let mut s = State::new();
    let root = s.new_root();
    let c = cert(k(1), r(1), &[(k(1), r(1))], EpsilonToken::INITIAL);
    assert!(!s.write_checked(&c).installed_anywhere());
    assert!(s.is_unknown_at(k(1), v(root, 1)));
    assert_eq!(s.candidate_at(k(1), v(root, 1)), None);
    assert!(!s.is_referenced(k(1), r(1)));
}

#[test]
fn mutually_premised_certificates_install_nowhere_until_one_is_grounded() {
    let mut s = State::new();
    let root = s.new_root();
    let j_over_k = cert(k(1), r(1), &[(k(2), r(1))], EpsilonToken::INITIAL);
    let k_over_j = cert(k(2), r(1), &[(k(1), r(1))], EpsilonToken::INITIAL);
    for _ in 0..2 {
        assert!(!s.write_checked(&j_over_k).installed_anywhere());
        assert!(!s.write_checked(&k_over_j).installed_anywhere());
    }
    assert!(s.is_unknown_at(k(1), v(root, 1)));
    assert!(s.is_unknown_at(k(2), v(root, 1)));

    let k_leaf = cert(k(2), r(1), &[], EpsilonToken::INITIAL);
    assert_eq!(s.write_checked(&k_leaf).installed, vec![root]);
    assert_eq!(s.write_checked(&j_over_k).installed, vec![root]);
    assert_eq!(s.valid_at(k(1), v(root, 1)), Some(r(1)));
    assert_eq!(s.valid_at(k(2), v(root, 1)), Some(r(1)));
}

/// The island of Appendix C: early cutoff leaves behind two certificates that premise each other
/// and an injected key at a revision it has since returned to. Neither is valid, and re-issuing
/// them in any order bootstraps nothing.
#[test]
fn a_certificate_island_is_never_valid() {
    let mut s = State::new();
    let root = s.new_root();
    let (x, z, j, kk) = (k(0), k(9), k(1), k(2));
    s.assert_at(root, x, r(1)); // 2
    s.assert_at(root, z, r(2)); // 3
    s.write_checked(&cert(j, r(1), &[(x, r(1))], EpsilonToken::INITIAL));
    let k_over_z2_and_j = cert(kk, r(1), &[(z, r(2)), (j, r(1))], EpsilonToken::INITIAL);
    s.write_checked(&k_over_z2_and_j);
    s.assert_at(root, z, r(3)); // 4: kk closes
    // An equal recompute of kk that no longer reads j.
    s.write_checked(&cert(kk, r(1), &[(z, r(3))], EpsilonToken::INITIAL));
    s.assert_at(root, x, r(2)); // 5: j closes
    // An equal recompute of j that now reads kk.
    let j_over_x2_and_k = cert(j, r(1), &[(x, r(2)), (kk, r(1))], EpsilonToken::INITIAL);
    s.write_checked(&j_over_x2_and_k);
    assert_eq!(s.valid_at(j, v(root, 5)), Some(r(1)));
    assert_eq!(s.valid_at(kk, v(root, 5)), Some(r(1)));

    let v6 = s.assert_at(root, z, r(2)); // kk closes, then j
    assert!(s.is_unknown_at(j, v6));
    assert!(s.is_unknown_at(kk, v6));
    // Coinductively the pair would justify itself: kk over (z@2, j@1) and j over (x@2, kk@1),
    // with z back at 2 and x at 2. Each write only ever sees the other's closed claim.
    for _ in 0..2 {
        assert!(!s.write_checked(&k_over_z2_and_j).installed_anywhere());
        assert!(s.is_unknown_at(kk, v6));
        s.write_checked(&j_over_x2_and_k);
        assert!(s.is_unknown_at(j, v6));
    }
}
