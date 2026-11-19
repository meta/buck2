/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Pagable integration for `static_interner::Intern<T>`.
//!
//! These impls used to live in `static_interner` (gated by a `pagable` cargo
//! feature). They moved here so `static_interner` can be published to
//! crates.io independently of `pagable`.

use std::hash::Hash;
use std::hash::Hasher;

use static_interner::Intern;
use static_interner::Internable;

use crate::PagableDeserialize;
use crate::PagableDeserializer;
use crate::PagableSerialize;
use crate::PagableSerializer;
use crate::arc_erase::ArcErase;
use crate::arc_erase::ArcEraseType;
use crate::arc_erase::ArcSerializeOutcome;
use crate::arc_erase::StdArcEraseType;
use crate::arc_erase::deserialize_arc;

impl<
    T: PagableSerialize
        + for<'de> PagableDeserialize<'de>
        + std::fmt::Debug
        + Hash
        + Send
        + Sync
        + Internable<Hasher = H>
        + Eq
        + 'static,
    H: Hasher + Default + 'static,
> ArcErase for Intern<T>
{
    type Weak = ();
    fn dupe_strong(&self) -> Self {
        *self
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        // Intern equality is pointer equality. `deref_static` returns the
        // stable `'static` address of the interned value, which is a unique
        // per-Intern identity.
        Intern::deref_static(self) as *const T as usize
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        None
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn PagableSerializer,
    ) -> crate::Result<ArcSerializeOutcome> {
        T::pagable_serialize(self, ser)?;
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> crate::Result<Self> {
        let interner = T::interner();
        let val = T::pagable_deserialize(deser)?;
        Ok(interner.intern(val))
    }
}

impl<
    T: PagableSerialize
        + for<'de> PagableDeserialize<'de>
        + std::fmt::Debug
        + Send
        + Sync
        + Internable<Hasher = H>
        + Eq
        + Hash
        + 'static,
    H: Hasher + Default + 'static,
> PagableSerialize for Intern<T>
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl<
    'de,
    T: PagableSerialize
        + for<'a> PagableDeserialize<'a>
        + std::fmt::Debug
        + Hash
        + Send
        + Sync
        + Internable<Hasher = H>
        + Eq
        + std::any::Any,
    H: Hasher + Default + 'static,
> PagableDeserialize<'de> for Intern<T>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        deserialize_arc::<Self, _>(deserializer)
    }
}
