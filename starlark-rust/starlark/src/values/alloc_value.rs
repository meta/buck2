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
use crate::values::StringValue;
use crate::values::Value;
use crate::values::type_repr::StarlarkTypeRepr;

/// A Rust value that converts into a Starlark [`Value`] on a [`Heap`]: what a native function
/// returns and what [`Heap::alloc`] takes.
///
/// This is a conversion, not a Starlark type: `char` implements it to make a `str`, `Vec<T>`
/// to make a `list`, `StarlarkInt` to pick between an
/// inline `int` and a big one. A representation of a Starlark type is allocated with
/// [`Heap::alloc_representation`] and returned from a native function as the
/// [`ValueOf`](crate::values::ValueOf) or [`ValueTyped`](crate::values::ValueTyped) it was
/// allocated as, so it needs no impl here.
pub trait AllocValue<'v>: StarlarkTypeRepr {
    /// Allocate the value on a heap and return a reference to the allocated value.
    ///
    /// Note, for certain values (e.g. empty strings) no allocation is actually performed,
    /// and a reference to the statically allocated object is returned.
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v>;
}

/// Type which allocates a string.
pub trait AllocStringValue<'v>: AllocValue<'v> + Sized {
    /// Allocate a string.
    fn alloc_string_value(self, heap: Heap<'v>) -> StringValue<'v>;
}

impl<'v> AllocValue<'v> for Value<'v> {
    fn alloc_value(self, _heap: Heap<'v>) -> Value<'v> {
        self
    }
}

impl<'v, A: AllocValue<'v>, B: AllocValue<'v>> AllocValue<'v> for Either<A, B> {
    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        match self {
            Either::Left(a) => a.alloc_value(heap),
            Either::Right(b) => b.alloc_value(heap),
        }
    }
}

impl<'fv, A: AllocFrozenValue<'fv>, B: AllocFrozenValue<'fv>> AllocFrozenValue<'fv>
    for Either<A, B>
{
    #[inline]
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv> {
        match self {
            Either::Left(a) => a.alloc_frozen_value(heap),
            Either::Right(b) => b.alloc_frozen_value(heap),
        }
    }
}

/// [`AllocValue`] for a [`FrozenHeap`]: a conversion into a frozen [`Value`]. A representation
/// is allocated with [`FrozenHeap::alloc_representation`] instead.
pub trait AllocFrozenValue<'fv>: StarlarkTypeRepr {
    /// Allocate a value in the frozen heap and return a reference to the allocated value.
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv>;
}

/// Type which allocates a string.
pub trait AllocFrozenStringValue<'fv>: AllocFrozenValue<'fv> + Sized {
    /// Allocate a string.
    fn alloc_frozen_string_value(self, heap: FrozenHeap<'fv>) -> StringValue<'fv>;
}

impl<'fv> AllocFrozenValue<'fv> for Value<'fv> {
    fn alloc_frozen_value(self, _heap: FrozenHeap<'fv>) -> Value<'fv> {
        self
    }
}
