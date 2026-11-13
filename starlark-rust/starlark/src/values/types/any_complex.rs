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

//! A type [`StarlarkAnyComplex`] which can wrap any Rust value into a [`Value`].

use std::any;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;

use allocative::Allocative;
use pagable::StaticValue;
use starlark_derive::NoSerialize;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::pagable::vtable_register::VtableRegistered;
use crate::typing::starlark_value::TyStarlarkValueVTable;
use crate::values::AllocFrozenRepresentation;
use crate::values::AllocRepresentation;
use crate::values::Allocator;
use crate::values::ComplexValue;
use crate::values::Freeze;
use crate::values::FreezePreservesType;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::FrozenAllocator;
use crate::values::Heap;
use crate::values::HeapSendable;
use crate::values::StarlarkValue;
use crate::values::Trace;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::layout::avalue::AValueSimpleBound;

/// Allocate arbitrary value on the starlark heap without implementing full [`StarlarkValue`].
///
/// This is useful for data not directly visible to starlark code.
///
/// This type is for "complex" values (with tracing during GC). For no GC version check
/// [`StarlarkAny`](crate::values::types::any::StarlarkAny).
#[derive(Trace, Allocative, ProvidesStaticType, NoSerialize)]
pub struct StarlarkAnyComplex<T> {
    /// The value.
    pub value: T,
}

impl<'v, T: Freeze<'v>> Freeze<'v> for StarlarkAnyComplex<T> {
    type Frozen<'fv> = StarlarkAnyComplex<T::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        Ok(StarlarkAnyComplex {
            value: self.value.freeze(freezer)?,
        })
    }
}

impl<'v, T> StarlarkAnyComplex<T>
where
    Self: StarlarkValue<'v>,
{
    /// Construct a new `StarlarkAnyComplex` value, which can be allocated on the heap.
    pub fn new(value: T) -> StarlarkAnyComplex<T> {
        StarlarkAnyComplex { value }
    }

    /// Obtain the value from a `Value`, if it is a `StarlarkAnyComplex<T>`.
    pub fn get(value: Value<'v>) -> Option<&'v T> {
        value.downcast_ref::<Self>().map(|x| &x.value)
    }

    /// Obtain the value from a `Value`, if it is a `StarlarkAnyComplex<T>`.
    pub fn get_err(value: Value<'v>) -> crate::Result<&'v T> {
        value.downcast_ref_err::<Self>().map(|x| &x.value)
    }
}

// Proper `Debug` is hard to require from users because of `Freeze` and
// `ProvidesStaticType`.
impl<T> Debug for StarlarkAnyComplex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(any::type_name::<T>())
            .finish_non_exhaustive()
    }
}

impl<T> Display for StarlarkAnyComplex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

/// Marker trait certifying that `T` has a registered typing vtable entry
/// for `StarlarkAnyComplex<T>`. Implemented per-`T` by
/// [`register_starlark_any_complex!`][crate::register_starlark_any_complex].
///
/// This indirection exists to work around the orphan rule: downstream
/// crates can't `impl HasTyVTable for StarlarkAnyComplex<LocalT>` directly
/// (both trait and outer type are foreign), but they can impl this marker on
/// their local `T`. A blanket impl in this crate then provides `HasTyVTable`
/// for the corresponding `StarlarkAnyComplex<T>`.
///
/// A `T` that lives on the frozen heap (`register_starlark_any_complex!(frozen ..)`) must also
/// be [`StarlarkPagable`](crate::pagable::StarlarkPagable) at its brand, which the registration
/// checks. Types that never actually round-trip can derive
/// [`StarlarkPagablePanic`](starlark_derive::StarlarkPagablePanic) to satisfy the bound with
/// panicking stubs.
pub trait StarlarkAnyComplexHasTyVTable {
    /// Typing vtable entry for `StarlarkAnyComplex<Self>`.
    const TY_VTABLE_STATIC: StaticValue<TyStarlarkValueVTable>;
}

#[cfg(feature = "pagable")]
impl<T> crate::typing::HasTyVTable for StarlarkAnyComplex<T>
where
    T: StarlarkAnyComplexHasTyVTable,
{
    const TY_VTABLE_STATIC: StaticValue<TyStarlarkValueVTable> =
        <T as StarlarkAnyComplexHasTyVTable>::TY_VTABLE_STATIC;
}

#[starlark_value(type = "any_complex")]
impl<'v, T> StarlarkValue<'v> for StarlarkAnyComplex<T>
where
    T: Allocative + ProvidesStaticType<'v> + 'v + StarlarkAnyComplexHasTyVTable,
    T::StaticType: Sized,
{
}

