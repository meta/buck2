/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! `StarlarkSerialize` and `StarlarkDeserialize` impls for common types
//! that delegate to their `PagableSerialize`/`PagableDeserialize` impls.
//!
//! This allows these types to be used directly in `#[derive(StarlarkPagable)]`
//! without needing `#[starlark_pagable(pagable)]`.

use std::hash::Hash;
use std::marker::PhantomData;
use std::num::NonZeroI32;
use std::sync::Arc;
use std::sync::Weak;

use dupe::Dupe;
use mini_map::MiniMutInsertionMap;
use mini_map::MiniMutInsertionSet;
use mini_vec::MiniBoxSlice;
use mini_vec::MiniVec;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use pagable::arc_erase::ArcErase;
use pagable::arc_erase::ArcEraseDyn;
use pagable::arc_erase::ArcEraseType;
use pagable::arc_erase::ArcSerializeOutcome;
use pagable::arc_erase::StdArcEraseType;
use pagable::arc_erase::WeakErase;
use pagable::arc_erase::deserialize_arc;
use starlark_map::Hashed;
use starlark_map::small_map::SmallMap;
use starlark_map::small_set::SmallSet;
use starlark_syntax::syntax::ast::AssignOp;

use crate::pagable::starlark_deserialize::StarlarkDeserialize;
use crate::pagable::starlark_deserialize::StarlarkDeserializeContext;
use crate::pagable::starlark_deserialize_context::StarlarkDeserializerImpl;
use crate::pagable::starlark_deserialize_context::deserialize_for_inspection;
use crate::pagable::starlark_serialize::StarlarkSerialize;
use crate::pagable::starlark_serialize::StarlarkSerializeContext;
use crate::pagable::starlark_serialize::StarlarkSerializeScope;
use crate::pagable::starlark_serialize_context::StarlarkSerializerImpl;
use crate::values::StringValue;

/// Implement `StarlarkSerialize` and `StarlarkDeserialize` for a type
/// by delegating to its `PagableSerialize`/`PagableDeserialize` impls.
macro_rules! impl_starlark_via_pagable {
    ($($ty:ty),* $(,)?) => {
        $(
            impl StarlarkSerialize for $ty {
                fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
                    PagableSerialize::pagable_serialize(self, ctx.pagable())?;
                    Ok(())
                }
            }

            impl<'fv> StarlarkDeserialize<'fv> for $ty {
                fn starlark_deserialize(ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>) -> crate::Result<Self> {
                    Ok(<$ty as PagableDeserialize>::pagable_deserialize(ctx.pagable())?)
                }
            }
        )*
    };
}

impl_starlark_via_pagable!(
    bool, u8, u16, u32, u64, usize, i8, i16, i32, i64, f32, f64, String,
);

impl_starlark_via_pagable!(starlark_map::StarlarkHashValue);

// `Hashed<K>` writes hash + key on the wire and reconstructs via the
// stored hash on deserialize (avoids requiring `K: Hash`).
impl<K: StarlarkSerialize> StarlarkSerialize for Hashed<K> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.hash().starlark_serialize(ctx)?;
        self.key().starlark_serialize(ctx)?;
        Ok(())
    }
}

impl<'fv, K: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv> for Hashed<K> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let hash = starlark_map::StarlarkHashValue::starlark_deserialize(ctx)?;
        let key = K::starlark_deserialize(ctx)?;
        Ok(Hashed::new_unchecked(hash, key))
    }
}

impl<T: StarlarkSerialize> StarlarkSerialize for [T] {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.len().pagable_serialize(ctx.pagable())?;
        for elem in self {
            elem.starlark_serialize(ctx)?;
        }
        Ok(())
    }
}

impl<T: StarlarkSerialize> StarlarkSerialize for Vec<T> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.len().pagable_serialize(ctx.pagable())?;
        for elem in self {
            elem.starlark_serialize(ctx)?;
        }
        Ok(())
    }
}

impl<'fv, T: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv> for Vec<T> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let len = usize::pagable_deserialize(ctx.pagable())?;
        let mut v = Vec::with_capacity(len);
        for _ in 0..len {
            v.push(T::starlark_deserialize(ctx)?);
        }
        Ok(v)
    }
}

impl<T: StarlarkSerialize + ?Sized> StarlarkSerialize for &T {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        (**self).starlark_serialize(ctx)
    }
}

impl<T: StarlarkSerialize> StarlarkSerialize for Box<T> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        (**self).starlark_serialize(ctx)
    }
}

