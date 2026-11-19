/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::hash::Hasher;

use allocative::Allocative;
use buck2_util::strong_hasher::Blake3StrongHasher;
use dupe::Dupe;
use pagable::Pagable;

/// Whether a directory is a complete listing of its contents within the entire build graph.
///
/// Roughly speaking:
///  - Usually, this is non-exhaustive. In most cases when we have an action input directory with an
///    entry at, say, `a/b/c`, we don't know whether there may also be an artifact defined at
///    `a/b/d` (and just not included in the directory).
///  - This is exhaustive basically only in the cases where it's inside a directory artifact, which
///    we know we've constructed exhaustively.
///
/// Almost nothing in buck needs this - for example, when we send actions to RE or the AC,
/// directories just are what they are.
///
/// However it is information that we need to retain because we need it in the materializer. In
/// particular, if the materializer sees `a/b/c`, it needs to know whether to delete the
/// pre-existing `a/b/d` or not.
#[derive(Copy, Clone, Dupe, PartialEq, Eq, Debug, Allocative)]
pub enum Exhaustiveness {
    /// These are the entries and there are no others; disk is to be made to match this listing
    /// exactly.
    Exhaustive,
    /// The listed entries exist; the listing is partial, and anything else present is not ours
    /// to touch.
    NonExhaustive,
}

impl Exhaustiveness {
    /// The join of the marking lattice: exhaustive wins.
    pub fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::NonExhaustive, Self::NonExhaustive) => Self::NonExhaustive,
            _ => Self::Exhaustive,
        }
    }
}

/// Identity of a directory tree's exhaustiveness marking.
///
/// Every fingerprinted directory carries an [`Exhaustiveness`] marking. The marking is
/// deliberately **not** part of the fingerprint: RE, the CAS, action digests, and all
/// serialized wire formats see identical bytes regardless of marking. Instead, a directory's
/// in-memory identity is the *pair* of its fingerprint and this hash: everything that means
/// "same directory" (equality, interning, merge short-circuits) must compare both. A single
/// bit alongside the fingerprint would not suffice for identity, because marking differences
/// deep in a tree are invisible in ancestor fingerprints — so this is a recursive hash over
/// the subtree's marking, a (cheap) second merkle parallel to the fingerprint.
///
/// The encoding:
/// - Bit 0 is the node's own flag: `1` = exhaustive.
/// - `0x1` means *uniformly exhaustive*: this node and every directory below it are
///   exhaustive. This is the normal state of content trees.
/// - `0x0` means *uniformly non-exhaustive*: this node and every directory below it are
///   non-exhaustive. This is the normal state of scaffolding, e.g. an assembled input tree
///   whose artifacts are all files.
/// - Anything else is `(mix << 1) | flag`, where `mix` is a non-zero hash of the direct
///   dir-children's `ExhaustivenessHash`es in entry order.
///
/// Two properties make this sound. Comparisons only ever matter between directories of equal
/// fingerprint, which therefore have identical shape: the two sentinel values each fully
/// determine the marking of the whole subtree, and in the mixed case, pairing dir-children
/// positionally (leaves contribute nothing) compares complete markings. And a mixed hash can
/// never equal a sentinel, because `mix` is forced non-zero.
///
/// Distinct markings of one content tree could in principle collide in the 64-bit mix. That
/// requires two coexisting marking-variants of the *same* tree (in practice a handful exist
/// per tree, ever) to collide, and the consequence is bounded to applying the wrong deletion
/// boundary — never wrong content. We accept those odds.
#[derive(
    Copy,
    Clone,
    Dupe,
    PartialEq,
    Eq,
    Hash,
    Allocative,
    Pagable,
    strong_hash::StrongHash
)]
pub struct ExhaustivenessHash(u64);

impl ExhaustivenessHash {
    pub const UNIFORMLY_EXHAUSTIVE: Self = Self(0x1);
    pub const UNIFORMLY_NON_EXHAUSTIVE: Self = Self(0x0);

    /// Whether this directory itself is an exhaustive listing.
    pub fn is_exhaustive(self) -> bool {
        self.0 & 1 != 0
    }

    /// Whether this directory and every directory below it are exhaustive.
    pub fn is_uniformly_exhaustive(self) -> bool {
        self == Self::UNIFORMLY_EXHAUSTIVE
    }