impl<T: crate::pagable::StarlarkSerialize> crate::pagable::StarlarkSerialize
    for StarlarkAnyComplex<T>
{
    fn starlark_serialize(
        &self,
        ctx: &mut dyn crate::pagable::StarlarkSerializeContext,
    ) -> crate::Result<()> {
        <T as crate::pagable::StarlarkSerialize>::starlark_serialize(&self.value, ctx)
    }
}

impl<'fv, T: crate::pagable::StarlarkDeserialize<'fv>> crate::pagable::StarlarkDeserialize<'fv>
    for StarlarkAnyComplex<T>
{
    fn starlark_deserialize(
        ctx: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        Ok(StarlarkAnyComplex {
            value: <T as crate::pagable::StarlarkDeserialize<'fv>>::starlark_deserialize(ctx)?,
        })
    }
}

/// Register vtables for `StarlarkAnyComplex<T>`.
///
/// Three forms:
/// - `register_starlark_any_complex!(T)` — registers the typing vtable for
///   `StarlarkAnyComplex<T>`. Use for a `T` that is never stored on a frozen heap: a `T` that
///   freezes into another type, or one that is only ever allocated with
///   [`Heap::alloc_complex_no_freeze`](crate::values::Heap::alloc_complex_no_freeze).
/// - `register_starlark_any_complex!(frozen T)` — registers the typing vtable **and**
///   the frozen-heap vtable. Use for a `T` that is stored on frozen heaps: one that is its own
///   frozen form, or one that another type freezes into.
/// - `register_starlark_any_complex!(T, frozen FrozenT)` — both of the above in one call, for
///   a `T` that freezes into a different `FrozenT`. Such a `T` is allocated with
///   [`StarlarkAnyComplex::alloc_retyped_by_freeze`].
///
/// ```ignore
/// register_starlark_any_complex!(frozen Foo<'_>);
/// register_starlark_any_complex!(Bar<'_>, frozen FrozenBar<'_>);
/// ```
#[macro_export]
macro_rules! register_starlark_any_complex {
    ($unfrozen:ty, frozen $frozen:ty) => {
        $crate::register_starlark_any_complex!($unfrozen);
        $crate::register_starlark_any_complex!(frozen $frozen);
    };
    (frozen $t:ty) => {
        $crate::register_starlark_any_complex!($t);
        // SAFETY: register_simple_vtable_entry! below registers the vtable.
        unsafe impl $crate::values::any_complex::FrozenAnyComplexVtableRegistered for $t {}
        $crate::register_simple_vtable_entry!($crate::values::any_complex::StarlarkAnyComplex<$t>);
    };
    ($t:ty) => {
        const _: () = {
            $crate::__declare_ty_vtable_static!(
                $crate::values::any_complex::StarlarkAnyComplex<$t>
            );
            impl $crate::values::any_complex::StarlarkAnyComplexHasTyVTable for $t {
                const TY_VTABLE_STATIC: pagable::StaticValue<
                    $crate::__derive_refs::TyStarlarkValueVTable,
                > = VTABLE_STATIC;
            }
        };
    };
}

impl<T> StarlarkAnyComplex<T> {
    /// Allocate as a value that cannot be frozen, for a payload without a
    /// [`Freeze`](crate::values::Freeze) impl; freezing the module that holds it is an error.
    /// A payload that is its own frozen form is allocated through
    /// [`alloc`](crate::values::Heap::alloc) instead, and one that freezes into another
    /// payload type through [`alloc_retyped_by_freeze`](Self::alloc_retyped_by_freeze).
    pub fn alloc_no_freeze<'v>(self, heap: Heap<'v>) -> ValueTyped<'v, Self>
    where
        Self: StarlarkValue<'v> + Trace<'v> + HeapSendable<'v>,
    {
        Allocator::new(heap).alloc_complex_no_freeze(self)
    }

    /// Allocate a payload that freezes into another payload type, as
    /// `register_starlark_any_complex!(T, frozen FrozenT)` declares one.
    ///
    /// `StarlarkAnyComplex<T>` is a Starlark type of its own for each `T`, so such a value is
    /// of another type once it is frozen: the one exception to
    /// [`FreezePreservesType`](crate::values::FreezePreservesType). That is why this gives
    /// the handle of the representation, which freezes into the frozen representation's, and
    /// there is no handle of the type to be had: it would not survive the freeze.
    //
    // FIXME(JakobDegen): This exists because `StarlarkAnyComplex` does not line up with how
    // Starlark types work everywhere else. It is one Starlark type per payload type, while a
    // payload and the different payload it freezes into are really two representations of one
    // type, as a mutable and a frozen list are. Making them one type means naming the
    // wrapper's type after the frozen payload, which needs payloads that never freeze to move
    // to a wrapper of their own first.
    pub fn alloc_retyped_by_freeze<'v>(self, heap: Heap<'v>) -> ValueTyped<'v, Self>
    where
        Self: ComplexValue<'v> + HeapSendable<'v>,
    {
        Allocator::new(heap).alloc_complex_any_frozen_type(self)
    }
}