impl<'fv, T: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv> for Box<T> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        Ok(Box::new(T::starlark_deserialize(ctx)?))
    }
}

impl<T: StarlarkSerialize> StarlarkSerialize for Option<T> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        match self {
            Some(v) => {
                true.pagable_serialize(ctx.pagable())?;
                v.starlark_serialize(ctx)?;
            }
            None => {
                false.pagable_serialize(ctx.pagable())?;
            }
        }
        Ok(())
    }
}

impl<'fv, T: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv> for Option<T> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let is_some = bool::pagable_deserialize(ctx.pagable())?;
        if is_some {
            Ok(Some(T::starlark_deserialize(ctx)?))
        } else {
            Ok(None)
        }
    }
}

// ============================================================================
// Insertion-ordered maps and sets, and starlark_map's facades over them
// ============================================================================

impl<K: StarlarkSerialize, V: StarlarkSerialize> StarlarkSerialize for MiniMutInsertionMap<K, V> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.len().pagable_serialize(ctx.pagable())?;
        for (k, v) in self.iter() {
            k.starlark_serialize(ctx)?;
            v.starlark_serialize(ctx)?;
        }
        Ok(())
    }
}

impl<'fv, K: SmallMapKeyDeserialize<'fv>, V: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv>
    for MiniMutInsertionMap<K, V>
{
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let len = usize::pagable_deserialize(ctx.pagable())?;
        let mut map = MiniMutInsertionMap::with_capacity(len);
        for _ in 0..len {
            let hashed_k = K::starlark_deserialize_hashed(ctx)?;
            let v = V::starlark_deserialize(ctx)?;
            map.insert_hashed(hashed_k, v);
        }
        Ok(map)
    }
}

impl<K: StarlarkSerialize, V: StarlarkSerialize> StarlarkSerialize for SmallMap<K, V> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.as_inner().starlark_serialize(ctx)
    }
}

impl<'fv, K: SmallMapKeyDeserialize<'fv>, V: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv>
    for SmallMap<K, V>
{
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        Ok(SmallMap::from(MiniMutInsertionMap::starlark_deserialize(
            ctx,
        )?))
    }
}

// ============================================================================
// SmallSet
// ============================================================================

impl<T: StarlarkSerialize> StarlarkSerialize for MiniMutInsertionSet<T> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.len().pagable_serialize(ctx.pagable())?;
        for v in self.iter() {
            v.starlark_serialize(ctx)?;
        }
        Ok(())
    }
}

impl<'fv, T: SmallMapKeyDeserialize<'fv>> StarlarkDeserialize<'fv> for MiniMutInsertionSet<T> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let len = usize::pagable_deserialize(ctx.pagable())?;
        let mut set = MiniMutInsertionSet::with_capacity(len);
        for _ in 0..len {
            let hashed = T::starlark_deserialize_hashed(ctx)?;
            set.insert_hashed(hashed);
        }
        Ok(set)
    }
}

impl<T: StarlarkSerialize> StarlarkSerialize for SmallSet<T> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.as_inner().starlark_serialize(ctx)
    }
}

impl<'fv, T: SmallMapKeyDeserialize<'fv>> StarlarkDeserialize<'fv> for SmallSet<T> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        Ok(SmallSet::from(MiniMutInsertionSet::starlark_deserialize(
            ctx,
        )?))
    }
}

/// Trait for types that can be deserialized as SmallMap keys, at the brand `'fv`.
/// Bridges the gap between types with `Hash` (use `Hashed::new`) and
/// starlark value types (use `get_hashed()`).
pub trait SmallMapKeyDeserialize<'fv>: StarlarkDeserialize<'fv> + Eq + Sized {
    /// Deserialize `Self` and compute its `Hashed` representation.
    fn starlark_deserialize_hashed(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Hashed<Self>>;
}

/// Impl for types with `Hash` — hash is computed via the standard `Hash` trait.
macro_rules! impl_small_map_key_hash {
    ($($ty:ty),* $(,)?) => {
        $(
            impl<'fv> SmallMapKeyDeserialize<'fv> for $ty {
                fn starlark_deserialize_hashed(
                    ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
                ) -> crate::Result<Hashed<Self>> {
                    let k = Self::starlark_deserialize(ctx)?;
                    Ok(Hashed::new(k))
                }
            }
        )*
    };
}

impl_small_map_key_hash!(String, bool, u8, u16, u32, u64, usize, i8, i16, i32, i64);

impl<'fv, T> SmallMapKeyDeserialize<'fv> for Arc<T>
where
    Arc<T>: StarlarkDeserialize<'fv> + Hash + Eq,
{
    fn starlark_deserialize_hashed(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Hashed<Self>> {
        let k: Self = deserialize_for_inspection(ctx)?;
        Ok(Hashed::new(k))
    }
}

/// `Value` has no `Hash` impl; `Value::get_hashed` hashes it. Hashing and
/// collision equality require readable keys, not unfinished construction references.
impl<'v> SmallMapKeyDeserialize<'v> for crate::values::Value<'v> {
    fn starlark_deserialize_hashed(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'v>,
    ) -> crate::Result<Hashed<Self>> {
        deserialize_for_inspection::<Self>(ctx)?.get_hashed()
    }
}

/// String hash is infallible.
impl<'v> SmallMapKeyDeserialize<'v> for StringValue<'v> {
    fn starlark_deserialize_hashed(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'v>,
    ) -> crate::Result<Hashed<Self>> {
        Ok(StringValue::starlark_deserialize(ctx)?.get_hashed())
    }
}

