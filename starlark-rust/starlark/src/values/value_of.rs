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

use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::marker::PhantomData;

use allocative::Allocative;
use dupe::Clone_;
use dupe::Copy_;
use dupe::Dupe_;
use starlark_derive::StarlarkPagable;

use crate as starlark;
use crate::any::IsStaticType;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::Freeze;
use crate::values::FreezeError;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::StarlarkValue;
use crate::values::Trace;
use crate::values::Tracer;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOfUnchecked;
use crate::values::ValueTyped;
use crate::values::type_repr::StarlarkTypeRepr;

/// A [`Value`] that `T` accepts: it was type-checked when it was constructed.
///
/// `T` is any [`UnpackValue`], spelled as a parameter of that type would be: a view or a
/// [`ValueTyped`](crate::values::ValueTyped) for a nominal type, [`ListType<T>`](crate::values::list::ListType) for `list[T]`,
/// [`NoneOr<T>`](crate::values::none::NoneOr) for `None | T`, a `#[derive(UnpackValue)]` enum
/// for a union. A `ValueOf<T>` is an `UnpackValue` (the check is `T`'s) and an `AllocValue`
/// (the value), so it is both a parameter binding and a return type; and it is the field type
/// for a Starlark value that Rust code holds on to, because it freezes to a `ValueOf` at the
/// frozen brand after checking that the value still is a `T` (a mutable container may have
/// been changed since it was checked).
///
/// The point is that code which holds a `ValueOf<T>` neither unpacks again on every read nor
/// asserts a type it already checked: [`unpack`](Self::unpack) is the one place that relies on
/// the check having been done.
#[derive(Clone_, Copy_, Dupe_, Allocative)]
#[allocative(bound = "")]
#[derive(pagable::PagablePanic, StarlarkPagable)]
pub struct ValueOf<'v, T: UnpackValue<'v>>(Value<'v>, PhantomData<fn() -> T>);

impl<'v, T: UnpackValue<'v>> ValueOf<'v, T> {
    /// `value` if it is a `T`; `None` if it is not (or if it is but does not convert to a `T`,
    /// which [`new_err`](Self::new_err) distinguishes).
    #[inline]
    pub fn new(value: Value<'v>) -> Option<Self> {
        match T::unpack_value(value) {
            Ok(Some(_)) => Some(Self::new_unchecked(value)),
            Ok(None) | Err(_) => None,
        }
    }

    /// `value` if it is a `T`, otherwise the error that unpacking a `T` would give.
    #[inline]
    pub fn new_err(value: Value<'v>) -> crate::Result<Self> {
        T::unpack_value_err(value)?;
        Ok(Self::new_unchecked(value))
    }

    /// `value`, which the caller knows to be a `T`: it was just checked, or just built from
    /// checked parts. Prefer [`alloc`](Self::alloc) and the typed constructors where they apply;
    /// a wrong claim here is caught by [`unpack`](Self::unpack) or by freezing, not by the
    /// compiler.
    #[inline]
    pub fn new_unchecked(value: Value<'v>) -> Self {
        ValueOf(value, PhantomData)
    }

    /// The value.
    #[inline]
    pub fn to_value(self) -> Value<'v> {
        self.0
    }

    /// The `T` this value is.
    ///
    /// Panics if the value is no longer a `T`, which only a mutable container whose contents
    /// were changed after the check can be; freezing re-checks, so a frozen `ValueOf` never
    /// panics here. Where that window matters, use [`try_unpack`](Self::try_unpack).
    #[inline]
    pub fn unpack(self) -> T {
        match T::unpack_value_err(self.0) {
            Ok(x) => x,
            Err(e) => panic!(
                "Value `{}` of type `{}` is no longer a `{}`: it was changed after it was checked: {e}",
                self.0,
                self.0.get_type(),
                T::starlark_type_repr(),
            ),
        }
    }

    /// The `T` this value is, as an error rather than a panic when a mutable container was
    /// changed after the check.
    #[inline]
    pub fn try_unpack(self) -> crate::Result<T> {
        T::unpack_value_err(self.0)
    }

    /// Transitional: as the unchecked handle, for code not yet migrated to `ValueOf`.
    #[inline]
    pub fn as_unchecked(self) -> ValueOfUnchecked<'v, T> {
        ValueOfUnchecked::new(self.0)
    }
}

