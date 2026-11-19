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

use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::mem;
use std::ptr;
use std::ptr::NonNull;

use allocative::Allocative;
use serde::Serialize;
use serde::Serializer;
use starlark_derive::ProvidesStaticType;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::AnyLifetime;
use crate::private::Private;
use crate::values::StarlarkValue;
use crate::values::layout::avalues::static_::AValueBasic;
use crate::values::layout::pointer::RawPointer;
use crate::values::layout::vtable::AValueDyn;
use crate::values::layout::vtable::AValueVTable;
use crate::values::layout::vtable::StarlarkValueRawPtr;
use crate::values::types::int::inline_int::InlineInt;
use crate::values::types::int::view::IntView;

/// The result of calling `type()` on integers.
pub const INT_TYPE: &str = "int";

// WARNING: This type isn't a real type, a pointer to this is secretly an i32.
// Therefore, don't derive stuff on it, since it will be wrong.
// However, `ProvidesStaticType` promises not to peek at its value, so that's fine.
#[doc(hidden)]
#[derive(ProvidesStaticType, Allocative)]
#[repr(C)]
pub struct PointerI32 {
    _private: (),
}

impl PartialEq for PointerI32 {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self, other)
    }
}

impl Eq for PointerI32 {}

impl Debug for PointerI32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.get(), f)
    }
}

impl PointerI32 {
    const _ASSERTIONS: () = {
        assert!(mem::align_of::<Self>() == 1);
    };

    #[inline]
    pub(crate) unsafe fn from_raw_pointer_unchecked(
        raw_pointer: RawPointer,
    ) -> &'static PointerI32 {
        unsafe {
            debug_assert!(raw_pointer.is_int());
            // SAFETY: a zero-sized type of alignment 1 has a valid place at every non-null
            // address, and the pointer is not null because the `TAG_INT` bit is set. The
            // address encodes the integer; the reference never reads memory.
            raw_pointer.ptr().cast::<PointerI32>().as_ref()
        }
    }

    #[inline]
    pub(crate) fn get(&self) -> InlineInt {
        // SAFETY: a `PointerI32` only ever comes from `from_raw_pointer_unchecked`, so its
        // address encodes an integer.
        unsafe { RawPointer(NonNull::from(self).cast()).unpack_int_unchecked() }
    }

    /// A static is immortal, so it is usable at every brand; `&'static self` is the proof.
    #[inline]
    pub(crate) fn as_avalue_dyn<'v>(&'static self) -> AValueDyn<'v> {
        unsafe { AValueDyn::new(StarlarkValueRawPtr::new_pointer_i32(self), Self::vtable()) }
    }

    #[inline]
    pub(crate) fn vtable() -> &'static AValueVTable {
        AValueVTable::new::<AValueBasic<PointerI32>>()
    }

    pub(crate) fn type_is_pointer_i32<'v, T: StarlarkValue<'v>>() -> bool {
        T::static_type_id() == PointerI32::static_type_id()
    }
}

impl Display for PointerI32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.get())
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for PointerI32 {
    type Type = IntView<'v>;

    fn is_special(_: Private) -> bool
    where
        Self: Sized,
    {
        true
    }
}

impl Serialize for PointerI32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.get().serialize(serializer)
    }
}
