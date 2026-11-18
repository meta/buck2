/*
 * Copyright 2018 The Starlark in Rust Authors.
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

//! Converting values from a mutable [`Heap`](crate::values::Heap) into a
//! [`FrozenHeap`](crate::values::FrozenHeap).
//!
//! Freezing uses the same forwarding scheme as the moving garbage collector to
//! preserve sharing and cycles. [`Freezer::freeze`] first returns pointer-encoded
//! or already-frozen values unchanged and follows an existing forwarding record.
//! For a live heap allocation, it dispatches through that allocation's AValue
//! vtable.
//!
//! # Choosing a freeze path
//!
//! A value can provide its frozen representation in several ways:
//!
//! * Built-in AValue layouts, such as lists and tuples, can implement their
//!   complete `heap_freeze` operation directly when their trailing storage needs
//!   specialized handling.
//! * A value allocated with [`Heap::alloc_complex`](crate::values::Heap::alloc_complex)
//!   implements [`FreezeDynamic`], whose module is where the protocol lives. Most such
//!   values implement [`Freeze`] instead, which fixes the frozen type at compile time;
//!   the blanket [`FreezeDynamic`] implementation below plugs it into the protocol, and
//!   [`Freeze::prepare_freeze`] lets it reuse an existing frozen [`Value`] instead of
//!   allocating one.
//!
//! Every result must be equal to the source value and produce the same hash.
use std::cell::OnceCell;
use std::cell::RefCell;
use std::cell::UnsafeCell;
use std::marker::PhantomData;

use either::Either;
use mini_map::MiniMutInsertionMap;
use mini_map::MiniMutInsertionSet;
use mini_vec::MiniBoxSlice;
use mini_vec::MiniVec;
use starlark_map::Hashed;
use starlark_map::small_map::SmallMap;
use starlark_map::small_set::SmallSet;
use starlark_syntax::slice_vec_ext::VecExt;

use crate::any::ProvidesStaticType;
use crate::values::FreezeDynamic;
use crate::values::FreezeError;
use crate::values::FreezePlan;
use crate::values::FreezeResult;
use crate::values::FreezeSlot;
use crate::values::FreezeTarget;
use crate::values::Freezer;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::layout::avalue::AValueSimpleBound;

/// Need to be implemented for non-simple `StarlarkValue`.
///
/// This is called on freeze of the heap. Must produce a replacement object to place
/// in the frozen heap.
///
/// `'v` is the brand of the heap the value is frozen out of, like the `'v` of
/// [`Trace<'v>`](crate::values::Trace): a type branded by `'v` implements `Freeze<'v>`, a
/// type that holds no values implements it for every `'v`.
///
/// For relatively simple cases it can be implemented with `#[derive(Freeze)]`:
///
/// ```
/// # struct AdditionalData;
///
/// use starlark::values::Freeze;
/// use starlark::values::Value;
///
/// #[derive(Freeze)]
/// struct MyType<'v> {
///     value: Value<'v>,
///     // This field does not implement `Freeze`, but we can use it as is for freeze.
///     #[freeze(identity)]
///     data: AdditionalData,
/// }
/// ```
pub trait Freeze<'v> {
    /// When type is frozen, it is frozen into this type.
    type Frozen<'fv>;

    /// Selects whether to allocate `Frozen` or reuse an existing frozen value.
    ///
    /// This is called while the source is still intact. Most implementations
    /// should use the default allocation plan.
    fn prepare_freeze<'fv>(
        &self,
        _freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<StaticFreezePlan<'v, 'fv, Self>>
    where
        Self: Sized,
        Self::Frozen<'fv>: StarlarkValue<'fv>,
    {
        Ok(StaticFreezePlan::allocate())
    }

    /// Freeze a value. The frozen value _must_ be equal to the original,
    /// and produce the same hash.
    ///
    /// Note during freeze, `Value` objects in `Self` might be already special forward-objects,
    /// trying to unpack these objects will crash the process.
    /// So the function is only allowed to access `Value` objects after it froze them.
    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>>;
}

/// Destination selected by [`Freeze::prepare_freeze`].
pub struct StaticFreezePlan<'v, 'fv, T>
where
    T: Freeze<'v>,
    T::Frozen<'fv>: StarlarkValue<'fv>,
{
    direct: Option<ValueTyped<'fv, T::Frozen<'fv>>>,
    marker: PhantomData<fn(&'v ()) -> T>,
}

impl<'v, 'fv, T> StaticFreezePlan<'v, 'fv, T>
where
    T: Freeze<'v>,
    T::Frozen<'fv>: StarlarkValue<'fv>,
{
    /// Allocates a new `T::Frozen` in the frozen heap. This is the default.
    pub fn allocate() -> Self {
        Self {
            direct: None,
            marker: PhantomData,
        }
    }

    /// Forwards to `value` instead of allocating a new frozen value.
    ///
    /// The value must be equal to the source and produce the same hash, like
    /// any freeze result, and must be owned by a heap that outlives the
    /// freeze destination — typically a statically allocated value.
    ///
    /// FIXME(JakobDegen): We may want to make it possible to *only* freeze
    /// directly — a [`Freeze::prepare_freeze`] that always returns a
    /// direct plan, with no by-value `freeze` implementation for the type.
    pub fn direct(value: ValueTyped<'fv, T::Frozen<'fv>>) -> Self {
        Self {
            direct: Some(value),
            marker: PhantomData,
        }
    }
}

impl<'v, T> FreezeDynamic<'v> for T
where
    T: Freeze<'v>,
    for<'a> T::Frozen<'a>: AValueSimpleBound<'a>,
{
    type Plan<'fv> = StaticFreezePlan<'v, 'fv, T>;

    fn prepare_freeze<'fv>(&self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Plan<'fv>> {
        Freeze::prepare_freeze(self, freezer)
    }
}

impl<'v, 'fv, T> FreezePlan<'v, 'fv, T> for StaticFreezePlan<'v, 'fv, T>
where
    T: Freeze<'v>,
    for<'a> T::Frozen<'a>: AValueSimpleBound<'a>,
{
    fn target(&self) -> FreezeTarget<'fv> {
        match self.direct {
            Some(value) => FreezeTarget::direct(value.to_value()),
            None => FreezeTarget::simple::<T::Frozen<'fv>>(),
        }
    }

    fn freeze_into(
        self,
        value: T,
        freezer: &Freezer<'v, 'fv>,
        slot: FreezeSlot<'fv>,
    ) -> FreezeResult<Value<'fv>> {
        if self.direct.is_some() {
            // The freeze driver forwards to a direct target without reserving
            // a destination or calling this method, so this arm only rejects
            // a driver bug; `value` is dropped unfrozen.
            return Err(FreezeError::new(
                "a direct branded freeze plan must not be initialized".to_owned(),
            ));
        }
        slot.write(value.freeze(freezer)?)
    }
}

/// Freezing preserves the Starlark type: the frozen form of this representation is a
/// representation of the same [`Type`](StarlarkValue::Type), at the frozen heap's brand.
///
/// So a value keeps its type when it is frozen, whichever of the type's representations it
/// has, and a handle of the type ([`ValueOf`](crate::values::ValueOf)) is still one
/// afterwards. [`alloc_complex`](crate::values::Allocator::alloc_complex) requires this of
/// what it allocates. The one complex value allocated without it is a
/// [`StarlarkAnyComplex`](crate::values::any_complex::StarlarkAnyComplex) whose payload
/// freezes into another payload type.
///
/// A representation that names its frozen form with [`Freeze`] has this implemented for it,
/// and the compiler checks it there: the two `Type`s must have one
/// [`StaticType`](ProvidesStaticType::StaticType), which makes them one type at two brands. A
/// representation whose [`FreezeDynamic`] plan selects the frozen form at run time implements
/// this itself, as a promise about every target its plans select.
pub trait FreezePreservesType<'v>: StarlarkValue<'v> {}

impl<'v, T> FreezePreservesType<'v> for T
where
    T: Freeze<'v> + StarlarkValue<'v>,
    for<'fv> T::Frozen<'fv>: StarlarkValue<'fv>,
    for<'fv> <T::Frozen<'fv> as StarlarkValue<'fv>>::Type:
        ProvidesStaticType<'fv, StaticType = <T::Type as ProvidesStaticType<'v>>::StaticType>,
{
}

macro_rules! impl_freeze_identity {
    ($($t:ty),*) => {
        $(
            impl<'v> Freeze<'v> for $t {
                type Frozen<'fv> = Self;

                fn freeze<'fv>(self, _freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
                    Ok(self)
                }
            }
        )*
    }
}

impl_freeze_identity!(String, i32, u32, i64, u64, usize, bool, ());

impl<'v, T: 'static> Freeze<'v> for PhantomData<&'v T> {
    type Frozen<'fv> = PhantomData<&'fv T>;

    fn freeze<'fv>(self, _freezer: &Freezer<'v, 'fv>) -> FreezeResult<PhantomData<&'fv T>> {
        Ok(PhantomData)
    }
}

impl<'v, T> Freeze<'v> for Vec<T>
where
    T: Freeze<'v>,
{
    type Frozen<'fv> = Vec<T::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Vec<T::Frozen<'fv>>> {
        self.into_try_map(|v| v.freeze(freezer))
    }
}

impl<'v, T> Freeze<'v> for RefCell<T>
where
    T: Freeze<'v>,
{
    type Frozen<'fv> = T::Frozen<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<T::Frozen<'fv>> {
        self.into_inner().freeze(freezer)
    }
}

impl<'v, T> Freeze<'v> for UnsafeCell<T>
where
    T: Freeze<'v>,
{
    type Frozen<'fv> = UnsafeCell<T::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        Ok(UnsafeCell::new(self.into_inner().freeze(freezer)?))
    }
}

impl<'v, T> Freeze<'v> for OnceCell<T>
where
    T: Freeze<'v>,
{
    type Frozen<'fv> = Option<T::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        self.into_inner().freeze(freezer)
    }
}

impl<'v, T> Freeze<'v> for Box<T>
where
    T: Freeze<'v>,
{
    type Frozen<'fv> = Box<T::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        Ok(Box::new((*self).freeze(freezer)?))
    }
}

impl<'v, T> Freeze<'v> for Box<[T]>
where
    T: Freeze<'v>,
{
    type Frozen<'fv> = Box<[T::Frozen<'fv>]>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        self.into_vec()
            .into_try_map(|v| v.freeze(freezer))
            .map(|v| v.into_boxed_slice())
    }
}

impl<'v, T> Freeze<'v> for MiniVec<T>
where
    T: Freeze<'v>,
{
    type Frozen<'fv> = MiniVec<T::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        // Not `collect`: its `size_hint` through `Result` has a zero lower bound, which would
        // leave growth slack behind in the frozen heap.
        let mut frozen = MiniVec::with_capacity(self.len());
        for v in self {
            frozen.push(v.freeze(freezer)?);
        }
        Ok(frozen)
    }
}

impl<'v, T> Freeze<'v> for MiniBoxSlice<T>
where
    T: Freeze<'v>,
{
    type Frozen<'fv> = MiniBoxSlice<T::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        Ok(MiniBoxSlice::from(MiniVec::from(self).freeze(freezer)?))
    }
}

impl<'v, T> Freeze<'v> for Option<T>
where
    T: Freeze<'v>,
{
    type Frozen<'fv> = Option<T::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Option<T::Frozen<'fv>>> {
        self.map(|v| v.freeze(freezer)).transpose()
    }
}

impl<'v, K: Freeze<'v>> Freeze<'v> for Hashed<K> {
    type Frozen<'fv> = Hashed<K::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        // `freeze` must not change hash.
        Ok(Hashed::new_unchecked(
            self.hash(),
            self.into_key().freeze(freezer)?,
        ))
    }
}

impl<'v, K, V> Freeze<'v> for MiniMutInsertionMap<K, V>
where
    K: Freeze<'v>,
    V: Freeze<'v>,
{
    type Frozen<'fv> = MiniMutInsertionMap<K::Frozen<'fv>, V::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let mut new = MiniMutInsertionMap::with_capacity(self.len());
        for (key, value) in self.into_iter_hashed() {
            let hash = key.hash();
            let key = key.into_key().freeze(freezer)?;
            // TODO(nga): verify hash unchanged after freeze.
            let key = Hashed::new_unchecked(hash, key);
            let value = value.freeze(freezer)?;
            new.insert_hashed_unique_unchecked(key, value);
        }
        Ok(new)
    }
}

impl<'v, T> Freeze<'v> for MiniMutInsertionSet<T>
where
    T: Freeze<'v>,
{
    type Frozen<'fv> = MiniMutInsertionSet<T::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let mut new = MiniMutInsertionSet::with_capacity(self.len());
        for value in self.into_iter_hashed() {
            let hash = value.hash();
            let value = value.into_key().freeze(freezer)?;
            // TODO(nga): verify hash unchanged after freeze.
            new.insert_hashed_unique_unchecked(Hashed::new_unchecked(hash, value));
        }
        Ok(new)
    }
}

impl<'v, K, V> Freeze<'v> for SmallMap<K, V>
where
    K: Freeze<'v>,
    V: Freeze<'v>,
{
    type Frozen<'fv> = SmallMap<K::Frozen<'fv>, V::Frozen<'fv>>;

    fn freeze<'fv>(
        self,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<SmallMap<K::Frozen<'fv>, V::Frozen<'fv>>> {
        let mut new = SmallMap::with_capacity(self.len());
        for (key, value) in self.into_iter_hashed() {
            let hash = key.hash();
            let key = key.into_key().freeze(freezer)?;
            // TODO(nga): verify hash unchanged after freeze.
            let key = Hashed::new_unchecked(hash, key);
            let value = value.freeze(freezer)?;
            new.insert_hashed_unique_unchecked(key, value);
        }
        Ok(new)
    }
}

impl<'v, T> Freeze<'v> for SmallSet<T>
where
    T: Freeze<'v>,
{
    type Frozen<'fv> = SmallSet<T::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let mut new = SmallSet::with_capacity(self.len());
        for value in self.into_iter_hashed() {
            let value = value.freeze(freezer)?;
            // TODO(nga): verify hash unchanged after freeze.
            new.insert_hashed_unique_unchecked(value);
        }
        Ok(new)
    }
}

impl<'v, L: Freeze<'v>, R: Freeze<'v>> Freeze<'v> for Either<L, R> {
    type Frozen<'fv> = Either<L::Frozen<'fv>, R::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        match self {
            Either::Left(l) => Ok(Either::Left(l.freeze(freezer)?)),
            Either::Right(r) => Ok(Either::Right(r.freeze(freezer)?)),
        }
    }
}

impl<'v> Freeze<'v> for Value<'v> {
    type Frozen<'fv> = Value<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Value<'fv>> {
        freezer.freeze(self)
    }
}

impl<'v, A: Freeze<'v>> Freeze<'v> for (A,) {
    type Frozen<'fv> = (A::Frozen<'fv>,);

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<(A::Frozen<'fv>,)> {
        Ok((self.0.freeze(freezer)?,))
    }
}

impl<'v, A: Freeze<'v>, B: Freeze<'v>> Freeze<'v> for (A, B) {
    type Frozen<'fv> = (A::Frozen<'fv>, B::Frozen<'fv>);

    fn freeze<'fv>(
        self,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<(A::Frozen<'fv>, B::Frozen<'fv>)> {
        Ok((self.0.freeze(freezer)?, self.1.freeze(freezer)?))
    }
}

impl<'v, A: Freeze<'v>, B: Freeze<'v>, C: Freeze<'v>> Freeze<'v> for (A, B, C) {
    type Frozen<'fv> = (A::Frozen<'fv>, B::Frozen<'fv>, C::Frozen<'fv>);

    fn freeze<'fv>(
        self,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<(A::Frozen<'fv>, B::Frozen<'fv>, C::Frozen<'fv>)> {
        Ok((
            self.0.freeze(freezer)?,
            self.1.freeze(freezer)?,
            self.2.freeze(freezer)?,
        ))
    }
}

impl<'v, A: Freeze<'v>, B: Freeze<'v>, C: Freeze<'v>, D: Freeze<'v>> Freeze<'v> for (A, B, C, D) {
    type Frozen<'fv> = (
        A::Frozen<'fv>,
        B::Frozen<'fv>,
        C::Frozen<'fv>,
        D::Frozen<'fv>,
    );

    fn freeze<'fv>(
        self,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<(
        A::Frozen<'fv>,
        B::Frozen<'fv>,
        C::Frozen<'fv>,
        D::Frozen<'fv>,
    )> {
        Ok((
            self.0.freeze(freezer)?,
            self.1.freeze(freezer)?,
            self.2.freeze(freezer)?,
            self.3.freeze(freezer)?,
        ))
    }
}

impl<'v, A: Freeze<'v>, B: Freeze<'v>, C: Freeze<'v>, D: Freeze<'v>, E: Freeze<'v>> Freeze<'v>
    for (A, B, C, D, E)
{
    type Frozen<'fv> = (
        A::Frozen<'fv>,
        B::Frozen<'fv>,
        C::Frozen<'fv>,
        D::Frozen<'fv>,
        E::Frozen<'fv>,
    );

    fn freeze<'fv>(
        self,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<(
        A::Frozen<'fv>,
        B::Frozen<'fv>,
        C::Frozen<'fv>,
        D::Frozen<'fv>,
        E::Frozen<'fv>,
    )> {
        Ok((
            self.0.freeze(freezer)?,
            self.1.freeze(freezer)?,
            self.2.freeze(freezer)?,
            self.3.freeze(freezer)?,
            self.4.freeze(freezer)?,
        ))
    }
}