// ============================================================================
// Unit / tuples / fixed-size arrays
// ============================================================================

impl StarlarkSerialize for () {
    fn starlark_serialize(&self, _ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        Ok(())
    }
}

impl<'fv> StarlarkDeserialize<'fv> for () {
    fn starlark_deserialize(
        _ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        Ok(())
    }
}

macro_rules! impl_starlark_for_tuple {
    ($($name:ident: $idx:tt),+) => {
        impl<$($name: StarlarkSerialize),+> StarlarkSerialize for ($($name,)+) {
            fn starlark_serialize(
                &self,
                ctx: &mut dyn StarlarkSerializeContext,
            ) -> crate::Result<()> {
                $(self.$idx.starlark_serialize(ctx)?;)+
                Ok(())
            }
        }
        impl<'fv, $($name: StarlarkDeserialize<'fv>),+> StarlarkDeserialize<'fv> for ($($name,)+) {
            fn starlark_deserialize(
                ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
            ) -> crate::Result<Self> {
                Ok(($($name::starlark_deserialize(ctx)?,)+))
            }
        }
    };
}

impl_starlark_for_tuple!(T1: 0);
impl_starlark_for_tuple!(T1: 0, T2: 1);
impl_starlark_for_tuple!(T1: 0, T2: 1, T3: 2);
impl_starlark_for_tuple!(T1: 0, T2: 1, T3: 2, T4: 3);
impl_starlark_for_tuple!(T1: 0, T2: 1, T3: 2, T4: 3, T5: 4);
impl_starlark_for_tuple!(T1: 0, T2: 1, T3: 2, T4: 3, T5: 4, T6: 5);
impl_starlark_for_tuple!(T1: 0, T2: 1, T3: 2, T4: 3, T5: 4, T6: 5, T7: 6);
impl_starlark_for_tuple!(T1: 0, T2: 1, T3: 2, T4: 3, T5: 4, T6: 5, T7: 6, T8: 7);

impl<T: StarlarkSerialize, const N: usize> StarlarkSerialize for [T; N] {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        for elem in self.iter() {
            elem.starlark_serialize(ctx)?;
        }
        Ok(())
    }
}

impl<'fv, T: StarlarkDeserialize<'fv>, const N: usize> StarlarkDeserialize<'fv> for [T; N] {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let mut tmp: Vec<T> = Vec::with_capacity(N);
        for _ in 0..N {
            tmp.push(T::starlark_deserialize(ctx)?);
        }
        // SAFETY: pushed exactly N elements above.
        Ok(tmp.try_into().unwrap_or_else(|_| unreachable!()))
    }
}

// ============================================================================
// Box<[T]> (boxed slice)
// ============================================================================

impl<T: StarlarkSerialize> StarlarkSerialize for Box<[T]> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.len().pagable_serialize(ctx.pagable())?;
        for elem in self.iter() {
            elem.starlark_serialize(ctx)?;
        }
        Ok(())
    }
}

impl<'fv, T: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv> for Box<[T]> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let len = usize::pagable_deserialize(ctx.pagable())?;
        let mut v = Vec::with_capacity(len);
        for _ in 0..len {
            v.push(T::starlark_deserialize(ctx)?);
        }
        Ok(v.into_boxed_slice())
    }
}

impl<T: StarlarkSerialize> StarlarkSerialize for MiniVec<T> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.as_slice().starlark_serialize(ctx)
    }
}