impl<'v, T: UnpackValue<'v>> ValueOf<'v, T> {
    /// A freshly allocated representation of `T`, where `T` is the nominal type the
    /// representation belongs to: the value is a `T` by construction.
    #[inline]
    pub fn alloc<R>(heap: Heap<'v>, representation: R) -> Self
    where
        R: StarlarkValue<'v, Type = T> + AllocValue<'v>,
    {
        Self::new_unchecked(heap.alloc(representation))
    }
}

/// A typed value is a `ValueOf` its own binding by construction.
impl<'v, T: StarlarkValue<'v>> From<ValueTyped<'v, T>> for ValueOf<'v, ValueTyped<'v, T>> {
    #[inline]
    fn from(value: ValueTyped<'v, T>) -> Self {
        Self::new_unchecked(value.to_value())
    }
}

impl<'v, T: UnpackValue<'v>> Debug for ValueOf<'v, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ValueOf").field(&self.0).finish()
    }
}

impl<'v, T: UnpackValue<'v>> Display for ValueOf<'v, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl<'v, T: UnpackValue<'v>> StarlarkTypeRepr for ValueOf<'v, T> {
    type Canonical = T::Canonical;

    fn starlark_type_repr() -> Ty {
        T::starlark_type_repr()
    }
}

impl<'v, T: UnpackValue<'v>> UnpackValue<'v> for ValueOf<'v, T> {
    type Error = T::Error;

    const UNCHECKED: bool = T::UNCHECKED;

    #[inline]
    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        Ok(T::unpack_value_impl(value)?.map(|_| Self::new_unchecked(value)))
    }
}

impl<'v, T: UnpackValue<'v>> AllocValue<'v> for ValueOf<'v, T> {
    #[inline]
    fn alloc_value(self, _heap: Heap<'v>) -> Value<'v> {
        self.0
    }
}

impl<'fv, T: UnpackValue<'fv>> AllocFrozenValue<'fv> for ValueOf<'fv, T> {
    #[inline]
    fn alloc_frozen_value(self, _heap: FrozenHeap<'fv>) -> Value<'fv> {
        self.0
    }
}

unsafe impl<'v, T: UnpackValue<'v>> Trace<'v> for ValueOf<'v, T> {
    fn trace(&mut self, tracer: &Tracer<'v>) {
        self.0.trace(tracer)
    }
}

/// Freezing re-checks the type: the value may be a mutable container that was changed after
/// it was checked, and this is the last chance to say so.
impl<'v, T> Freeze<'v> for ValueOf<'v, T>
where
    T: UnpackValue<'v> + ProvidesStaticType<'v>,
    T::StaticType: IsStaticType,
    for<'fv> <T::StaticType as IsStaticType>::Reinfect<'fv>: UnpackValue<'fv>,
{
    type Frozen<'fv> = ValueOf<'fv, <T::StaticType as IsStaticType>::Reinfect<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let value = self.0.freeze(freezer)?;
        match <<T::StaticType as IsStaticType>::Reinfect<'fv>>::unpack_value(value) {
            Ok(Some(_)) => Ok(ValueOf::new_unchecked(value)),
            Ok(None) => Err(FreezeError::new(format!(
                "Value `{value}` of type `{}` was changed after it was checked: it is no longer a `{}`",
                value.get_type(),
                T::starlark_type_repr(),
            ))),
            Err(e) => Err(FreezeError::new(format!(
                "Value `{value}` of type `{}` was changed after it was checked: it is no longer a `{}`: {e}",
                value.get_type(),
                T::starlark_type_repr(),
            ))),
        }
    }
}
