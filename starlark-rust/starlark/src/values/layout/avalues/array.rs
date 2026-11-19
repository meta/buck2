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
use std::mem::MaybeUninit;
use std::ptr;

use allocative::Key;
use allocative::Visitor;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;

use crate::collections::maybe_uninit_backport::maybe_uninit_write_slice;
use crate::collections::maybe_uninit_backport::maybe_uninit_write_slice_cloned;
use crate::pagable::StarlarkPagable;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::Trace;
use crate::values::Tracer;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::array::VALUE_EMPTY_ARRAY;
use crate::values::layout::avalue::AValue;
use crate::values::layout::avalue::AValueImpl;
use crate::values::layout::heap::repr::AValueForward;
use crate::values::layout::heap::repr::AValueHeader;
use crate::values::layout::heap::repr::AValueRepr;
use crate::values::types::any_array::AnyArray;
use crate::values::types::any_array::AnyArrayRegistered;
use crate::values::types::array::Array;

fn array_avalue<'v>(
    cap: u32,
) -> AValueImpl<'v, impl AValue<'v, StarlarkValue = Array<'v>, ExtraElem = Value<'v>>> {
    AValueImpl::<AValueArray>::new(unsafe { Array::new(0, cap) })
}

fn any_array_avalue<'v, T: AnyArrayRegistered + for<'fv> StarlarkPagable<'fv>>(
    cap: usize,
) -> AValueImpl<'v, impl AValue<'v, StarlarkValue = AnyArray<T>, ExtraElem = T>> {
    AValueImpl::<AValueAnyArray<T>>::new(unsafe { AnyArray::new(cap) })
}

struct AValueArray;

impl<'v> AValue<'v> for AValueArray {
    type StarlarkValue = Array<'v>;

    type ExtraElem = Value<'v>;

    fn extra_len(value: &Array<'v>) -> usize {
        // Note we return capacity, not length here.
        value.capacity()
    }

    fn offset_of_extra() -> usize {
        Array::offset_of_content()
    }

    fn visit_extra_allocative<'a, 'b: 'a>(
        value: &Self::StarlarkValue,
        visitor: &'a mut Visitor<'b>,
    ) {
        visitor.visit_simple(Key::new("content"), mem::size_of::<Value>() * value.len());
        visitor.visit_simple(
            Key::new("unused_capacity"),
            mem::size_of::<Value>() * (value.capacity() - value.len()),
        );
    }

    unsafe fn heap_freeze<'fv>(
        _me: *mut AValueRepr<Self::StarlarkValue>,
        _freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<Value<'fv>> {
        panic!("arrays should not be frozen")
    }

    unsafe fn heap_copy(
        me: *mut AValueRepr<Self::StarlarkValue>,
        tracer: &Tracer<'v>,
    ) -> Value<'v> {
        unsafe {
            debug_assert!(
                (*me).payload.capacity() != 0,
                "empty array is allocated statically"
            );

            if (*me).payload.len() == 0 {
                return VALUE_EMPTY_ARRAY.unpack().to_value();
            }

            AValueForward::assert_does_not_overwrite_extra::<Self>();
            let content = (*me).payload.content_mut();

            let (r, extra) = tracer.reserve_with_extra::<Self>(content.len());
            let x =
                AValueHeader::overwrite_with_forward::<Self::StarlarkValue>(me, r.forward_ptr());

            debug_assert_eq!(content.len(), x.len());

            content.trace(tracer);

            // Note when copying we are dropping extra capacity.
            let v = r.fill(Array::new(content.len() as u32, content.len() as u32));
            let extra = &mut *extra;
            maybe_uninit_write_slice(extra, content);
            v
        }
    }
}

/// `AValue` impl for `AnyArray<T>`: stored on the frozen heap with a
/// trailing slice of `T` (see `alloc_any_array_value`).
pub struct AValueAnyArray<T>(PhantomData<T>);

impl<'v, T: AnyArrayRegistered + for<'fv> StarlarkPagable<'fv>> AValue<'v> for AValueAnyArray<T> {
    type StarlarkValue = AnyArray<T>;
    type ExtraElem = T;

    fn extra_len(value: &AnyArray<T>) -> usize {
        value.len
    }

