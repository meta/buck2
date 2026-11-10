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
use std::mem;

#[cfg(feature = "pagable")]
use crate::pagable::StarlarkDeserialize;
#[cfg(feature = "pagable")]
use crate::pagable::StarlarkDeserializeAt;
use crate::private::Private;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::Tracer;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::layout::avalue::AValue;
use crate::values::layout::avalue::AValueImpl;
use crate::values::layout::avalue::AValueSimpleBound;
use crate::values::layout::avalue::heap_copy_impl;
use crate::values::layout::heap::repr::AValueHeader;
use crate::values::layout::heap::repr::AValueRepr;

/// AValue implementation for a simple value on the unfrozen heap: a `T` that holds no values,
/// and so is a simple value at every brand. Freezing moves it to the frozen heap as it is, as an
/// [`AValueFrozen`].
pub(crate) struct AValueSimple<T>(PhantomData<T>);

impl<'v, T: for<'a> AValueSimpleBound<'a>> AValue<'v> for AValueSimple<T> {
    type StarlarkValue = T;

    type ExtraElem = ();

    fn extra_len(_value: &T) -> usize {
        0
    }

    fn offset_of_extra() -> usize {
        mem::size_of::<Self>()
    }

    unsafe fn heap_freeze<'fv>(
        me: *mut AValueRepr<Self::StarlarkValue>,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<Value<'fv>> {
        unsafe {
            let (r, _extra) = freezer
                .frozen_heap()
                .reserve_with_extra::<AValueFrozen<T>>(0);
            let x = AValueHeader::overwrite_with_forward::<T>(me, r.forward_ptr());
            Ok(r.fill(x))
        }
    }

    unsafe fn heap_copy(
        me: *mut AValueRepr<Self::StarlarkValue>,
        tracer: &Tracer<'v>,
    ) -> Value<'v> {
        unsafe { heap_copy_impl::<Self>(me, tracer, |_v, _tracer| {}) }
    }
}

/// AValue implementation for a fixed-size value on a frozen heap: a `T` at the heap's brand,
/// whether it holds values or not, since nothing on a frozen heap is traced or frozen. This is
/// the vtable that paging registers for every such type.
pub struct AValueFrozen<T>(PhantomData<T>);

impl<'v, T: AValueSimpleBound<'v>> AValue<'v> for AValueFrozen<T> {
    type StarlarkValue = T;

    type ExtraElem = ();

    fn extra_len(_value: &T) -> usize {
        0
    }

    fn offset_of_extra() -> usize {
        mem::size_of::<Self>()
    }

    unsafe fn heap_freeze<'fv>(
        _me: *mut AValueRepr<Self::StarlarkValue>,
        _freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<Value<'fv>> {
        unreachable!("a value in a frozen heap is never frozen")
    }

    unsafe fn heap_copy(
        _me: *mut AValueRepr<Self::StarlarkValue>,
        _tracer: &Tracer<'v>,
    ) -> Value<'v> {
        unreachable!("a value in a frozen heap is never garbage collected")
    }

    #[cfg(feature = "pagable")]
    fn starlark_serialize(
        me: *const AValueRepr<Self::StarlarkValue>,
        ctx: &mut dyn crate::pagable::StarlarkSerializeContext,
    ) -> crate::Result<()> {
        let value = unsafe { &(*me).payload };
        value.starlark_serialize(ctx)
    }

    #[cfg(feature = "pagable")]
    fn starlark_deserialize<'fv>(
        me: *mut AValueRepr<Self::StarlarkValue>,
        ctx: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<()> {
        let value = <T as StarlarkDeserializeAt<'v, 'fv>>::Reinfected::starlark_deserialize(ctx)?;
        // SAFETY: `Reinfected` is `T` with `'v` replaced by `'fv` (the `ProvidesStaticType` and
        // `IsStaticType` contracts), so it has `T`'s layout and the slot fits it. Writing the
        // `'fv` value into the `'v` slot is the brand erasure described on
        // `AValue::starlark_deserialize`.
        unsafe {
            std::ptr::write((&raw mut (*me).payload).cast(), value);
        }
        Ok(())
    }
}

// The strategies are crate-private: a representation outside the crate chooses its strategy
// in its `AllocRepresentation` impl, through the `Allocator`.
impl<'fh> FrozenHeap<'fh> {
    /// Allocate a value at this heap's brand: nothing on a frozen heap is traced or frozen.
    pub(crate) fn alloc_simple_typed<T: AValueSimpleBound<'fh>>(
        self,
        val: T,
    ) -> ValueTyped<'fh, T> {
        assert!(!T::is_special(Private));
        self.alloc_raw(AValueImpl::<AValueFrozen<T>>::new(val))
    }

    /// [`alloc_simple_typed`](Self::alloc_simple_typed) as a [`Value`].
    pub(crate) fn alloc_simple<T: AValueSimpleBound<'fh>>(self, val: T) -> Value<'fh> {
        self.alloc_simple_typed(val).to_value()
    }
}

impl<'v> Heap<'v> {
    /// Allocate a `'static` value: it holds no `Value`s, so it is neither traced nor frozen, and
    /// freezing the module moves it to the frozen heap as it is. Being `'static`, it is [`Send`]
    /// and [`Sync`] like the contents of every frozen value.
    pub(crate) fn alloc_simple<T: for<'a> AValueSimpleBound<'a>>(self, x: T) -> Value<'v> {
        assert!(!T::is_special(Private));
        self.alloc_raw(AValueImpl::<AValueSimple<T>>::new(x))
            .to_value()
    }
}
