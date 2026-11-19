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

//! Heap layout of record instances: the record type and field count, followed
//! by the field values inline, like a tuple.

use allocative::Key;
use allocative::Visitor;

use crate::collections::maybe_uninit_backport::maybe_uninit_write_slice;
use crate::pagable::StarlarkSerialize;
use crate::pagable::vtable_register::register_special_avalue_frozen;
use crate::values::FreezeResult;
use crate::values::Freezer;
#[cfg(test)]
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::Tracer;
use crate::values::Value;
use crate::values::layout::avalue::AValue;
use crate::values::layout::avalue::AValueImpl;
use crate::values::layout::avalue::TrailingValues;
use crate::values::layout::avalue::heap_copy_with_extra;
use crate::values::layout::avalue::heap_freeze_with_extra;
use crate::values::layout::avalue::starlark_deserialize_with_extra;
use crate::values::layout::avalue::starlark_serialize_with_extra;
use crate::values::layout::heap::repr::AValueRepr;
use crate::values::record::Record;

fn record_avalue<'v>(typ: Value<'v>, len: usize) -> AValueImpl<'v, AValueRecord> {
    AValueImpl::<AValueRecord>::new(unsafe { Record::new_header(typ, len) })
}

#[cfg(test)]
fn frozen_record_avalue<'fv>(typ: Value<'fv>, len: usize) -> AValueImpl<'fv, AValueFrozenRecord> {
    AValueImpl::<AValueFrozenRecord>::new(unsafe { Record::new_header(typ, len) })
}

struct AValueRecord;

impl<'v> AValue<'v> for AValueRecord {
    type StarlarkValue = Record<'v>;

    type ExtraElem = Value<'v>;

    fn extra_len(value: &Record<'v>) -> usize {
        value.len()
    }

    fn offset_of_extra() -> usize {
        Record::offset_of_content()
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
            heap_freeze_with_extra::<Self, AValueFrozenRecord>(me, freezer, |x, freezer, len| {
                Ok(Record::new_header(freezer.freeze(x.typ)?, len))
            })
        }
    }

    unsafe fn heap_copy(
        me: *mut AValueRepr<Self::StarlarkValue>,
        tracer: &Tracer<'v>,
    ) -> Value<'v> {
        unsafe {
            heap_copy_with_extra::<Self>(me, tracer, |mut x, tracer| {
                tracer.trace(&mut x.typ);
                x
            })
        }
    }
}

struct AValueFrozenRecord;

impl<'v> AValue<'v> for AValueFrozenRecord {
    type StarlarkValue = Record<'v>;

    type ExtraElem = Value<'v>;

    fn extra_len(value: &Record<'v>) -> usize {
        value.len()
    }

    fn offset_of_extra() -> usize {
        Record::offset_of_content()
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
        starlark_serialize_with_extra::<Self>(me, ctx, |value, ctx| {
            StarlarkSerialize::starlark_serialize(&value.typ, ctx)
        })
    }

    fn starlark_deserialize<'fv>(
        me: *mut AValueRepr<Self::StarlarkValue>,
        ctx: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<()> {
        unsafe {
            starlark_deserialize_with_extra::<Self, Record<'fv>>(me, ctx, |ctx, len| {
                Ok(Record::new_header(ctx.deserialize_value()?, len))
            })
        }
    }
}

impl<'v> Heap<'v> {
    /// Allocate a record of type `typ` holding `content`, without checking it
    /// against the type; see [`Record::alloc`] for the checked form.
    pub(crate) fn alloc_record(self, typ: Value<'v>, content: &[Value<'v>]) -> Value<'v> {
        unsafe {
            let (v, extra) = self.alloc_raw_extra(record_avalue(typ, content.len()));
            let extra = &mut *extra;
            maybe_uninit_write_slice(extra, content);
            v.to_value()
        }
    }
}

#[cfg(test)]
impl<'fh> FrozenHeap<'fh> {
    /// Allocate a record of type `typ` holding `content`, without checking it
    /// against the type.
    pub(crate) fn alloc_record(self, typ: Value<'fh>, content: &[Value<'fh>]) -> Value<'fh> {
        unsafe {
            let (v, extra) = self.alloc_raw_extra(frozen_record_avalue(typ, content.len()));
            let extra = &mut *extra;
            maybe_uninit_write_slice(extra, content);
            v.to_value()
        }
    }
}

// Register vtable for Record (special type not handled by #[starlark_value] macro).
register_special_avalue_frozen!(Record<'static>, AValueFrozenRecord);