impl<'v, T> AllocRepresentation<'v> for StarlarkAnyComplex<T>
where
    Self: StarlarkValue<'v> + HeapSendable<'v> + FreezePreservesType<'v>,
    T: Trace<'v> + ProvidesStaticType<'v> + Freeze<'v>,
    for<'fv> StarlarkAnyComplex<T::Frozen<'fv>>: AValueSimpleBound<'fv>,
    for<'fv> StarlarkAnyComplex<T::Frozen<'fv>>:
        ProvidesStaticType<'fv, StaticType = StarlarkAnyComplex<T::Frozen<'static>>>,
{
    type Repr = Self;

    fn alloc(self, allocator: Allocator<'v>) -> ValueTyped<'v, Self> {
        allocator.alloc_complex(self)
    }
}

impl<'fv, T> AllocFrozenRepresentation<'fv> for StarlarkAnyComplex<T>
where
    Self: AValueSimpleBound<'fv>,
{
    type Repr = Self;

    fn alloc_frozen(self, allocator: FrozenAllocator<'fv>) -> ValueTyped<'fv, Self> {
        allocator.alloc_simple(self)
    }
}

/// Marker trait for frozen types that can be wrapped in `StarlarkAnyComplex`
/// and are registered for vtable lookup.
///
/// # Safety
///
/// Implementors must also register the vtable entry for `StarlarkAnyComplex<Self>`
/// using [`register_simple_vtable_entry!`](macro@crate::register_simple_vtable_entry). Use the
/// [`register_starlark_any_complex!`](macro@crate::register_starlark_any_complex)
/// macro with the `frozen` form instead of implementing this trait manually —
/// it handles both the trait implementation and vtable registration.
pub unsafe trait FrozenAnyComplexVtableRegistered {}

unsafe impl<T> VtableRegistered for StarlarkAnyComplex<T> where T: FrozenAnyComplexVtableRegistered {}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use starlark_derive::ProvidesStaticType;
    use starlark_derive::StarlarkPagable;
    use starlark_derive::StarlarkPagablePanic;
    use starlark_derive::Trace;

    use crate as starlark;
    use crate::const_frozen_string;
    use crate::environment::Module;
    use crate::values::Freeze;
    use crate::values::FreezeResult;
    use crate::values::Freezer;
    use crate::values::StringValue;
    use crate::values::Value;
    use crate::values::layout::heap::name::StarlarkTestHeapName;
    use crate::values::list::AllocList;
    use crate::values::types::any_complex::StarlarkAnyComplex;

    // Types used for StarlarkAnyComplex test - must be at module level for registration.
    #[derive(Trace, Allocative, ProvidesStaticType, StarlarkPagablePanic)]
    struct UnfrozenData<'v> {
        string: StringValue<'v>,
        other: Value<'v>,
    }

    impl<'v> Freeze<'v> for UnfrozenData<'v> {
        type Frozen<'fv> = FrozenData<'fv>;

        fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<FrozenData<'fv>> {
            Ok(FrozenData {
                string: self.string.freeze(freezer)?,
                other: freezer.freeze(self.other)?,
            })
        }
    }

    #[derive(Allocative, ProvidesStaticType, StarlarkPagable)]
    struct FrozenData<'fv> {
        string: StringValue<'fv>,
        other: Value<'fv>,
    }

    crate::register_starlark_any_complex!(UnfrozenData<'_>, frozen FrozenData<'_>);

    #[test]
    fn test_any_complex() {
        Module::with_temp_heap(|module| {
            let data = StarlarkAnyComplex::new(UnfrozenData {
                string: module.heap().alloc_str("aaa"),
                other: module.heap().alloc(AllocList([1, 2])).to_value(),
            })
            .alloc_retyped_by_freeze(module.heap())
            .to_value();

            assert_eq!(
                const_frozen_string!("aaa").at(),
                StarlarkAnyComplex::<UnfrozenData>::get_err(data)
                    .unwrap()
                    .string
            );

            module.set_extra_value(data);

            let module = module.freeze_named(StarlarkTestHeapName::frozen_heap_name())?;

            module.extra_value().unwrap().by_ref(|data| {
                assert_eq!(
                    const_frozen_string!("aaa").at(),
                    StarlarkAnyComplex::<FrozenData>::get_err(*data)
                        .unwrap()
                        .string
                );
            });
            crate::Result::Ok(())
        })
        .unwrap();
    }
}
