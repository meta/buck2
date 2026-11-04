/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use dupe::Dupe;

use crate::ids::Key;
use crate::ids::Revision;
use crate::ids::Version;

/// One premise of a certificate: a key the compute read, and the revision it saw there.
#[derive(Copy, Clone, Dupe, Debug, PartialEq, Eq, Hash, Allocative)]
pub struct Premise {
    pub key: Key,
    pub revision: Revision,
}

impl Premise {
    pub fn new(key: Key, revision: Revision) -> Self {
        Premise { key, revision }
    }
}

/// The dependency payload of a certificate as the core reads it: a sequence of premises.
///
/// An environment may keep richer structure in its payload (dice keeps the series-parallel
/// shape of the compute, `incrementality.md` Appendix B); the core only ever iterates it, and
/// treats the premises as a set. A key may appear more than once. Equality of payloads is the
/// environment's, and decides whether two certificates are traces of the same circumstances.
pub trait Premises: PartialEq {
    fn premises(&self) -> impl Iterator<Item = Premise> + '_;
}

impl Premises for [Premise] {
    fn premises(&self) -> impl Iterator<Item = Premise> + '_ {
        self.iter().copied()
    }
}

impl Premises for Vec<Premise> {
    fn premises(&self) -> impl Iterator<Item = Premise> + '_ {
        self.iter().copied()
    }
}

impl Premises for Vec<(Key, Revision)> {
    fn premises(&self) -> impl Iterator<Item = Premise> + '_ {
        self.iter().map(|(k, r)| Premise::new(*k, *r))
    }
}

/// What an execution environment attaches to the core's objects: data on assertions, which the
/// core stores without interpreting, and an attribute of derivations (`incrementality.md` §4.7),
/// which the core computes from that data.
pub trait Env: 'static {
    /// The dependency payload of a certificate.
    type Premises: Premises + Allocative;

    /// Data kept with every assertion, i.e. with every injected revision and every dirty.
    type AssertionData: Copy + Allocative;

    /// The attribute of a derivation: [`Self::assertion`] at an (Assert) leaf, [`Self::combine`]
    /// over the premises at a (Cert) step. The core keeps the attribute of the derivation each
    /// claim stands for and hands it out with every `Valid` lookup.
    type Attribute: Copy + Allocative;

    /// The attribute of a derivation that rests on no assertion: a certificate without premises
    /// whose key was never dirtied.
    const EMPTY: Self::Attribute;

    /// The attribute of the assertion made at `at` with `data`.
    fn assertion(at: Version, data: Self::AssertionData) -> Self::Attribute;

    /// The attribute of a derivation from those of two parts of it. Commutative and associative,
    /// with [`Self::EMPTY`] as the identity. `a` and `b` are always attributes of derivations at
    /// one version, so every assertion in either lies on that version's chain.
    fn combine(a: Self::Attribute, b: Self::Attribute) -> Self::Attribute;
}