impl<'fv, T: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv> for MiniVec<T> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let len = usize::pagable_deserialize(ctx.pagable())?;
        let mut v = MiniVec::with_capacity(len);
        for _ in 0..len {
            v.push(T::starlark_deserialize(ctx)?);
        }
        Ok(v)
    }
}

impl<T: StarlarkSerialize> StarlarkSerialize for MiniBoxSlice<T> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.as_slice().starlark_serialize(ctx)
    }
}

impl<'fv, T: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv> for MiniBoxSlice<T> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        Ok(MiniBoxSlice::from(MiniVec::starlark_deserialize(ctx)?))
    }
}

// ============================================================================
// PhantomData (zero-size, no-op ser/de)
// ============================================================================

impl<T: ?Sized> StarlarkSerialize for PhantomData<T> {
    fn starlark_serialize(&self, _ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        Ok(())
    }
}

impl<'fv, T: ?Sized> StarlarkDeserialize<'fv> for PhantomData<T> {
    fn starlark_deserialize(
        _ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        Ok(PhantomData)
    }
}

// ============================================================================
// CodeMap (from starlark_syntax) — bridges its `pagable::Pagable` impl into the
// starlark ser/de layer. Manual impl because we can't add `StarlarkPagableViaPagable`
// derive on a foreign type from outside its crate.
// ============================================================================

impl StarlarkSerialize for starlark_syntax::codemap::CodeMap {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        PagableSerialize::pagable_serialize(self, ctx.pagable())?;
        Ok(())
    }
}

impl<'fv> StarlarkDeserialize<'fv> for starlark_syntax::codemap::CodeMap {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        Ok(PagableDeserialize::pagable_deserialize(ctx.pagable())?)
    }
}

// `AssignOp` lives in `starlark_syntax`, we bridge its `Pagable` impl into
// the starlark layer here.
impl StarlarkSerialize for AssignOp {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        PagableSerialize::pagable_serialize(self, ctx.pagable())?;
        Ok(())
    }
}

impl<'fv> StarlarkDeserialize<'fv> for AssignOp {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        Ok(PagableDeserialize::pagable_deserialize(ctx.pagable())?)
    }
}

impl StarlarkSerialize for starlark_syntax::syntax::ast::Visibility {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        PagableSerialize::pagable_serialize(self, ctx.pagable())?;
        Ok(())
    }
}

impl<'fv> StarlarkDeserialize<'fv> for starlark_syntax::syntax::ast::Visibility {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        Ok(PagableDeserialize::pagable_deserialize(ctx.pagable())?)
    }
}

impl StarlarkSerialize for NonZeroI32 {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.get().pagable_serialize(ctx.pagable())?;
        Ok(())
    }
}

impl<'fv> StarlarkDeserialize<'fv> for NonZeroI32 {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let v = i32::pagable_deserialize(ctx.pagable())?;
        NonZeroI32::new(v)
            .ok_or_else(|| crate::Error::new_other(anyhow::anyhow!("expected non-zero i32, got 0")))
    }
}

// ============================================================================
// Arc<T> — Arc-identity dedup for `T: StarlarkSerialize + StarlarkDeserialize`.
//
// Pagable's `ArcErase` blanket requires `T: PagableSerialize`, which
// starlark-only types don't have. `StarlarkArcBridge<T>` wraps `Arc<T>` and
// impls `ArcErase` itself — recovering the starlark context inside ser/de.
//
// An `Arc`'s contents are shared by every value holding it, across heaps, so they can hold no
// branded value: `T` is `'static` and deserializable at every brand.
// ============================================================================

/// Wrapper that lets `Arc<T: StarlarkSerialize + StarlarkDeserialize>` plug
/// into pagable's `Arc` dedup machinery via starlark-context recovery.
struct StarlarkArcBridge<T: 'static> {
    inner: Arc<T>,
    // Repair provenance is not part of Arc identity and cannot change wire contents.
    scope: StarlarkSerializeScope,
}

impl<T: 'static> Clone for StarlarkArcBridge<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.dupe(),
            scope: self.scope,
        }
    }
}
impl<T: 'static> Dupe for StarlarkArcBridge<T> {}

/// Weak counterpart of [`StarlarkArcBridge`]. Wraps `Weak<T>` so its
/// `WeakErase::upgrade_weak` produces a boxed `StarlarkArcBridge<T>` rather
/// than the pagable-flavored Arc that pagable's blanket would produce.
struct StarlarkArcBridgeWeak<T: 'static> {
    inner: Weak<T>,
    scope: StarlarkSerializeScope,
}

