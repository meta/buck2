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

//! A representation's own way onto a heap.

use dupe::Dupe;

use crate::private::Private;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::StarlarkValue;
use crate::values::Trace;
use crate::values::ValueTyped;
use crate::values::layout::avalue::AValueImpl;
use crate::values::layout::avalue::AValueSimpleBound;
use crate::values::layout::avalues::complex::AValueComplex;
use crate::values::layout::avalues::complex_no_freeze::AValueComplexNoFreeze;
use crate::values::layout::avalues::simple::AValueFrozen;
use crate::values::layout::avalues::simple::AValueSimple;
use crate::values::layout::heap::send::HeapSendable;
use crate::values::traits::ComplexValue;

/// How a representation goes onto a [`Heap`]: what it is built from, and the allocation it
/// chose. A representation allocated by value is built from itself; one laid out inline
/// (records, tuples, strings) is built from a recipe.
///
/// `#[starlark_value]` writes the impl for a by-value representation from the shape of the
/// type and its flags; an inline representation writes its own. Nothing here allocates
/// implicitly: a Rust value that is not a representation reaches the heap through
/// [`AllocValue`](crate::values::AllocValue), whose impls are written by hand.
pub trait AllocRepresentation<'v>: Sized {
    /// The representation this builds.
    type Repr: StarlarkValue<'v>;

    /// Allocate, with the strategy the representation chose.
    fn alloc(self, allocator: Allocator<'v>) -> ValueTyped<'v, Self::Repr>;
}

/// [`AllocRepresentation`] for a [`FrozenHeap`]. Implemented only for a representation that
/// can live on one, so a value that cannot be frozen cannot be put there at all.
pub trait AllocFrozenRepresentation<'fv>: Sized {
    /// The representation this builds.
    type Repr: StarlarkValue<'fv>;

    /// Allocate on the frozen heap.
    fn alloc_frozen(self, allocator: FrozenAllocator<'fv>) -> ValueTyped<'fv, Self::Repr>;
}

/// The allocation strategies of a [`Heap`], handed to exactly one
/// [`AllocRepresentation::alloc`] by [`Heap::alloc_representation`].
#[derive(Copy, Clone, Dupe)]
pub struct Allocator<'v>(Heap<'v>);

impl<'v> Allocator<'v> {
    /// The heap, for the values a representation holds.
    #[inline]
    pub fn heap(self) -> Heap<'v> {
        self.0
    }

    /// A `'static` representation: it holds no `Value`, so it is neither traced nor frozen,
    /// and freezing the module moves it to the frozen heap as it is.
    pub fn alloc_simple<T: for<'a> AValueSimpleBound<'a>>(self, x: T) -> ValueTyped<'v, T> {
        assert!(!T::is_special(Private));
        self.0.alloc_raw(AValueImpl::<AValueSimple<T>>::new(x))
    }

    /// A representation that holds `Value`s: traced, and frozen by its
    /// [`Freeze`](crate::values::Freeze) impl.
    pub fn alloc_complex<T>(self, x: T) -> ValueTyped<'v, T>
    where
        T: ComplexValue<'v> + HeapSendable<'v>,
    {
        assert!(!T::is_special(Private));
        self.0.alloc_raw(AValueImpl::<AValueComplex<T>>::new(x))
    }

    /// A representation that holds `Value`s and cannot be frozen: traced, and an error when
    /// the module that holds it freezes.
    pub fn alloc_complex_no_freeze<T>(self, x: T) -> ValueTyped<'v, T>
    where
        T: StarlarkValue<'v> + Trace<'v> + HeapSendable<'v>,
    {
        assert!(!T::is_special(Private));
        self.0
            .alloc_raw(AValueImpl::<AValueComplexNoFreeze<T>>::new(x))
    }
}

/// The allocation strategies of a [`FrozenHeap`], handed to exactly one
/// [`AllocFrozenRepresentation::alloc_frozen`] by [`FrozenHeap::alloc_representation`].
#[derive(Copy, Clone, Dupe)]
pub struct FrozenAllocator<'fv>(FrozenHeap<'fv>);

impl<'fv> FrozenAllocator<'fv> {
    /// The heap, for the values a representation holds.
    #[inline]
    pub fn heap(self) -> FrozenHeap<'fv> {
        self.0
    }

    /// A representation at the frozen brand: nothing on a frozen heap is traced or frozen.
    pub fn alloc_simple<T: AValueSimpleBound<'fv>>(self, x: T) -> ValueTyped<'fv, T> {
        assert!(!T::is_special(Private));
        self.0.alloc_raw(AValueImpl::<AValueFrozen<T>>::new(x))
    }
}

impl<'v> Heap<'v> {
    /// Allocate a representation the way it chose to be allocated.
    #[inline]
    pub fn alloc_representation<B: AllocRepresentation<'v>>(
        self,
        builder: B,
    ) -> ValueTyped<'v, B::Repr> {
        builder.alloc(Allocator(self))
    }
}

impl<'fv> FrozenHeap<'fv> {
    /// Allocate a representation the way it chose to be allocated.
    #[inline]
    pub fn alloc_representation<B: AllocFrozenRepresentation<'fv>>(
        self,
        builder: B,
    ) -> ValueTyped<'fv, B::Repr> {
        builder.alloc_frozen(FrozenAllocator(self))
    }
}
