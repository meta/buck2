/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;
use std::fmt::Debug;
use std::fmt::Display;
use std::hash::Hash;
use std::hash::Hasher;

use allocative::Allocative;
use pagable::PagableTagged;
use pagable::pagable_typetag;
use strong_hash::StrongHash;

#[pagable_typetag]
pub trait DynEvalKindKey:
    PagableTagged + Display + Send + Sync + Debug + Allocative + 'static
{
    fn hash(&self, state: &mut dyn Hasher);
    fn strong_hash(&self, state: &mut dyn Hasher);
    fn eq(&self, other: &dyn DynEvalKindKey) -> bool;
    fn as_any(&self) -> &dyn Any;

    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

impl<
    T: Display
        + Send
        + Sync
        + Debug
        + StrongHash
        + Allocative
        + Hash
        + Eq
        + PartialEq
        + Any
        + PagableTagged
        + 'static,
> DynEvalKindKey for T
{
    fn hash(&self, mut state: &mut dyn Hasher) {
        Hash::hash(self, &mut state)
    }

    fn strong_hash(&self, mut state: &mut dyn Hasher) {
        StrongHash::strong_hash(self, &mut state)
    }

    fn eq(&self, other: &dyn DynEvalKindKey) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|v| v == self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Hash for dyn DynEvalKindKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.hash(state)
    }
}

impl StrongHash for dyn DynEvalKindKey {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        StrongHash::strong_hash(self.type_name(), state);
        self.strong_hash(state)
    }
}

impl PartialEq for dyn DynEvalKindKey {
    fn eq(&self, other: &Self) -> bool {
        self.eq(other)
    }
}

impl Eq for dyn DynEvalKindKey {}
