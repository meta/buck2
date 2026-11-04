/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! `commit`, `incrementality.md` §5.3.

use crate::Change;
use crate::EpsilonToken;
use crate::Seq;
use crate::tests::Stamp;
use crate::tests::State;
use crate::tests::StateExt;
use crate::tests::cert;
use crate::tests::k;
use crate::tests::leaf_and_dependent;
use crate::tests::r;
use crate::tests::v;

#[test]
fn asserting_the_current_revision_mints_no_version() {
    let mut s = State::new();
    let root = s.new_root();
    assert_eq!(s.assert_at(root, k(0), r(1)), v(root, 2));
    assert_eq!(s.assert_at(root, k(0), r(1)), v(root, 2));
    assert_eq!(s.assert_at(root, k(0), r(2)), v(root, 3));
    assert_eq!(s.assert_at(root, k(0), r(1)), v(root, 4));
}

#[test]
fn a_commit_with_no_effective_change_returns_the_head() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    s.assert_at(root, k(1), r(1));
    let head = s.head(root);
    let after = s.commit_checked(
        root,
        vec![
            Change::Assert {
                key: k(0),
                revision: r(1),
                data: Stamp::mint(),
            },
            Change::Assert {
                key: k(1),
                revision: r(1),
                data: Stamp::mint(),
            },
        ],
    );
    assert_eq!(after, head);
}

#[test]
fn a_dirty_always_mints_a_version() {
    let mut s = State::new();
    let root = s.new_root();
    assert_eq!(s.dirty_at(root, k(0)), v(root, 2));
    assert_eq!(s.dirty_at(root, k(0)), v(root, 3));
    assert_ne!(s.epsilon(k(0), v(root, 2)), s.epsilon(k(0), v(root, 3)));
    assert_eq!(s.epsilon(k(0), v(root, 1)), EpsilonToken::INITIAL);
}

/// The BFS closes everything attached that transitively depends on the change, and nothing else.
#[test]
fn a_change_closes_its_transitive_dependents_only() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    s.assert_at(root, k(10), r(1));
    let head = s.head(root);
    // k1 <- k2 <- k3 hang off the leaf k0; k11 hangs off k10.
    for c in [
        cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL),
        cert(k(2), r(1), &[(k(1), r(1))], EpsilonToken::INITIAL),
        cert(k(3), r(1), &[(k(2), r(1))], EpsilonToken::INITIAL),
        cert(k(11), r(1), &[(k(10), r(1))], EpsilonToken::INITIAL),
    ] {
        s.write_checked(&c);
    }
    let next = s.assert_at(root, k(0), r(2));
    for key in [k(1), k(2), k(3)] {
        assert_eq!(
            s.valid_at(key, head),
            Some(r(1)),
            "{key:?} before the change"
        );
        assert!(s.is_unknown_at(key, next), "{key:?} after the change");
    }
    assert_eq!(s.valid_at(k(11), next), Some(r(1)));
    assert!(s.rdeps(root, k(0)).is_empty());
    assert!(s.rdeps(root, k(1)).is_empty());
    assert_eq!(s.rdeps(root, k(10)), &[k(11)]);
}

#[test]
fn batched_changes_close_overlapping_dependents_only() {
    for inherited in [false, true] {
        for reverse in [false, true] {
            let mut s = State::new();
            let root = s.new_root();
            s.assert_at(root, k(0), r(1));
            s.assert_at(root, k(1), r(1));
            for c in [
                cert(k(2), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL),
                cert(k(3), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL),
                cert(
                    k(4),
                    r(1),
                    &[(k(1), r(1)), (k(2), r(1)), (k(3), r(1))],
                    EpsilonToken::INITIAL,
                ),
                cert(k(5), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL),
            ] {
                s.write_checked(&c);
            }
            let branch = if inherited {
                s.fork_checked(s.head(root))
            } else {
                root
            };
            let before = s.head(branch);
            let mut changes = vec![
                Change::Assert {
                    key: k(0),
                    revision: r(1),
                    data: Stamp::mint(),
                },
                Change::Dirty {
                    key: k(2),
                    data: Stamp::mint(),
                },
                Change::Assert {
                    key: k(1),
                    revision: r(2),
                    data: Stamp::mint(),
                },
                Change::Dirty {
                    key: k(3),
                    data: Stamp::mint(),
                },
                Change::Dirty {
                    key: k(4),
                    data: Stamp::mint(),
                },
            ];
            if reverse {
                changes.reverse();
            }
            let after = s.commit_checked(branch, changes);
            assert_eq!(after, v(branch, before.seq().get() + 1));
            for key in [k(2), k(3), k(4)] {
                assert_eq!(s.valid_at(key, before), Some(r(1)));
                assert!(s.is_unknown_at(key, after));
                assert_eq!(s.epsilon(key, before), EpsilonToken::INITIAL);
                assert_ne!(s.epsilon(key, after), EpsilonToken::INITIAL);
                assert!(s.rdeps(branch, key).is_empty());
                if inherited {
                    assert_eq!(s.valid_at(key, s.head(root)), Some(r(1)));
                    assert_eq!(s.epsilon(key, s.head(root)), EpsilonToken::INITIAL);
                }
            }
            assert_eq!(s.valid_at(k(0), after), Some(r(1)));
            assert_eq!(s.valid_at(k(1), after), Some(r(2)));
            assert_eq!(s.valid_at(k(5), after), Some(r(1)));
            assert_eq!(s.rdeps(branch, k(0)), &[k(5)]);
            assert!(s.rdeps(branch, k(1)).is_empty());
            if inherited {
                assert_eq!(s.rdeps(root, k(0)), &[k(2), k(3), k(5)]);
            }
        }
    }
}

