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

use std::any::type_name;
use std::marker::PhantomData;
use std::mem;

use crate::private::Private;
use crate::values::FreezeError;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::Heap;
use crate::values::HeapSendable;
use crate::values::StarlarkValue;
use crate::values::Trace;
use crate::values::Tracer;
use crate::values::Value;
use crate::values::layout::avalue::AValue;
use crate::values::layout::avalue::AValueImpl;
use crate::values::layout::avalue::heap_copy_impl;
use crate::values::layout::heap::repr::AValueRepr;

#[derive(Debug, thiserror::Error)]
enum AValueError {
    #[error("Value of type `{0}` cannot be frozen")]
    CannotBeFrozen(&'static str),
}

pub(crate) struct AValueComplexNoFreeze<T>(PhantomData<T>);

impl<'v, T> AValue<'v> for AValueComplexNoFreeze<T>
where
    T: StarlarkValue<'v> + Trace<'v>,
{
    type StarlarkValue = T;

    type ExtraElem = ();

    fn extra_len(_value: &T) -> usize {
        0
    }

    fn offset_of_extra() -> usize {
        mem::size_of::<Self::StarlarkValue>()
    }

    unsafe fn heap_freeze<'fv>(
        _me: *mut AValueRepr<Self::StarlarkValue>,
        _freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<Value<'fv>> {
        Err(FreezeError::new(
            AValueError::CannotBeFrozen(type_name::<T>()).to_string(),
        ))
    }

    unsafe fn heap_copy(
        me: *mut AValueRepr<Self::StarlarkValue>,
        tracer: &Tracer<'v>,
    ) -> Value<'v> {
        unsafe { heap_copy_impl::<Self>(me, tracer, Trace::trace) }
    }
}

impl<'v> Heap<'v> {
    /// Allocate a value which can be traced (garbage collected), but cannot be frozen.
    pub(crate) fn alloc_complex_no_freeze<T>(self, x: T) -> Value<'v>
    where
        T: StarlarkValue<'v> + Trace<'v>,
        T: HeapSendable<'v>,
    {
        assert!(!T::is_special(Private));
        self.alloc_raw(AValueImpl::<AValueComplexNoFreeze<T>>::new(x))
            .to_value()
    }
}

// The fixtures deliberately model one Starlark-visible type with multiple
// frozen Rust representations sharing a single canonical type, the shape
// runtime-selected freezing exists for (see buck2's `cmd_args`).