    /// Whether this directory and every directory below it are non-exhaustive.
    pub fn is_uniformly_non_exhaustive(self) -> bool {
        self == Self::UNIFORMLY_NON_EXHAUSTIVE
    }

    /// This directory's own marking (bit 0), disregarding the subtree.
    pub fn own_exhaustiveness(self) -> Exhaustiveness {
        if self.is_exhaustive() {
            Exhaustiveness::Exhaustive
        } else {
            Exhaustiveness::NonExhaustive
        }
    }

    /// For two directories of equal fingerprint: whether `self`'s marking already equals the
    /// pointwise join of both markings, making it free for a merge to keep `self`'s tree
    /// unchanged. When neither side covers the other, the join must be computed pointwise.
    pub fn covers(self, other: Self) -> bool {
        self == other || self.is_uniformly_exhaustive() || other.is_uniformly_non_exhaustive()
    }

    /// Computes the hash for a directory from its own marking and its direct dir-children's
    /// hashes, which `dir_children` must yield in entry order (leaf entries are skipped by the
    /// caller). The closure is called a second time only when the subtree is not uniform.
    pub fn compute<I: IntoIterator<Item = ExhaustivenessHash>>(
        exhaustiveness: Exhaustiveness,
        dir_children: impl Fn() -> I,
    ) -> Self {
        let uniform = match exhaustiveness {
            Exhaustiveness::Exhaustive => Self::UNIFORMLY_EXHAUSTIVE,
            Exhaustiveness::NonExhaustive => Self::UNIFORMLY_NON_EXHAUSTIVE,
        };
        if dir_children().into_iter().all(|c| c == uniform) {
            return uniform;
        }

        // Mixed hashes are embedded in pagable-persisted directory data, so the mix is part
        // of a persisted format: it must be computed with a hash whose algorithm never
        // changes, with platform-independent input bytes. Blake3 (the strong-hash choice) is
        // that; general-purpose hashers like `BuckHasher` are allowed to change between
        // versions.
        let mut hasher = Blake3StrongHasher::new();
        for c in dir_children() {
            hasher.write(&c.0.to_le_bytes());
        }
        // Mask to 63 bits so the shift below cannot truncate `mix` to zero, then force it
        // non-zero; mixed values are thereby always distinct from both sentinels.
        let mix = (hasher.finish() & (u64::MAX >> 1)).max(1);
        Self((mix << 1) | matches!(exhaustiveness, Exhaustiveness::Exhaustive) as u64)
    }
}

