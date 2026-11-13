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

//! This mod defines utilities to easily create Rust values as Starlark values.

use either::Either;

use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::type_expr::StarlarkTypeExpr;

/// A Rust value that converts into a Starlark [`Value`] on a [`Heap`]: what a native function
/// returns and what [`Heap::alloc`] takes.
///
/// This is a conversion, not a Starlark type: `char` implements it to make a `str`, `Vec<T>`
/// to make a `list`, `StarlarkInt` to pick between an
/// inline `int` and a big one. A representation of a Starlark type is allocated with
/// [`Heap::alloc_representation`] and returned from a native function as the
/// [`ValueOf`] or [`ValueTyped`](crate::values::ValueTyped) it was allocated as, so it needs
/// no impl here.
pub trait AllocValue<'v> {
    /// The Starlark type of the value this makes, which is what a native function returning
    /// `Self` is documented and type-checked as. A conversion that can make several types names
    /// their union (`Either<IntView, StarlarkFloat>`); one that can make anything names
    /// [`Value`].
    type Type: StarlarkTypeExpr<'v>;

    /// The value, on the heap, as the handle of [`Type`](Self::Type) it is by construction.
    ///
    /// Some values are not allocated: an empty string or `None` is a reference to a static.
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type>;
}

impl<'v> Heap<'v> {
    /// Allocate a new value on a [`Heap`]: the value, on the heap, as the handle of the
    /// builder's [`Type`](AllocValue::Type).
    pub fn alloc<T: AllocValue<'v>>(self, x: T) -> ValueOf<'v, T::Type> {
        x.alloc_value(self)
    }
}

impl<'v> AllocValue<'v> for Value<'v> {
    type Type = Value<'v>;

    fn alloc_value(self, _heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        self.into()
    }
}

impl<'v, A: AllocValue<'v>, B: AllocValue<'v>> AllocValue<'v> for Either<A, B> {
    type Type = Either<A::Type, B::Type>;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        match self {
            Either::Left(a) => ValueOf::left(a.alloc_value(heap)),
            Either::Right(b) => ValueOf::right(b.alloc_value(heap)),
        }
    }
}

impl<'fv, A: AllocFrozenValue<'fv>, B: AllocFrozenValue<'fv>> AllocFrozenValue<'fv>
    for Either<A, B>
{
    type Type = Either<A::Type, B::Type>;

    #[inline]
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        match self {
            Either::Left(a) => ValueOf::left(a.alloc_frozen_value(heap)),
            Either::Right(b) => ValueOf::right(b.alloc_frozen_value(heap)),
        }
    }
}

/// [`AllocValue`] for a [`FrozenHeap`]: a conversion into a frozen [`Value`]. A representation
/// is allocated with [`FrozenHeap::alloc_representation`] instead.
pub trait AllocFrozenValue<'fv> {
    /// The Starlark type of the value this makes, as [`AllocValue::Type`].
    type Type: StarlarkTypeExpr<'fv>;

    /// The value, on the frozen heap, as the handle of [`Type`](Self::Type) it is by
    /// construction.
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type>;
}

impl<'fh> FrozenHeap<'fh> {
    /// Allocate a new value on this heap: the value, on the heap, as the handle of the
    /// builder's [`Type`](AllocFrozenValue::Type).
    pub fn alloc<T: AllocFrozenValue<'fh>>(self, val: T) -> ValueOf<'fh, T::Type> {
        val.alloc_frozen_value(self)
    }
}

impl<'fv> AllocFrozenValue<'fv> for Value<'fv> {
    type Type = Value<'fv>;

    fn alloc_frozen_value(self, _heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        self.into()
    }
}