    fn offset_of_extra() -> usize {
        AnyArray::<T>::offset_of_content()
    }

    fn visit_extra_allocative<'a, 'b: 'a>(
        value: &Self::StarlarkValue,
        visitor: &'a mut Visitor<'b>,
    ) {
        visitor.visit_simple(Key::new("content"), mem::size_of::<T>() * value.len);
    }

    unsafe fn heap_freeze<'fv>(
        _me: *mut AValueRepr<Self::StarlarkValue>,
        _freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<Value<'fv>> {
        panic!("AnyArray for now can only be allocated in FrozenHeap");
    }

    unsafe fn heap_copy(
        _me: *mut AValueRepr<Self::StarlarkValue>,
        _tracer: &Tracer<'v>,
    ) -> Value<'v> {
        panic!("AnyArray for now can only be allocated in FrozenHeap");
    }

    /// Wire format: `len: usize` followed by `len` elements serialized via
    /// `T::starlark_serialize`. Mirrors `AValueList::starlark_serialize`.
    fn starlark_serialize(
        me: *const AValueRepr<Self::StarlarkValue>,
        ctx: &mut dyn crate::pagable::StarlarkSerializeContext,
    ) -> crate::Result<()> {
        let any_array = unsafe { &(*me).payload };
        any_array.len.pagable_serialize(ctx.pagable())?;
        for elem in any_array.as_slice() {
            elem.starlark_serialize(ctx)?;
        }
        Ok(())
    }

    /// Reconstruct into pre-allocated memory. Layout at `me`:
    ///
    /// ```text
    ///        ┌─────────────────────────────┐  ← me
    ///        │ AValueHeader                │
    ///        ├─────────────────────────────┤  ← + offset_of_payload()
    ///   (1)  │ AnyArray<T> { len }         │  payload
    ///        ├─────────────────────────────┤  ← + offset_of_extra()
    ///   (2)  │ T[0], T[1], ..., T[len-1]   │  trailing element slots
    ///        └─────────────────────────────┘
    /// ```
    ///
    /// 1. Write the `AnyArray<T>` payload (with `len`) into `me.payload`.
    /// 2. Deserialize each of `len` elements into its trailing slot.
    fn starlark_deserialize<'fv>(
        me: *mut AValueRepr<Self::StarlarkValue>,
        ctx: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<()> {
        let len = usize::pagable_deserialize(ctx.pagable())?;
        unsafe {
            // Initialize the `AnyArray<T>` shell with the correct len.
            ptr::write(&mut (*me).payload, AnyArray::new(len));

            // The trailing `T`s live at `offset_of_payload + offset_of_extra`.
            let extra_offset = AValueRepr::<Self::StarlarkValue>::offset_of_payload()
                + <Self as AValue>::offset_of_extra();
            let extra_ptr = (me as *mut u8).add(extra_offset) as *mut MaybeUninit<T>;
            for i in 0..len {
                let elem = T::starlark_deserialize(ctx)?;
                (*extra_ptr.add(i)).write(elem);
            }
        }
        Ok(())
    }
}

impl<'fh> FrozenHeap<'fh> {
    /// Allocate a slice in the frozen heap as an [`AnyArray`].
    pub(crate) fn alloc_any_array_value<
        T: AnyArrayRegistered + for<'fv> StarlarkPagable<'fv> + Send + Sync + Clone,
    >(
        self,
        values: &[T],
    ) -> ValueTyped<'fh, AnyArray<T>> {
        // Always allocate via AnyArray, even for empty/single elements.
        // This keeps the pointer arithmetic from the elements back to the value valid.
        let (any_array, content) = self.alloc_raw_extra(any_array_avalue(values.len()));
        let content = unsafe { &mut *content };
        maybe_uninit_write_slice_cloned(content, values);
        any_array
    }
}

impl<'v> Heap<'v> {
    pub(crate) fn alloc_array(self, cap: usize) -> ValueTyped<'v, Array<'v>> {
        if cap == 0 {
            return VALUE_EMPTY_ARRAY.unpack();
        }

        let cap: u32 = cap.try_into().expect("capacity overflows u32::MAX");

        self.alloc_raw_extra(array_avalue(cap)).0
    }
}
