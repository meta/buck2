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

use std::mem;
use std::mem::MaybeUninit;
use std::ptr;

use allocative::Key;
use allocative::Visitor;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;

use crate::pagable::vtable_register::register_special_avalue_frozen;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::StarlarkTypeExpr;
use crate::values::Trace;
use crate::values::Tracer;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::layout::avalue::AValue;
use crate::values::layout::avalue::AValueImpl;
use crate::values::layout::avalue::heap_copy_impl;
use crate::values::layout::heap::maybe_uninit_slice_util::maybe_uninit_write_from_exact_size_iter;
use crate::values::layout::heap::repr::AValueHeader;
use crate::values::layout::heap::repr::AValueRepr;
use crate::values::layout::heap::repr::ForwardPtr;
use crate::values::list::StarlarkList;
use crate::values::list::value::ListGen;
use crate::values::list::value::VALUE_EMPTY_FROZEN_LIST;
use crate::values::types::array::Array;
use crate::values::types::list::value::FrozenListData;
use crate::values::types::list::value::ListData;

fn list_avalue<'v>(
    content: ValueTyped<'v, Array<'v>>,
) -> AValueImpl<'v, impl AValue<'v, StarlarkValue = ListGen<ListData<'v>>, ExtraElem = ()>> {
    AValueImpl::<AValueList>::new(ListGen(ListData::new(content)))
}

fn frozen_list_avalue<'fv>(len: usize) -> AValueImpl<'fv, AValueFrozenList> {
    AValueImpl::<AValueFrozenList>::new(unsafe { ListGen(FrozenListData::new(len)) })
}

struct AValueList;

impl<'v> AValue<'v> for AValueList {
    type StarlarkValue = ListGen<ListData<'v>>;

    type ExtraElem = ();

    fn extra_len(_value: &ListGen<ListData<'v>>) -> usize {
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
            let content = (*me).payload.0.content();

            if content.is_empty() {
                let fv = Value::new_empty_list();
                AValueHeader::overwrite_with_forward::<Self::StarlarkValue>(
                    me,
                    ForwardPtr::new_frozen(fv),
                );
                return Ok(fv);
            }

            let (r, extra) = freezer
                .frozen_heap()
                .reserve_with_extra::<AValueFrozenList>(content.len());
            AValueHeader::overwrite_with_forward::<Self::StarlarkValue>(me, r.forward_ptr());
            let extra = &mut *extra;
            assert_eq!(extra.len(), content.len());
            for (elem_place, elem) in extra.iter_mut().zip(content) {
                elem_place.write(freezer.freeze(*elem)?);
            }
            Ok(r.fill(ListGen(FrozenListData::new(content.len()))))
        }
    }

    unsafe fn heap_copy(
        me: *mut AValueRepr<Self::StarlarkValue>,
        tracer: &Tracer<'v>,
    ) -> Value<'v> {
        unsafe { heap_copy_impl::<Self>(me, tracer, Trace::trace) }
    }
}

pub(crate) struct AValueFrozenList;

impl<'v> AValue<'v> for AValueFrozenList {
    type StarlarkValue = ListGen<FrozenListData<'v>>;

    type ExtraElem = Value<'v>;

    fn extra_len(value: &ListGen<FrozenListData<'v>>) -> usize {
        value.0.len()
    }

    fn offset_of_extra() -> usize {
        ListGen::<FrozenListData<'v>>::offset_of_content()
    }