/// Force-dirtying an attached key closes it and its dependents; the key's untracked input moves
/// to a fresh revision at the new seq.
#[test]
fn a_dirty_closes_the_key_and_its_dependents() {
    let mut s = State::new();
    let root = s.new_root();
    let leaf = cert(k(0), r(1), &[], EpsilonToken::INITIAL);
    s.write_checked(&leaf);
    let dep = cert(k(1), r(1), &[(k(0), r(1))], EpsilonToken::INITIAL);
    s.write_checked(&dep);
    let v2 = s.dirty_at(root, k(0));
    assert!(s.is_unknown_at(k(0), v2));
    assert!(s.is_unknown_at(k(1), v2));
    assert_eq!(s.valid_at(k(0), v(root, 1)), Some(r(1)));
    assert_eq!(s.valid_at(k(1), v(root, 1)), Some(r(1)));
    assert_ne!(
        s.epsilon(k(0), v2),
        EpsilonToken::INITIAL,
        "the dirty just committed is in force at the new version"
    );
    assert_eq!(s.epsilon(k(0), v(root, 1)), EpsilonToken::INITIAL);
}

/// Closing an inherited claim gives the branch its own closed copy and leaves the parent alone.
#[test]
fn closing_an_inherited_claim_copies_it_to_the_committing_branch() {
    let (mut s, root, c) = leaf_and_dependent();
    let child = s.fork_checked(v(root, 2));
    let c2 = s.assert_at(child, k(0), r(2));
    assert!(s.is_unknown_at(k(1), c2));
    assert_eq!(s.candidate_at(k(1), c2), Some(c.revision));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    // The parent's claim is still attached.
    assert_eq!(s.valid_at(k(1), s.head(root)), Some(r(1)));
    let v3 = s.dirty_at(root, k(5));
    assert_eq!(s.valid_at(k(1), v3), Some(r(1)));
    assert_eq!(s.rdeps(root, k(0)), &[k(1)]);
    assert!(s.rdeps(child, k(0)).is_empty());
}

#[test]
fn seqs_are_per_branch() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    s.assert_at(root, k(0), r(2));
    let child = s.fork_checked(v(root, 3));
    assert_eq!(s.head(child).seq(), Seq::FIRST);
    let c2 = s.assert_at(child, k(0), r(3));
    assert_eq!(c2, v(child, 2));
    assert_eq!(s.head(root), v(root, 3));
}

#[test]
#[cfg_attr(
    all(miri, panic = "abort"),
    ignore = "with panic=abort the panic kills the in-process Miri harness"
)]
#[should_panic(expected = "never both")]
fn asserting_a_certified_key_panics() {
    let mut s = State::new();
    let root = s.new_root();
    s.write_checked(&cert(k(0), r(1), &[], EpsilonToken::INITIAL));
    s.assert_at(root, k(0), r(1));
}

#[test]
#[cfg_attr(
    all(miri, panic = "abort"),
    ignore = "with panic=abort the panic kills the in-process Miri harness"
)]
#[should_panic(expected = "never both")]
fn certifying_an_asserted_key_panics() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    s.write_checked(&cert(k(0), r(1), &[], EpsilonToken::INITIAL));
}

#[test]
#[cfg_attr(
    all(miri, panic = "abort"),
    ignore = "with panic=abort the panic kills the in-process Miri harness"
)]
#[should_panic(expected = "never both")]
fn dirtying_an_asserted_key_panics() {
    let mut s = State::new();
    let root = s.new_root();
    s.assert_at(root, k(0), r(1));
    s.dirty_at(root, k(0));
}

