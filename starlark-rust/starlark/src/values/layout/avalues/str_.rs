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
use std::ptr::copy_nonoverlapping;

use allocative::Key;
use allocative::Visitor;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use starlark_map::Hashed;

use crate::collections::StarlarkHashValue;
use crate::pagable::vtable_register::register_special_avalue_frozen;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::OwnedFrozen;
use crate::values::StringValue;
use crate::values::Tracer;
use crate::values::Value;
use crate::values::constant_string;
use crate::values::layout::avalue::AValue;
use crate::values::layout::avalue::AValueImpl;
use crate::values::layout::heap::repr::AValueHeader;
use crate::values::layout::heap::repr::AValueRepr;
use crate::values::layout::heap::repr::ForwardPtr;
use crate::values::string::str_type::StarlarkStr;

pub(crate) const VALUE_STR_A_VALUE_PTR: AValueHeader =
    AValueHeader::new_const::<StarlarkStrAValue>();

#[inline]
pub(crate) fn starlark_str<'v>(
    len: usize,
    hash: StarlarkHashValue,
) -> AValueImpl<'v, impl AValue<'v, ExtraElem = usize> + Send + Sync> {
    AValueImpl::<StarlarkStrAValue>::new(unsafe { StarlarkStr::new(len, hash) })
}

pub(crate) struct StarlarkStrAValue;

impl<'v> AValue<'v> for StarlarkStrAValue {
    type StarlarkValue = StarlarkStr;

    type ExtraElem = usize;

    fn extra_len(value: &StarlarkStr) -> usize {
        StarlarkStr::payload_len_for_len(value.len())
    }

    fn offset_of_extra() -> usize {
        StarlarkStr::offset_of_content()
    }

    const IS_STR: bool = true;

    fn visit_extra_allocative<'a, 'b: 'a>(
        value: &Self::StarlarkValue,
        visitor: &'a mut Visitor<'b>,
    ) {
        let content_size = value.as_str().len();
        visitor.visit_simple(Key::new("content"), content_size);

        let allocated_size =
            StarlarkStr::payload_len_for_len(value.len()) * mem::size_of::<usize>();
        visitor.visit_simple(Key::new("padding"), allocated_size - content_size);
    }

    unsafe fn heap_freeze<'fv>(
        me: *mut AValueRepr<Self::StarlarkValue>,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<Value<'fv>> {
        unsafe {
            debug_assert!(
                (*me).payload.len() > 1,
                "short strings are allocated statically"
            );

            let s = (*me).payload.as_str();
            let fv = freezer.frozen_heap().alloc_str(s).to_value();
            debug_assert!(fv.is_str());
            AValueHeader::overwrite_with_forward::<Self::StarlarkValue>(
                me,
                ForwardPtr::new_frozen(fv),
            );
            Ok(fv)
        }
    }

    unsafe fn heap_copy(
        me: *mut AValueRepr<Self::StarlarkValue>,
        tracer: &Tracer<'v>,
    ) -> Value<'v> {
        unsafe {
            debug_assert!(
                (*me).payload.len() > 1,
                "short strings are allocated statically"
            );

            let s = (*me).payload.as_str();
            let v = tracer.alloc_str(s);
            debug_assert!(v.is_str());
            AValueHeader::overwrite_with_forward::<Self::StarlarkValue>(
                me,
                ForwardPtr::new_unfrozen(v),
            );
            v
        }
    }

    fn starlark_serialize(
        me: *const AValueRepr<Self::StarlarkValue>,
        ctx: &mut dyn crate::pagable::StarlarkSerializeContext,
    ) -> crate::Result<()> {
        let value = unsafe { &(*me).payload };
        value.as_str().pagable_serialize(ctx.pagable())?;
        Ok(())
    }

    fn starlark_deserialize<'fv>(
        me: *mut AValueRepr<Self::StarlarkValue>,
        ctx: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<()> {
        let s = String::pagable_deserialize(ctx.pagable())?;
        let len = s.len();
        unsafe {
            ptr::write(
                &mut (*me).payload,
                StarlarkStr::new(len, StarlarkHashValue::new_unchecked(0)),
            );
            let extra_offset = AValueRepr::<Self::StarlarkValue>::offset_of_payload()
                + <Self as AValue>::offset_of_extra();
            let extra_ptr = (me as *mut u8).add(extra_offset) as *mut MaybeUninit<usize>;
            let payload_len = StarlarkStr::payload_len_for_len(len);
            // Zero the last word for padding.
            if payload_len > 0 {
                (*extra_ptr.add(payload_len - 1)).write(0usize);
            }
            // Copy string bytes.
            copy_nonoverlapping(s.as_ptr(), extra_ptr as *mut u8, len);
        }
        Ok(())
    }
}

