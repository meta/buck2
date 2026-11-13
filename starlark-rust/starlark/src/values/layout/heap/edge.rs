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

use std::marker::PhantomData;

use dupe::Dupe;

use crate::any::IsStaticType;
use crate::any::ProvidesStaticType;
use crate::values::Value;
use crate::values::layout::heap::branding::rebrand_ref_unchecked;
use crate::values::layout::heap::branding::rebrand_unchecked;

/// Witness that the heap identified by `'v` depends on the heap identified by `'dep`.
///
/// As described in the `branding` module, a `Value<'v>` may live either in the heap identified
/// by `'v` or in any frozen heap that that heap depends on. This type is a certificate of such a
/// dependency: it proves that the heap of `'dep` is kept alive by the heap of `'v`, so that
/// anything kept alive by the former is usable in the context of the latter. That conversion is
/// what [`rebrand`](HeapEdge::rebrand) provides, for compound types too: a
/// `ValueTyped<'dep, Tuple<'dep>>` becomes a `ValueTyped<'v, Tuple<'v>>`. Edges are `Copy` and
/// zero sized.
#[derive(Copy, Clone, Dupe)]
pub struct HeapEdge<'v, 'dep> {
    _invariant: PhantomData<(fn(&'v ()) -> &'v (), fn(&'dep ()) -> &'dep ())>,
}

impl<'v, 'dep> HeapEdge<'v, 'dep> {
    /// Assert the existence of this heap dependency.
    ///
    /// # SAFETY
    ///
    /// The heap identified by `'v` must keep the heap identified by `'dep` alive. Additionally,
    /// `'dep` must be a brand: a lifetime at which only values kept alive by that heap (or
    /// `'static` data) can exist. An ordinary borrow lifetime is not a brand — stack data can be
    /// borrowed at it, and [`rebrand`](HeapEdge::rebrand) would extend such a borrow past its
    /// referent.
    pub(crate) unsafe fn unchecked_new() -> Self {
        Self {
            _invariant: PhantomData,
        }
    }

    /// Convert a value kept alive by the `'dep` heap for use in the context of the `'v` heap.
    pub fn rebrand<U>(self, v: U) -> <U::StaticType as IsStaticType>::Reinfect<'v>
    where
        U: ProvidesStaticType<'dep>,
        U::StaticType: IsStaticType + Sized,
        <U::StaticType as IsStaticType>::Reinfect<'v>: Sized,
    {
        // SAFETY: Everything at the `'dep` brand is kept alive by the `'dep` heap, which the `'v`
        // heap keeps alive by the construction contract of `self`.
        unsafe { rebrand_unchecked::<U::StaticType, U, _>(v) }
    }

    /// [`rebrand`](HeapEdge::rebrand), behind a reference.
    pub fn rebrand_ref<'a, U>(self, v: &'a U) -> &'a <U::StaticType as IsStaticType>::Reinfect<'v>
    where
        U: ProvidesStaticType<'dep>,
        U::StaticType: IsStaticType + Sized,
        <U::StaticType as IsStaticType>::Reinfect<'v>: Sized,
    {
        // SAFETY: As for `rebrand`.
        unsafe { rebrand_ref_unchecked::<U::StaticType, U, _>(v) }
    }
}

impl<'v> HeapEdge<'v, 'static> {
    /// The dependency of every heap on the `'static` brand.
    ///
    /// The only data at the `'static` brand is immortal: statics ([`AllocStaticSimple`],
    /// [`const_frozen_string!`]) and the [`Methods`] tables reached through `&'static Methods`.
    /// Nothing keeps it alive because nothing needs to, and the frozen bit already makes the
    /// garbage collector skip it, so every heap trivially depends on it and the edge can be minted
    /// anywhere.
    ///
    /// Such data is brought to the brand of the heap in use by spelling `at()`
    /// ([`AllocStaticSimple::at`], [`ValueTyped::at`], [`ValueOf::at`]), which is this edge's
    /// [`rebrand`](HeapEdge::rebrand) behind a name.
    ///
    /// [`AllocStaticSimple`]: crate::values::AllocStaticSimple
    /// [`AllocStaticSimple::at`]: crate::values::AllocStaticSimple::at
    /// [`ValueTyped::at`]: crate::values::ValueTyped::at
    /// [`ValueOf::at`]: crate::values::ValueOf::at
    /// [`const_frozen_string!`]: crate::const_frozen_string
    /// [`Methods`]: crate::environment::Methods
    pub fn immortal() -> Self {
        // SAFETY: `'static` is a brand: no stack data can be borrowed at it, so a `'static`-branded
        // value can only be immortal data, which every heap keeps alive by virtue of it never
        // being freed. The `branding` module states where that is guaranteed.
        unsafe { Self::unchecked_new() }
    }
}

