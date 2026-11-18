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

use allocative::Key;
use allocative::Visitor;

use crate::collections::maybe_uninit_backport::maybe_uninit_write_slice;
use crate::pagable::vtable_register::register_special_avalue_frozen;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::StarlarkTypeExpr;
use crate::values::Tracer;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::layout::avalue::AValue;
use crate::values::layout::avalue::AValueImpl;
use crate::values::layout::avalue::heap_copy_with_extra;
use crate::values::layout::avalue::heap_freeze_with_extra;
use crate::values::layout::avalue::starlark_deserialize_with_extra;
use crate::values::layout::avalue::starlark_serialize_with_extra;
use crate::values::layout::heap::maybe_uninit_slice_util::maybe_uninit_write_from_exact_size_iter;
use crate::values::layout::heap::repr::AValueRepr;
use crate::values::tuple::TupleType;
use crate::values::types::tuple::value::Tuple;

fn tuple_avalue<'v>(len: usize) -> AValueImpl<'v, AValueTuple> {
    AValueImpl::<AValueTuple>::new(unsafe { Tuple::new(len) })
}

fn frozen_tuple_avalue<'fv>(len: usize) -> AValueImpl<'fv, AValueFrozenTuple> {
    AValueImpl::<AValueFrozenTuple>::new(unsafe { Tuple::new(len) })
}

struct AValueTuple;

impl<'v> AValue<'v> for AValueTuple {
    type StarlarkValue = Tuple<'v>;

    type ExtraElem = Value<'v>;

    fn extra_len(value: &Tuple<'v>) -> usize {
        value.len()
    }

    fn offset_of_extra() -> usize {
        Tuple::offset_of_content()
    }

    fn visit_extra_allocative<'a, 'b: 'a>(
        value: &Self::StarlarkValue,
        visitor: &'a mut Visitor<'b>,
    ) {
        visitor.visit_simple(
            Key::new("content"),
            std::mem::size_of::<Value>() * value.len(),
        );
    }

    unsafe fn heap_freeze<'fv>(
        me: *mut AValueRepr<Self::StarlarkValue>,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<Value<'fv>> {
        unsafe {
            debug_assert!(
                (*me).payload.len() != 0,
                "empty tuple is allocated statically"
            );
            heap_freeze_with_extra::<Self, AValueFrozenTuple>(me, freezer, |_, _, len| {
                Ok(Tuple::new(len))
            })
        }
    }

    unsafe fn heap_copy(
        me: *mut AValueRepr<Self::StarlarkValue>,
        tracer: &Tracer<'v>,
    ) -> Value<'v> {
        unsafe {
            debug_assert!(
                (*me).payload.len() != 0,
                "empty tuple is allocated statically"
            );
            heap_copy_with_extra::<Self>(me, tracer, |x, _| x)
        }
    }
}

struct AValueFrozenTuple;

impl<'v> AValue<'v> for AValueFrozenTuple {
    type StarlarkValue = Tuple<'v>;

    type ExtraElem = Value<'v>;

    fn extra_len(value: &Tuple<'v>) -> usize {
        value.len()
    }

    fn offset_of_extra() -> usize {
        Tuple::offset_of_content()
    }

    fn visit_extra_allocative<'a, 'b: 'a>(
        value: &Self::StarlarkValue,
        visitor: &'a mut Visitor<'b>,
    ) {
        visitor.visit_simple(
            Key::new("content"),
            std::mem::size_of::<Value>() * value.len(),
        );
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
        starlark_serialize_with_extra::<Self>(me, ctx, |_, _| Ok(()))
    }

    fn starlark_deserialize<'fv>(
        me: *mut AValueRepr<Self::StarlarkValue>,
        ctx: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<()> {
        unsafe {
            starlark_deserialize_with_extra::<Self, Tuple<'fv>>(me, ctx, |_, len| {
                Ok(Tuple::new(len))
            })
        }
    }
}

impl<'fh> FrozenHeap<'fh> {
    /// Allocate a tuple with the given elements on this heap.
    pub(crate) fn alloc_tuple(self, elems: &[Value<'fh>]) -> ValueOf<'fh, TupleType<Value<'fh>>> {
        // Every value is a `typing.Any`, so the tuple is a `tuple[typing.Any, ...]`.
        ValueOf::new_unchecked(self.alloc_tuple_values(elems.iter().copied()))
    }

    /// Allocate a tuple of these items on this heap, typed by them.
    #[inline]
    pub(crate) fn alloc_tuple_iter<T: StarlarkTypeExpr<'fh>>(
        self,
        elems: impl IntoIterator<Item = ValueOf<'fh, T>>,
    ) -> ValueOf<'fh, TupleType<T>> {
        // The items are `T`s, so the tuple is a `tuple[T, ...]`.
        ValueOf::new_unchecked(self.alloc_tuple_values(elems.into_iter().map(ValueOf::to_value)))
    }

    fn alloc_tuple_values(self, elems: impl Iterator<Item = Value<'fh>>) -> Value<'fh> {
        let (lower, upper) = elems.size_hint();
        if Some(lower) == upper {
            if lower == 0 {
                return Value::new_empty_tuple();
            }

            unsafe {
                let (v, extra) = self.alloc_raw_extra(frozen_tuple_avalue(lower));
                let extra = &mut *extra;
                maybe_uninit_write_from_exact_size_iter(extra, elems, Value::new_none());
                v.to_value()
            }
        } else {
            self.alloc_tuple_values(elems.collect::<Vec<_>>().into_iter())
        }
    }
}

impl<'v> Heap<'v> {
    /// Allocate a tuple with the given elements.
    pub(crate) fn alloc_tuple(self, elems: &[Value<'v>]) -> ValueOf<'v, TupleType<Value<'v>>> {
        // Every value is a `typing.Any`, so the tuple is a `tuple[typing.Any, ...]`.
        ValueOf::new_unchecked(self.alloc_tuple_values_from_slice(elems))
    }

    fn alloc_tuple_values_from_slice(self, elems: &[Value<'v>]) -> Value<'v> {
        if elems.is_empty() {
            return Value::new_empty_tuple();
        }

        unsafe {
            let (v, extra) = self.alloc_raw_extra(tuple_avalue(elems.len()));
            let extra = &mut *extra;
            maybe_uninit_write_slice(extra, elems);
            v.to_value()
        }
    }

    /// Allocate a tuple of these items, typed by them.
    #[inline]
    pub(crate) fn alloc_tuple_iter<T: StarlarkTypeExpr<'v>>(
        self,
        elems: impl IntoIterator<Item = ValueOf<'v, T>>,
    ) -> ValueOf<'v, TupleType<T>> {
        // The items are `T`s, so the tuple is a `tuple[T, ...]`.
        ValueOf::new_unchecked(self.alloc_tuple_values(elems.into_iter().map(ValueOf::to_value)))
    }

    fn alloc_tuple_values(self, elems: impl Iterator<Item = Value<'v>>) -> Value<'v> {
        let (lower, upper) = elems.size_hint();
        if Some(lower) == upper {
            if lower == 0 {
                return Value::new_empty_tuple();
            }

            unsafe {
                let (v, extra) = self.alloc_raw_extra(tuple_avalue(lower));
                let extra = &mut *extra;
                maybe_uninit_write_from_exact_size_iter(extra, elems, Value::new_none());
                v.to_value()
            }
        } else {
            self.alloc_tuple_values_from_slice(&elems.collect::<Vec<_>>())
        }
    }
}

// Register vtable for Tuple (special type not handled by #[starlark_value] macro).
register_special_avalue_frozen!(Tuple<'static>, AValueFrozenTuple);