// The string type's allocation primitives. Everything that is not the string type itself
// allocates through `heap.alloc(..)`: a `&str` or a `String`, or one of the recipes in
// `values::string`, which are how these are reached from outside the crate.

impl<'fh> FrozenHeap<'fh> {
    /// Allocate a string on this heap.
    ///
    /// Frozen heaps always intern: equal strings share one allocation, which is worth its lookup
    /// for strings that are kept for the heap's lifetime. [`Heap`] leaves the choice to the
    /// caller, see [`Heap::alloc_str`] and [`Heap::alloc_str_intern`].
    pub(crate) fn alloc_str(self, x: &str) -> StringValue<'fh> {
        self.alloc_str_hashed(Hashed::new(x))
    }

    /// Allocate prehashed string.
    pub(crate) fn alloc_str_hashed(self, s: Hashed<&str>) -> StringValue<'fh> {
        if let Some(s) = constant_string(*s) {
            return s.at();
        }
        let erased = self.string_interner().intern(s, || {
            let string = self.alloc_str_init(s.len(), s.hash(), |dest| unsafe {
                copy_nonoverlapping(s.as_ptr(), dest, s.len())
            });
            // SAFETY: Stored in this heap's interner, which lives exactly as long as the heap.
            unsafe { OwnedFrozen::<StringValue<'static>>::erase_brand(string) }
        });
        // SAFETY: Every entry of this heap's interner was allocated in this heap (just above),
        // and `'fh` is this heap's brand.
        unsafe { OwnedFrozen::<StringValue<'static>>::restore_brand(erased) }
    }
}

impl<'v> Heap<'v> {
    /// Allocate a string on the heap, without interning it; see [`Heap::alloc_str_intern`].
    pub(crate) fn alloc_str(self, x: &str) -> StringValue<'v> {
        if let Some(x) = constant_string(x) {
            x.at()
        } else {
            self.alloc_str_init(x.len(), StarlarkStr::UNINIT_HASH, |dest| unsafe {
                copy_nonoverlapping(x.as_ptr(), dest, x.len())
            })
        }
    }

    /// Allocate a string on the heap, sharing the allocation with any equal string interned on
    /// it before. Prefer this for strings that are allocated repeatedly and kept alive.
    /// [`FrozenHeap::alloc_str`] always interns.
    pub(crate) fn alloc_str_intern(self, x: &str) -> StringValue<'v> {
        if let Some(x) = constant_string(x) {
            x.at()
        } else {
            let x = Hashed::new(x);
            self.string_interner().intern(x, || {
                self.alloc_str_init(x.len(), x.hash(), |dest| unsafe {
                    copy_nonoverlapping(x.as_ptr(), dest, x.len())
                })
            })
        }
    }

    /// Allocate a string on the heap, based on two concatenated strings.
    pub(crate) fn alloc_str_concat(self, x: &str, y: &str) -> StringValue<'v> {
        if x.is_empty() {
            self.alloc_str(y)
        } else if y.is_empty() {
            self.alloc_str(x)
        } else {
            self.alloc_str_init(x.len() + y.len(), StarlarkStr::UNINIT_HASH, |dest| unsafe {
                copy_nonoverlapping(x.as_ptr(), dest, x.len());
                copy_nonoverlapping(y.as_ptr(), dest.add(x.len()), y.len())
            })
        }
    }

    /// Allocate a string on the heap, based on three concatenated strings.
    pub(crate) fn alloc_str_concat3(self, x: &str, y: &str, z: &str) -> StringValue<'v> {
        if x.is_empty() {
            self.alloc_str_concat(y, z)
        } else if y.is_empty() {
            self.alloc_str_concat(x, z)
        } else if z.is_empty() {
            self.alloc_str_concat(x, y)
        } else {
            self.alloc_str_init(
                x.len() + y.len() + z.len(),
                StarlarkStr::UNINIT_HASH,
                |dest| unsafe {
                    copy_nonoverlapping(x.as_ptr(), dest, x.len());
                    let dest = dest.add(x.len());
                    copy_nonoverlapping(y.as_ptr(), dest, y.len());
                    let dest = dest.add(y.len());
                    copy_nonoverlapping(z.as_ptr(), dest, z.len());
                },
            )
        }
    }

    pub(crate) fn alloc_char(self, x: char) -> StringValue<'v> {
        let mut dst = [0; 4];
        let res = x.encode_utf8(&mut dst);
        self.alloc_str(res)
    }
}

// Register vtable for StarlarkStr (special type not handled by #[starlark_value] macro).
register_special_avalue_frozen!(StarlarkStr, StarlarkStrAValue);