/// Asserting, on a child, the revision it inherits is no change: no version is minted and the
/// inherited dependents stay attached.
#[test]
fn asserting_the_inherited_revision_on_a_child_mints_no_version() {
    let (mut s, root, _) = leaf_and_dependent();
    let child = s.fork_checked(v(root, 2));
    assert_eq!(s.assert_at(child, k(0), r(1)), v(child, 1));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    assert_eq!(s.rdeps(child, k(0)), &[k(1)]);
    let child_v2 = s.assert_at(child, k(0), r(2));
    assert_eq!(child_v2, v(child, 2));
    assert!(s.is_unknown_at(k(1), child_v2));
}

/// A dirty at a child of a computed key the child inherits closes the key and its dependents at
/// the child only.
#[test]
fn dirtying_an_inherited_computed_key_closes_it_at_the_child_only() {
    let (mut s, root, _) = leaf_and_dependent();
    s.write_checked(&cert(k(2), r(1), &[(k(1), r(1))], EpsilonToken::INITIAL));
    let child = s.fork_checked(v(root, 2));
    let child_v2 = s.dirty_at(child, k(1));
    assert!(s.is_unknown_at(k(1), child_v2));
    assert_eq!(
        s.nearest_certificate(k(1), child_v2).map(|c| c.revision),
        Some(r(1))
    );
    assert!(s.is_unknown_at(k(2), child_v2));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    assert_eq!(s.valid_at(k(2), v(child, 1)), Some(r(1)));
    assert_eq!(s.epsilon(k(1), v(child, 1)), EpsilonToken::INITIAL);
    assert_ne!(s.epsilon(k(1), child_v2), EpsilonToken::INITIAL);
    assert_eq!(s.valid_at(k(1), v(root, 2)), Some(r(1)));
    assert_eq!(s.valid_at(k(2), v(root, 2)), Some(r(1)));
    assert!(s.rdeps(child, k(1)).is_empty());
    assert_eq!(s.rdeps(root, k(1)), &[k(2)]);
}

/// After the parent closes an inherited key and drains its edges, the child's own edges still
/// close the key when the child changes the dep.
#[test]
fn a_childs_edges_survive_the_parent_draining_its_own() {
    let (mut s, root, _) = leaf_and_dependent();
    let child = s.fork_checked(v(root, 2));
    s.assert_at(root, k(0), r(2));
    assert!(s.rdeps(root, k(0)).is_empty());
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
    let child_v2 = s.assert_at(child, k(0), r(3));
    assert!(s.is_unknown_at(k(1), child_v2));
    assert_eq!(s.valid_at(k(1), v(child, 1)), Some(r(1)));
}

/// Whether an assert is a change is decided against the branch's own view, which may differ
/// from the parent's: asserting the parent's revision again after an own change is a change.
#[test]
fn reasserting_the_parents_revision_after_an_own_change_is_a_change() {
    let (mut s, root, _) = leaf_and_dependent();
    let child = s.fork_checked(v(root, 2));
    assert_eq!(s.assert_at(child, k(0), r(2)), v(child, 2));
    assert_eq!(s.assert_at(child, k(0), r(1)), v(child, 3));
    assert_eq!(s.valid_at(k(0), v(child, 2)), Some(r(2)));
    assert_eq!(s.valid_at(k(0), v(child, 3)), Some(r(1)));
    assert_eq!(s.valid_at(k(0), v(root, 2)), Some(r(1)));
}

#[test]
#[cfg_attr(
    all(miri, panic = "abort"),
    ignore = "with panic=abort the panic kills the in-process Miri harness"
)]
#[should_panic(expected = "appears twice in one commit")]
fn a_key_appearing_twice_in_one_commit_panics() {
    let mut s = State::new();
    let root = s.new_root();
    s.commit(
        root,
        vec![
            Change::Dirty {
                key: k(1),
                data: Stamp::mint(),
            },
            Change::Dirty {
                key: k(1),
                data: Stamp::mint(),
            },
        ],
    );
}

/// Deleting the only branch with a claim on a key forgets the claim, not that the key has been
/// certified: asserting it afterwards is still the mistake it was before.
#[test]
#[cfg_attr(
    all(miri, panic = "abort"),
    ignore = "with panic=abort the panic kills the in-process Miri harness"
)]
#[should_panic(expected = "asserted after being certified")]
fn a_key_certified_on_a_deleted_branch_still_cannot_be_asserted() {
    let (mut s, root, _) = leaf_and_dependent();
    let other = s.new_root();
    s.delete_checked(root);
    s.assert_at(other, k(1), r(1));
}
