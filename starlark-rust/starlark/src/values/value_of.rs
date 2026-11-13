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

//! `ValueOf`: a value known to be of a Starlark type.

use std::convert::Infallible;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::marker::PhantomData;
use std::ops::Deref;

use allocative::Allocative;
use dupe::Clone_;
use dupe::Copy_;
use dupe::Dupe_;
use serde::Serialize;
use starlark_derive::StarlarkPagable;

use crate as starlark;
use crate::any::IsStaticType;
use crate::any::ProvidesStaticType;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::Freeze;
use crate::values::FreezeError;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::Trace;
use crate::values::Tracer;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;

/// A [`Value`] known to be of the type `T`: it was checked when it was constructed.
///
/// `T` is a [`StarlarkTypeExpr`], spelled as the type is: `Foo` for a nominal type, its view
/// for a type with several representations, [`ListType<T>`](crate::values::list::ListType)
/// for `list[T]`, [`NoneOr<T>`](crate::values::none::NoneOr) for `None | T`, a
/// `#[derive(UnpackValue)]` enum for a union. A `ValueOf<T>` is an `UnpackValue` and an
/// `AllocValue`, so it is both a parameter binding and a return type; and it is the field type
/// for a Starlark value that Rust code holds on to, because it freezes to a `ValueOf` at the
/// frozen brand after checking that the value still is a `T` (a mutable container may have been
/// changed since it was checked).
///
/// The point is that code which holds a `ValueOf<T>` neither re-checks on every read nor
/// asserts a type it already checked. It is read through [`get`](Self::get), which gives
/// `T`'s handle ([`StarlarkTypeExpr::Get`]) and cannot fail when the check cannot be
/// invalidated ([`StarlarkTypeExprStable`]); where it can, because `T` looks into a mutable
/// container, [`try_get`](Self::try_get) and the container handles' accessors re-check and
/// report an error, which a frozen value never produces. A single-representation nominal `T`
/// also derefs to the `T`.
#[derive(Clone_, Copy_, Dupe_, Allocative)]
#[allocative(bound = "")]
#[derive(pagable::PagablePanic, StarlarkPagable)]
#[repr(transparent)]
pub struct ValueOf<'v, T: StarlarkTypeExpr<'v>>(Value<'v>, PhantomData<fn() -> T>);

impl<'v, T: StarlarkTypeExpr<'v>> ValueOf<'v, T> {
    /// `value` if it is a `T`.
    #[inline]
    pub fn new(value: Value<'v>) -> Option<Self> {
        let matches = T::matches(value);
        #[cfg(debug_assertions)]
        crate::values::unpack::check_matches_agreement::<T>(value, matches);
        matches.then(|| Self::new_unchecked(value))
    }

    /// `value` and what it is read as, if it is a `T`: [`new`](Self::new) and
    /// [`get`](Self::get) in one read.
    #[inline]
    pub fn new_with_get(value: Value<'v>) -> Option<(Self, T::Get)> {
        let get = T::get(value);
        #[cfg(debug_assertions)]
        crate::values::unpack::check_matches_agreement::<T>(value, get.is_some());
        get.map(|get| (Self::new_unchecked(value), get))
    }

    /// `value` if it is a `T`, otherwise an error saying what it is instead.
    #[inline]
    pub fn new_err(value: Value<'v>) -> crate::Result<Self> {
        Self::new(value)
            .ok_or_else(|| crate::values::unpack::expected_type_error(value, T::starlark_type_repr))
    }

    /// `value`, which the caller knows to be a `T`: it was just checked, or just built from
    /// checked parts. Prefer [`alloc`](Self::alloc) and the typed constructors where they apply;
    /// a wrong claim here is caught by [`get`](Self::get) or by freezing, not by the compiler.
    #[inline]
    pub fn new_unchecked(value: Value<'v>) -> Self {
        ValueOf(value, PhantomData)
    }

    /// A slice of values the caller knows to be `T`s, as [`new_unchecked`](Self::new_unchecked)
    /// knows one, without copying.
    #[inline]
    pub fn slice_from_values_unchecked<'a>(values: &'a [Value<'v>]) -> &'a [Self] {
        // SAFETY: `#[repr(transparent)]` over `Value<'v>` at the same lifetime.
        unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), values.len()) }
    }

    /// The value.
    #[inline]
    pub fn to_value(self) -> Value<'v> {
        self.0
    }

    /// The handle this value is read through, for a `T` whose check cannot be invalidated
    /// ([`StarlarkTypeExprStable`]).
    #[inline]
    pub fn get(self) -> T::Get
    where
        T: StarlarkTypeExprStable<'v>,
    {
        match T::get(self.0) {
            Some(x) => x,
            None => unreachable!(
                "`{}` is `StarlarkTypeExprStable`, yet value `{}` stopped being one",
                T::starlark_type_repr(),
                self.0,
            ),
        }
    }

    /// The handle this value is read through, re-checked: for a `T` that looks into a mutable
    /// container, the container may have been changed since the check, and this is where that
    /// surfaces. Freezing re-checks too, so a frozen `ValueOf` always gives `Ok`.
    #[inline]
    pub fn try_get(self) -> crate::Result<T::Get> {
        T::get(self.0)
            .ok_or_else(|| crate::values::unpack::changed_since_checked_error::<T>(self.0))
    }
}