impl fmt::Debug for ExhaustivenessHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_uniformly_exhaustive() {
            write!(f, "UniformlyExhaustive")
        } else if self.is_uniformly_non_exhaustive() {
            write!(f, "UniformlyNonExhaustive")
        } else {
            write!(
                f,
                "Mixed({:#x}, exhaustive: {})",
                self.0,
                self.is_exhaustive()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn mixed_input() -> [ExhaustivenessHash; 2] {
        [
            ExhaustivenessHash::UNIFORMLY_EXHAUSTIVE,
            ExhaustivenessHash::UNIFORMLY_NON_EXHAUSTIVE,
        ]
    }

    #[test]
    fn test_sentinels() {
        assert_eq!(
            ExhaustivenessHash::compute(Exhaustiveness::Exhaustive, std::iter::empty),
            ExhaustivenessHash::UNIFORMLY_EXHAUSTIVE,
        );
        assert_eq!(
            ExhaustivenessHash::compute(Exhaustiveness::NonExhaustive, std::iter::empty),
            ExhaustivenessHash::UNIFORMLY_NON_EXHAUSTIVE,
        );
        assert_eq!(
            ExhaustivenessHash::compute(Exhaustiveness::Exhaustive, || {
                [ExhaustivenessHash::UNIFORMLY_EXHAUSTIVE; 3]
            }),
            ExhaustivenessHash::UNIFORMLY_EXHAUSTIVE,
        );
        assert_eq!(
            ExhaustivenessHash::compute(Exhaustiveness::NonExhaustive, || {
                [ExhaustivenessHash::UNIFORMLY_NON_EXHAUSTIVE; 3]
            }),
            ExhaustivenessHash::UNIFORMLY_NON_EXHAUSTIVE,
        );
    }

    #[test]
    fn test_frontier_nodes_are_mixed() {
        // A non-exhaustive dir over exhaustive children (a boundary frontier) and an
        // exhaustive dir over non-exhaustive children (a monotonicity violation, but identity
        // must still be sound) both hash as mixed.
        for exhaustiveness in [Exhaustiveness::Exhaustive, Exhaustiveness::NonExhaustive] {
            let child = match exhaustiveness {
                Exhaustiveness::Exhaustive => ExhaustivenessHash::UNIFORMLY_NON_EXHAUSTIVE,
                Exhaustiveness::NonExhaustive => ExhaustivenessHash::UNIFORMLY_EXHAUSTIVE,
            };
            let h = ExhaustivenessHash::compute(exhaustiveness, || [child]);
            assert!(!h.is_uniformly_exhaustive());
            assert!(!h.is_uniformly_non_exhaustive());
            assert_eq!(
                h.is_exhaustive(),
                exhaustiveness == Exhaustiveness::Exhaustive
            );
        }
    }

    #[test]
    fn test_own_marking_distinguishes() {
        let a = ExhaustivenessHash::compute(Exhaustiveness::Exhaustive, mixed_input);
        let b = ExhaustivenessHash::compute(Exhaustiveness::NonExhaustive, mixed_input);
        assert_ne!(a, b);
        assert!(a.is_exhaustive());
        assert!(!b.is_exhaustive());
    }

    #[test]
    fn test_child_order_distinguishes() {
        let ab = ExhaustivenessHash::compute(Exhaustiveness::NonExhaustive, mixed_input);
        let ba = ExhaustivenessHash::compute(Exhaustiveness::NonExhaustive, || {
            let [a, b] = mixed_input();
            [b, a]
        });
        assert_ne!(ab, ba);
    }

    #[test]
    fn test_join() {
        use Exhaustiveness::*;
        assert_eq!(NonExhaustive.join(NonExhaustive), NonExhaustive);
        assert_eq!(Exhaustive.join(NonExhaustive), Exhaustive);
        assert_eq!(NonExhaustive.join(Exhaustive), Exhaustive);
        assert_eq!(Exhaustive.join(Exhaustive), Exhaustive);
    }

    #[test]
    fn test_covers() {
        let top = ExhaustivenessHash::UNIFORMLY_EXHAUSTIVE;
        let bottom = ExhaustivenessHash::UNIFORMLY_NON_EXHAUSTIVE;
        let mixed_a = ExhaustivenessHash::compute(Exhaustiveness::NonExhaustive, mixed_input);
        let mixed_b = ExhaustivenessHash::compute(Exhaustiveness::Exhaustive, mixed_input);

        for h in [top, bottom, mixed_a, mixed_b] {
            assert!(h.covers(h));
            assert!(top.covers(h));
            assert!(h.covers(bottom));
        }
        assert!(!bottom.covers(mixed_a));
        assert!(!mixed_a.covers(top));
        assert!(!mixed_a.covers(mixed_b));
        assert!(!mixed_b.covers(mixed_a));
    }

    #[test]
    fn test_deterministic() {
        assert_eq!(
            ExhaustivenessHash::compute(Exhaustiveness::NonExhaustive, mixed_input),
            ExhaustivenessHash::compute(Exhaustiveness::NonExhaustive, mixed_input),
        );
    }

    /// Golden values locking mixed hashes on every platform. Mixed hashes are embedded in
    /// pagable-persisted directory data; if this fails, previously persisted data no longer
    /// matches what a fresh binary computes. That is a persisted-format change: do not update
    /// the golden values without versioning or invalidating that data.
    #[test]
    fn test_mixed_hashes_are_stable() {
        let mixed_n = ExhaustivenessHash::compute(Exhaustiveness::NonExhaustive, mixed_input);
        let mixed_e = ExhaustivenessHash::compute(Exhaustiveness::Exhaustive, mixed_input);
        let nested = ExhaustivenessHash::compute(Exhaustiveness::NonExhaustive, || {
            [mixed_n, mixed_e, ExhaustivenessHash::UNIFORMLY_EXHAUSTIVE]
        });
        assert_eq!(
            [mixed_n.0, mixed_e.0, nested.0],
            [0xe8a00d2eec2e91ce, 0xe8a00d2eec2e91cf, 0x6c9ec375220b0142],
            "see this test's doc comment before updating these values"
        );
    }
}