impl<'v> HeapEdge<'v, 'v> {
    /// The dependency of every heap on itself.
    ///
    /// For code that is written against an edge between two brands and is handed one heap for
    /// both.
    pub fn identity() -> Self {
        // SAFETY: A heap keeps itself alive, and `'v` is a brand by the same assumption every use
        // of it makes; with both sides equal, `rebrand` changes no type.
        unsafe { Self::unchecked_new() }
    }
}

/// Witness that the frozen heap identified by `'fm` keeps alive every heap in which a frozen
/// value at `'v` can live: `'fm` is the frozen heap that the module value heap `'v` seals into.
///
/// A [`HeapEdge<'v, 'dep>`] lets anything at `'dep` be used at `'v`. This is the other direction
/// between a module's own two heaps, for frozen values only: [`rebrand`](SealEdge::rebrand)
/// brings a frozen `Value<'v>` to `'fm`, which is how the optimizer folds a value it observed at
/// `'v` into IR it allocates at `'fm`, and how the [`Freezer`](crate::values::Freezer) hands
/// back a value that is frozen already. Unfrozen values live in the value heap and cannot cross.
///
/// `ModuleHeaps` is the only minter, and the proof that the property holds is written there; the
/// `branding` module states it too. (Not to be confused with
/// [`OwnedFrozenReconstructor::frozen_edge`](crate::values::OwnedFrozenReconstructor::frozen_edge),
/// which is a `HeapEdge` whose dependent heap is a frozen heap.)
#[derive(Copy, Clone, Dupe)]
pub(crate) struct SealEdge<'fm, 'v> {
    _invariant: PhantomData<(fn(&'fm ()) -> &'fm (), fn(&'v ()) -> &'v ())>,
}

impl<'fm, 'v> SealEdge<'fm, 'v> {
    /// Assert the existence of this property.
    ///
    /// # SAFETY
    ///
    /// The heap identified by `'fm` must reference, directly or through its references, every
    /// frozen heap in which a frozen `Value<'v>` can live, for as long as the edge exists. Both
    /// lifetimes must be brands, see [`HeapEdge::unchecked_new`].
    pub(in crate::values::layout::heap) unsafe fn unchecked_new() -> Self {
        Self {
            _invariant: PhantomData,
        }
    }

    /// The frozen value `v` at `'fm`, or `None` if `v` is not frozen.
    pub(crate) fn rebrand(self, v: Value<'v>) -> Option<Value<'fm>> {
        if !v.is_frozen() {
            return None;
        }
        // SAFETY: `v` is frozen, and by the construction contract of `self` the heap of `'fm`
        // keeps alive whichever frozen heap it lives in.
        Some(unsafe { v.rebrand_frozen_unchecked() })
    }
}

#[cfg(test)]
mod tests {
    use crate::const_frozen_string;
    use crate::values::AllocStaticSimple;
    use crate::values::Heap;
    use crate::values::StringValue;
    use crate::values::Value;
    use crate::values::ValueTyped;
    use crate::values::layout::heap::edge::HeapEdge;
    use crate::values::list::AllocList;
    use crate::values::list::ListRef;
    use crate::values::none::NoneType;
    use crate::values::tuple::AllocTuple;
    use crate::values::tuple::value::Tuple;
    use crate::values::tuple::value::VALUE_EMPTY_TUPLE;
    use crate::values::types::none::none_type::VALUE_NONE;

    /// Rebrands a static for the given heap; the heap only serves to name the brand.
    fn at_brand<'v>(_heap: Heap<'v>, v: ValueTyped<'static, NoneType>) -> Value<'v> {
        HeapEdge::immortal().rebrand(v).to_value()
    }

    #[test]
    fn test_immortal_edge_at_nested_brands() {
        let none: &'static AllocStaticSimple<NoneType> = &VALUE_NONE;
        let none: ValueTyped<'static, NoneType> = none.at();
        Heap::temp(|outer| {
            let v1 = at_brand(outer, none);
            let list1 = outer.alloc(AllocList([v1])).to_value();
            Heap::temp(|inner| {
                let v2 = at_brand(inner, none);
                let list2 = inner.alloc(AllocList([v2])).to_value();
                assert!(v1.ptr_eq(none.to_value()));
                assert!(v2.ptr_eq(none.to_value()));
                assert!(ListRef::from_value(list1).unwrap().content()[0].is_none());
                assert!(ListRef::from_value(list2).unwrap().content()[0].is_none());
            });
        });
    }

    /// `at()` carries the type to the brand along with the value.
    #[test]
    fn test_at_follows_the_brand() {
        fn check<'v>(heap: Heap<'v>) {
            let tuple: ValueTyped<'v, Tuple<'v>> = VALUE_EMPTY_TUPLE.at();
            assert_eq!(0, tuple.as_ref().len());
            let name: StringValue<'v> = const_frozen_string!("static string").at();
            assert_eq!("static string", name.as_str());
            let both = heap.alloc(AllocTuple([tuple.to_value(), name.to_value()]));
            assert_eq!("((), \"static string\")", both.to_string());
        }
        Heap::temp(check);
    }
}