impl<'v, T: StarlarkType<'v>> ValueOf<'v, T> {
    /// A value of `T` made from `builder`: a representation of `T`, or anything else that
    /// makes one. The value is a `T` by construction.
    #[inline]
    pub fn alloc(heap: Heap<'v>, builder: impl AllocValue<'v, Type = T>) -> Self {
        heap.alloc(builder)
    }

    /// [`alloc`](Self::alloc) on a frozen heap.
    #[inline]
    pub fn alloc_frozen(
        heap: FrozenHeap<'v>,
        builder: impl AllocFrozenValue<'v, Type = T>,
    ) -> Self {
        heap.alloc(builder)
    }

    /// A typed value of a representation of `T`, which is a `T` by construction as for
    /// [`alloc`](Self::alloc).
    #[inline]
    pub fn from_representation<R>(value: ValueTyped<'v, R>) -> Self
    where
        R: StarlarkValue<'v, Type = T>,
    {
        Self::new_unchecked(value.to_value())
    }
}

impl<'v, T: StarlarkValue<'v> + StarlarkType<'v, Get = ValueTyped<'v, T>>> ValueOf<'v, T> {
    /// The value's representation, for a type that has exactly one: the handle to such a type
    /// is the handle to its representation, and derefs to it.
    #[inline]
    pub fn as_ref(self) -> &'v T {
        self.get().as_ref()
    }
}

impl<'v, T: StarlarkValue<'v> + StarlarkType<'v, Get = ValueTyped<'v, T>>> Deref
    for ValueOf<'v, T>
{
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.as_ref()
    }
}

/// Every value is a `typing.Any`.
impl<'v> From<Value<'v>> for ValueOf<'v, Value<'v>> {
    #[inline]
    fn from(value: Value<'v>) -> Self {
        Self::new_unchecked(value)
    }
}

/// A typed value of a single-representation type is a handle to the type by construction.
impl<'v, T: StarlarkValue<'v> + StarlarkType<'v, Get = ValueTyped<'v, T>>> From<ValueTyped<'v, T>>
    for ValueOf<'v, T>
{
    #[inline]
    fn from(value: ValueTyped<'v, T>) -> Self {
        Self::new_unchecked(value.to_value())
    }
}

// SAFETY: the static type is the same type with the heap lifetime erased.
unsafe impl<'v, T> ProvidesStaticType<'v> for ValueOf<'v, T>
where
    T: StarlarkTypeExpr<'v> + ProvidesStaticType<'v>,
    T::StaticType: StarlarkTypeExpr<'static>,
{
    type StaticType = ValueOf<'static, T::StaticType>;
}

impl<T> IsStaticType for ValueOf<'static, T>
where
    T: StarlarkTypeExpr<'static> + IsStaticType,
    for<'lt> T::Reinfect<'lt>: StarlarkTypeExpr<'lt>,
{
    type Reinfect<'lt> = ValueOf<'lt, T::Reinfect<'lt>>;
}

impl<'v, T: StarlarkTypeExpr<'v>> PartialEq for ValueOf<'v, T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> Eq for ValueOf<'v, T> {}

impl<'v, T: StarlarkTypeExpr<'v>> Debug for ValueOf<'v, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ValueOf").field(&self.0).finish()
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> Display for ValueOf<'v, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> Serialize for ValueOf<'v, T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> UnpackValue<'v> for ValueOf<'v, T> {
    type Type = T;
    type Error = Infallible;

    #[inline]
    fn unpack(value: ValueOf<'v, T>, _get: T::Get) -> Result<Self, Infallible> {
        Ok(value)
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> AllocValue<'v> for ValueOf<'v, T> {
    type Type = T;

    #[inline]
    fn alloc_value(self, _heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        self
    }
}

impl<'fv, T: StarlarkTypeExpr<'fv>> AllocFrozenValue<'fv> for ValueOf<'fv, T> {
    type Type = T;

    #[inline]
    fn alloc_frozen_value(self, _heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        self
    }
}

unsafe impl<'v, T: StarlarkTypeExpr<'v>> Trace<'v> for ValueOf<'v, T> {
    fn trace(&mut self, tracer: &Tracer<'v>) {
        self.0.trace(tracer)
    }
}

/// Freezing re-checks the type: the value may be a mutable container that was changed after
/// it was checked, and this is the last chance to say so.
impl<'v, T> Freeze<'v> for ValueOf<'v, T>
where
    T: StarlarkTypeExpr<'v> + ProvidesStaticType<'v>,
    T::StaticType: IsStaticType,
    for<'fv> <T::StaticType as IsStaticType>::Reinfect<'fv>: StarlarkTypeExpr<'fv> + Sized,
{
    type Frozen<'fv> = ValueOf<'fv, <T::StaticType as IsStaticType>::Reinfect<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let value = self.0.freeze(freezer)?;
        ValueOf::new(value).ok_or_else(|| {
            FreezeError::new(format!(
                "Value `{value}` of type `{}` was changed after it was checked: it is no longer a `{}`",
                value.get_type(),
                T::starlark_type_repr(),
            ))
        })
    }
}