    fn visit_extra_allocative<'a, 'b: 'a>(
        value: &Self::StarlarkValue,
        visitor: &'a mut Visitor<'b>,
    ) {
        visitor.visit_simple(Key::new("content"), mem::size_of::<Value>() * value.0.len());
    }

    unsafe fn heap_freeze<'fv>(
        _me: *mut AValueRepr<Self::StarlarkValue>,
        _freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<Value<'fv>> {
        panic!("already frozen");
    }

    unsafe fn heap_copy(
        _me: *mut AValueRepr<Self::StarlarkValue>,
        _tracer: &Tracer<'v>,
    ) -> Value<'v> {
        panic!("shouldn't be copying frozen values");
    }

    fn starlark_serialize(
        me: *const AValueRepr<Self::StarlarkValue>,
        ctx: &mut dyn crate::pagable::StarlarkSerializeContext,
    ) -> crate::Result<()> {
        let value = unsafe { &(*me).payload };
        let content = value.0.content();
        content.len().pagable_serialize(ctx.pagable())?;
        for elem in content {
            crate::pagable::StarlarkSerialize::starlark_serialize(elem, ctx)?;
        }
        Ok(())
    }

    fn starlark_deserialize<'fv>(
        me: *mut AValueRepr<Self::StarlarkValue>,
        ctx: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<()> {
        let len = usize::pagable_deserialize(ctx.pagable())?;
        unsafe {
            ptr::write(&mut (*me).payload, ListGen(FrozenListData::new(len)));
            let extra_offset = AValueRepr::<Self::StarlarkValue>::offset_of_payload()
                + <Self as AValue>::offset_of_extra();
            // The elements are deserialized at `'fv` into slots the heap types at `'v`: the
            // brand erasure described on `AValue::starlark_deserialize`.
            let extra_ptr = (me as *mut u8).add(extra_offset) as *mut MaybeUninit<Value<'fv>>;
            for i in 0..len {
                (*extra_ptr.add(i)).write(ctx.deserialize_value()?);
            }
        }
        Ok(())
    }
}

impl<'fh> FrozenHeap<'fh> {
    /// Allocate a list of these items on this heap, typed by them.
    #[inline]
    pub(crate) fn alloc_list_iter<T: StarlarkTypeExpr<'fh>>(
        self,
        elems: impl IntoIterator<Item = ValueOf<'fh, T>>,
    ) -> ValueOf<'fh, StarlarkList<'fh, T>> {
        // The items are `T`s, so the list is a `list[T]`.
        ValueOf::new_unchecked(self.alloc_list_values(elems.into_iter().map(ValueOf::to_value)))
    }

    fn alloc_list_values(self, elems: impl Iterator<Item = Value<'fh>>) -> Value<'fh> {
        let (lower, upper) = elems.size_hint();
        if Some(lower) == upper {
            if lower == 0 {
                return VALUE_EMPTY_FROZEN_LIST.at().to_value();
            }

            unsafe {
                let (v, elem_places) = self.alloc_raw_extra(frozen_list_avalue(lower));
                let elem_places = &mut *elem_places;
                maybe_uninit_write_from_exact_size_iter(elem_places, elems, Value::new_none());
                v.to_value()
            }
        } else {
            self.alloc_list_values(elems.collect::<Vec<_>>().into_iter())
        }
    }
}

impl<'v> Heap<'v> {
    /// Allocate a list with the given elements.
    pub(crate) fn alloc_list(self, elems: &[Value<'v>]) -> ValueOf<'v, StarlarkList<'v>> {
        let array = self.alloc_array(elems.len());
        array.extend_from_slice(elems);
        // Every value is a `typing.Any`, so the list is a `list[typing.Any]`.
        ValueOf::new_unchecked(self.alloc_raw(list_avalue(array)).to_value())
    }

    /// Allocate a list of these items, typed by them.
    #[inline]
    pub(crate) fn alloc_list_iter<T: StarlarkTypeExpr<'v>>(
        self,
        elems: impl IntoIterator<Item = ValueOf<'v, T>>,
    ) -> ValueOf<'v, StarlarkList<'v, T>> {
        // The items are `T`s, so the list is a `list[T]`.
        ValueOf::new_unchecked(self.alloc_list_values(elems.into_iter().map(ValueOf::to_value)))
    }

    fn alloc_list_values(self, elems: impl Iterator<Item = Value<'v>>) -> Value<'v> {
        let array = self.alloc_array(0);
        let list = self.alloc_raw(list_avalue(array));
        list.0.extend(elems, self);
        list.to_value()
    }

    /// Allocate a list by concatenating two slices.
    pub(crate) fn alloc_list_concat(
        self,
        a: &[Value<'v>],
        b: &[Value<'v>],
    ) -> ValueOf<'v, StarlarkList<'v>> {
        let array = self.alloc_array(a.len() + b.len());
        array.extend_from_slice(a);
        array.extend_from_slice(b);
        // Every value is a `typing.Any`, so the list is a `list[typing.Any]`.
        ValueOf::new_unchecked(self.alloc_raw(list_avalue(array)).to_value())
    }
}

// Register vtable for FrozenList (special type not handled by #[starlark_value] macro).
register_special_avalue_frozen!(crate::values::list::value::FrozenList, AValueFrozenList);