impl<T: StarlarkSerialize + for<'fv> StarlarkDeserialize<'fv> + Send + Sync + 'static> WeakErase
    for StarlarkArcBridgeWeak<T>
{
    fn is_expired(&self) -> bool {
        self.inner.strong_count() == 0
    }

    fn upgrade_weak(&self) -> Option<Box<dyn ArcEraseDyn>> {
        self.inner.upgrade().map(|inner| {
            Box::new(StarlarkArcBridge {
                inner,
                scope: self.scope,
            }) as _
        })
    }
}

impl<T: StarlarkSerialize + for<'fv> StarlarkDeserialize<'fv> + Send + Sync + 'static> ArcErase
    for StarlarkArcBridge<T>
{
    type Weak = StarlarkArcBridgeWeak<T>;

    fn dupe_strong(&self) -> Self {
        self.dupe()
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        Arc::as_ptr(&self.inner) as *const () as usize
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        Some(StarlarkArcBridgeWeak {
            inner: Arc::downgrade(&self.inner),
            scope: self.scope,
        })
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn PagableSerializer,
    ) -> pagable::Result<ArcSerializeOutcome> {
        let state = StarlarkSerializerImpl::get_or_create_state(ser);
        let mut ctx = StarlarkSerializerImpl::recover_from_pagable(ser, state, self.scope);
        <T as StarlarkSerialize>::starlark_serialize(&self.inner, &mut ctx)
            .map_err(|e: crate::Error| e.into_anyhow())?;
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> pagable::Result<Self> {
        let inner = StarlarkDeserializerImpl::recover_from_pagable(deser.as_dyn(), |ctx| {
            T::starlark_deserialize(ctx)
        })
        .map_err(|e: crate::Error| e.into_anyhow())?;
        Ok(Self {
            inner: Arc::new(inner),
            scope: StarlarkSerializeScope::rootless(),
        })
    }
}

impl<T: StarlarkSerialize + for<'fv> StarlarkDeserialize<'fv> + Send + Sync + 'static>
    StarlarkSerialize for Arc<T>
{
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        let bridge = StarlarkArcBridge {
            inner: self.clone(),
            scope: ctx.serialization_scope(),
        };
        ctx.pagable().serialize_arc(&bridge)?;
        Ok(())
    }
}

impl<'fv, T: StarlarkSerialize + for<'x> StarlarkDeserialize<'x> + Send + Sync + 'static>
    StarlarkDeserialize<'fv> for Arc<T>
{
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let bridge: StarlarkArcBridge<T> = deserialize_arc(ctx.pagable())?;
        Ok(bridge.inner)
    }
}

// ============================================================================
// Either<A, B>
//
// Wire format: one u8 tag (0 = Left, 1 = Right) followed by the chosen variant.
// ============================================================================

impl<A: StarlarkSerialize, B: StarlarkSerialize> StarlarkSerialize for either::Either<A, B> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        match self {
            either::Either::Left(a) => {
                0u8.pagable_serialize(ctx.pagable())?;
                a.starlark_serialize(ctx)?;
            }
            either::Either::Right(b) => {
                1u8.pagable_serialize(ctx.pagable())?;
                b.starlark_serialize(ctx)?;
            }
        }
        Ok(())
    }
}

impl<'fv, A: StarlarkDeserialize<'fv>, B: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv>
    for either::Either<A, B>
{
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let tag = u8::pagable_deserialize(ctx.pagable())?;
        match tag {
            0 => Ok(either::Either::Left(A::starlark_deserialize(ctx)?)),
            1 => Ok(either::Either::Right(B::starlark_deserialize(ctx)?)),
            _ => Err(crate::Error::new_other(anyhow::anyhow!(
                "invalid Either tag: {}",
                tag
            ))),
        }
    }
}

impl<T: StarlarkSerialize> StarlarkSerialize for std::cell::OnceCell<T> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        let inner: Option<&T> = self.get();
        inner.starlark_serialize(ctx)
    }
}

impl<'fv, T: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv> for std::cell::OnceCell<T> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let val = <Option<T>>::starlark_deserialize(ctx)?;
        match val {
            None => Ok(std::cell::OnceCell::new()),
            Some(val) => Ok(val.into()),
        }
    }
}

impl<T: StarlarkSerialize> StarlarkSerialize for std::cell::RefCell<T> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        let inner = self.borrow();
        inner.starlark_serialize(ctx)
    }
}

impl<'fv, T: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv> for std::cell::RefCell<T> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let val = T::starlark_deserialize(ctx)?;
        Ok(val.into())
    }
}
