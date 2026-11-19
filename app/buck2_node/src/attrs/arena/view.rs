/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Decoded views of attribute values.

use buck2_util::strong_hasher::StrongHash128;

use crate::attrs::arena::schema::EqParts;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::storage::AttrToken;
use crate::attrs::arena::storage::AttrWord;
use crate::attrs::arena::storage::CoercedAttrArena;
use crate::attrs::arena::storage::RefRun;
use crate::attrs::coerced::CoercedAttrRefKind;
use crate::attrs::coerced::kinds::decode;
use crate::attrs::coerced::kinds::dispatch;

/// A decoded (shallow) view of one attribute value. Children decode lazily via
/// further `view` calls.
#[derive(Copy, Clone)]
pub struct CoercedAttrRef<'a> {
    pub(crate) arena: &'a CoercedAttrArena,
    pub(crate) r: AttrWord,
}

/// Content equality: the same kind with equal parts, across arenas.
impl PartialEq for CoercedAttrRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.content_eq(other)
    }
}

/// A run of child refs viewed as items of `T`: a sequence's values, a
/// dict's (key, value) pairs, or an arg's parts. `len`/`iter` only; the
/// items decode lazily.
pub struct Refs<'a, T> {
    pub(crate) run: RefRun<'a>,
    pub(super) _item: std::marker::PhantomData<T>,
}

impl<T> Copy for Refs<'_, T> {}
impl<T> Clone for Refs<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

/// How a [`Refs`] run decodes: `STRIDE` refs per item.
pub trait RefItem: Sized {
    const STRIDE: u32;
    type Item<'a>;
    /// Input form of the items for encoding, borrowed.
    type Spec: ?Sized;
    fn item<'a>(refs: Refs<'a, Self>, i: u32) -> Self::Item<'a>;
    fn tokens(spec: &Self::Spec) -> impl Iterator<Item = AttrToken> + '_;
}

/// One child value per ref.
pub struct Values;

impl RefItem for Values {
    const STRIDE: u32 = 1;
    type Item<'a> = CoercedAttrRef<'a>;
    type Spec = [AttrToken];
    fn item<'a>(refs: Refs<'a, Self>, i: u32) -> CoercedAttrRef<'a> {
        refs.run.arena.view(refs.run.get(i))
    }
    fn tokens(spec: &[AttrToken]) -> impl Iterator<Item = AttrToken> + '_ {
        spec.iter().copied()
    }
}

/// A (key, value) pair per two refs.
pub struct Pairs;

impl RefItem for Pairs {
    const STRIDE: u32 = 2;
    type Item<'a> = (CoercedAttrRef<'a>, CoercedAttrRef<'a>);
    type Spec = [(AttrToken, AttrToken)];
    fn item<'a>(refs: Refs<'a, Self>, i: u32) -> Self::Item<'a> {
        let run = refs.run;
        (
            run.arena.view(run.get(2 * i)),
            run.arena.view(run.get(2 * i + 1)),
        )
    }
    fn tokens(spec: &[(AttrToken, AttrToken)]) -> impl Iterator<Item = AttrToken> + '_ {
        spec.iter().flat_map(|(k, v)| [*k, *v])
    }
}

impl<'a, T: RefItem> Refs<'a, T> {
    pub fn len(&self) -> usize {
        (self.run.len() / T::STRIDE) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = T::Item<'a>> + use<'a, T> {
        let refs = *self;
        (0..self.len() as u32).map(move |i| T::item(refs, i))
    }

    /// Every child ref in storage order, whatever the item shape.
    pub(super) fn children(
        &self,
    ) -> impl ExactSizeIterator<Item = CoercedAttrRef<'a>> + use<'a, T> {
        let arena = self.run.arena;
        self.run.iter().map(move |r| arena.view(r))
    }
}

impl<'a> CoercedAttrRef<'a> {
    /// The ref itself: equal words in one arena are the same value.
    pub(crate) fn word(&self) -> AttrWord {
        self.r
    }

    pub fn kind(&self) -> CoercedAttrRefKind<'a> {
        decode(self.arena, self.r)
    }
}

impl<'a> CoercedAttrRef<'a> {
    /// The decoded parts if this value is of kind `K`: the typed downcast,
    /// for readers that want one kind rather than the sum `kind()` gives.
    ///
    /// `K` is one of the kind types in [`crate::attrs::coerced::kinds`].
    pub fn downcast<K: Kind>(&self) -> Option<View<'a, K>> {
        (self.arena.kind_of(self.r) == K::KIND).then(|| K::decode(self.arena, self.r))
    }

    /// The stored identity-grade content hash, if this ref was interned as
    /// a root. Equal content yields equal hashes across arenas and processes.
    pub fn root_strong_hash(&self) -> Option<StrongHash128> {
        self.arena.root_strong_hash(self.r)
    }

    /// Structural equality across (possibly different) arenas. Refs are
    /// arena-local, so cutoff comparisons between old and new values must
    /// compare content.
    pub fn content_eq(&self, other: &CoercedAttrRef<'_>) -> bool {
        let kind = self.arena.kind_of(self.r);
        kind == other.arena.kind_of(other.r)
            && dispatch(
                kind,
                EqParts {
                    a: *self,
                    b: *other,
                },
            )
    }
}
