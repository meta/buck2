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

// We use pointer tagging on the bottom three bits:
// 000 => frozen pointer
// 001 => unfrozen pointer
// 010 => int (32 bit)
// 100 => frozen string
// 101 => unfrozen string
// The remaining three patterns are never a `Value`; see `TAGS_NEVER_VALUE`.

// We group our bytes based on the tag info, not traditional alignment.
#![allow(clippy::unusual_byte_groupings)]

use std::fmt::Debug;
use std::marker::PhantomData;
use std::mem;
use std::num::NonZero;
use std::ptr::NonNull;

use allocative::Allocative;
use dupe::Dupe;
use either::Either;
use static_assertions::assert_eq_size;

use crate::values::int::pointer_i32::PointerI32;
use crate::values::layout::heap::repr::AValueHeader;
use crate::values::layout::heap::repr::AValueHeaderPtr;
use crate::values::layout::heap::repr::AValueHeapEntry;
use crate::values::types::int::inline_int::InlineInt;

/// Tagged pointer logically equivalent to `*mut AValueHeader`.
///
/// The tags live in the low bits of the address and the pointer keeps the provenance of the
/// value it was made from, so untagging it gives a pointer that may be dereferenced; an inline
/// integer is a pointer without provenance that is never dereferenced.
///
/// Transparent so that `Value`, which is transparent over it, has the layout of a non-null
/// pointer: `Deferred` reinterprets a word as a `Value` on that basis.
#[derive(Clone, Copy, Dupe, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub(crate) struct RawPointer(pub(crate) NonNull<()>);

// SAFETY: Nothing dereferences a `RawPointer` directly; what it points at is reached through
// `Pointer`, which is neither `Send` nor `Sync`. As far as its own API goes it is an address.
unsafe impl Send for RawPointer {}
unsafe impl Sync for RawPointer {}

impl Allocative for RawPointer {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        visitor.visit_simple_sized::<Self>();
    }
}

impl Debug for RawPointer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RawPointer")
            .field(&format_args!("0x{:x}", self.ptr_value()))
            .finish()
    }
}

impl RawPointer {
    /// `ptr`, whose tag bits must be clear, with `tag` in them.
    #[inline]
    fn tagged(ptr: NonNull<()>, tag: usize) -> RawPointer {
        debug_assert!(ptr.addr().get() & TAG_MASK == 0);
        debug_assert!(tag & !TAG_MASK == 0);
        let ptr = RawPointer(ptr.map_addr(|addr| addr | tag));

        // Run debug assertions.
        let _ignore = PointerTags::from_pointer(ptr);

        ptr
    }

    #[inline]
    pub(crate) fn new_int(i: InlineInt) -> RawPointer {
        let word = ((i.to_i32() as isize) << INT_SHIFT) as usize | TAG_INT;
        // SAFETY: `TAG_INT` is not zero.
        let word = unsafe { NonZero::new_unchecked(word) };
        RawPointer(NonNull::without_provenance(word))
    }

    #[inline]
    pub(crate) fn new_unfrozen(ptr: NonNull<AValueHeader>, is_string: bool) -> RawPointer {
        let tag = if is_string { TAG_STR } else { 0 };
        Self::tagged(ptr.cast(), tag | TAG_UNFROZEN)
    }

    #[inline]
    pub(crate) fn new_frozen(ptr: NonNull<AValueHeader>, is_string: bool) -> RawPointer {
        let tag = if is_string { TAG_STR } else { 0 };
        Self::tagged(ptr.cast(), tag)
    }

    /// The pointer, tags included; not for dereferencing.
    #[inline]
    pub(crate) fn ptr(self) -> NonNull<()> {
        self.0
    }

    /// The address, tags included.
    #[inline]
    pub(crate) fn ptr_value(self) -> usize {
        self.0.addr().get()
    }

    /// Get the pointer value with tag bits stripped.
    /// Returns the raw address suitable for comparison with header addresses.
    #[inline]
    #[cfg(all(test, feature = "pagable"))]
    pub(crate) fn ptr_value_untagged(self) -> usize {
        self.ptr_value() & !TAG_MASK
    }

    #[inline]
    pub(crate) fn tags(self) -> PointerTags {
        PointerTags::from_pointer(self)
    }

    #[inline]
    pub(crate) fn is_str(self) -> bool {
        self.tags().is_str()
    }

    #[inline]
    pub(crate) fn is_int(self) -> bool {
        self.tags().is_int()
    }

    #[inline]
    pub(crate) fn is_unfrozen(self) -> bool {
        self.tags().is_unfrozen()
    }

    #[inline]
    pub(crate) fn unpack_int(self) -> Option<InlineInt> {
        if !self.is_int() {
            None
        } else {
            unsafe { Some(self.unpack_int_unchecked()) }
        }
    }

    #[inline]
    pub(crate) unsafe fn unpack_pointer_i32_unchecked(self) -> &'static PointerI32 {
        unsafe {
            debug_assert!(self.is_int());
            debug_assert!(self.ptr_value() & !INT_DATA_MASK == TAG_INT);

            PointerI32::from_raw_pointer_unchecked(self)
        }
    }

    /// Unpack integer when it is known to be not a pointer.
    #[inline]
    pub(crate) unsafe fn unpack_int_unchecked(self) -> InlineInt {
        debug_assert!(self.is_int());
        debug_assert!(self.ptr_value() & !INT_DATA_MASK == TAG_INT);

        InlineInt::new_unchecked(((self.ptr_value() as isize) >> INT_SHIFT) as i32)
    }

    /// # Safety
    ///
    /// `self` must encode a pointer, not an inline integer. Before dereferencing the returned
    /// pointer, the caller must ensure that it points to a live, properly aligned allocation
    /// containing a valid `T`.
    #[inline]
    pub(crate) unsafe fn unpack_ptr_no_int_unchecked_raw<T>(self) -> *const T {
        debug_assert!(!self.is_int());
        self.0
            .as_ptr()
            .map_addr(|addr| addr & !(TAG_STR | TAG_UNFROZEN))
            .cast()
    }

    /// # Safety
    ///
    /// `self` must encode a pointer to a live `AValueOrForward`, not an inline integer. The
    /// allocation must remain valid for `'v`.
    #[inline]
    pub(crate) unsafe fn unpack_ptr_no_int_unchecked<'v>(self) -> &'v AValueHeapEntry {
        unsafe { &*self.unpack_ptr_no_int_unchecked_raw() }
    }

    /// The heap entry, by this pointer: the untagged value pointer, with its provenance.
    ///
    /// # Safety
    ///
    /// `self` must encode a pointer, not an inline integer.
    #[inline]
    pub(crate) unsafe fn heap_entry_ptr(self) -> NonNull<AValueHeapEntry> {
        let ptr = unsafe { self.unpack_ptr_no_int_unchecked_raw::<AValueHeapEntry>() };
        // SAFETY: untagging a non-null pointer leaves it non-null.
        unsafe { NonNull::new_unchecked(ptr.cast_mut()) }
    }
}

// A tagged pointer to a value, branded with the heap it belongs to.
#[derive(Clone, Copy, Dupe)]
#[repr(transparent)]
pub(crate) struct Pointer<'p> {
    ptr: RawPointer,
    // Make sure we are invariant in all the types/lifetimes, and are not `Send` or `Sync`
    // See https://stackoverflow.com/questions/62659221/why-does-a-program-compile-despite-an-apparent-lifetime-mismatch
    _phantom: PhantomData<*mut &'p ()>,
}

assert_eq_size!(Pointer<'static>, usize);
assert_eq_size!(Option<Pointer<'static>>, usize);

#[allow(dead_code)] // False positive.
const TAG_BITS: usize = 3;
pub(crate) const TAG_MASK: usize = 0b111;
#[allow(clippy::assertions_on_constants)]
const _: () = assert!(TAG_MASK == (1 << TAG_BITS) - 1);

const TAG_INT: usize = 0b010;
const TAG_STR: usize = 0b100;
// Pointer to an object, which is not frozen.
// Note, an object can be changed from unfrozen to frozen, not vice versa.
pub(crate) const TAG_UNFROZEN: usize = 0b001;

/// All possible tag values, three least significant bits of a pointer.
#[repr(usize)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) enum PointerTags {
    Int = TAG_INT,
    StrUnfrozen = TAG_STR | TAG_UNFROZEN,
    StrFrozen = TAG_STR,
    OtherUnfrozen = TAG_UNFROZEN,
    OtherFrozen = 0,
}

impl PointerTags {
    #[inline]
    unsafe fn from_usize_unchecked(x: usize) -> Self {
        debug_assert!(
            x == PointerTags::Int as usize
                || x == PointerTags::StrUnfrozen as usize
                || x == PointerTags::StrFrozen as usize
                || x == PointerTags::OtherUnfrozen as usize
                || x == PointerTags::OtherFrozen as usize
        );
        unsafe { mem::transmute(x) }
    }

    #[inline]
    fn from_pointer(ptr: RawPointer) -> Self {
        unsafe { Self::from_usize_unchecked(ptr.ptr_value() & TAG_MASK) }
    }

    #[inline]
    fn to_usize(self) -> usize {
        self as usize
    }

    /// String value, frozen or not.
    #[inline]
    fn is_str(self) -> bool {
        self.to_usize() & TAG_STR != 0
    }

    /// Inline integer.
    #[inline]
    fn is_int(self) -> bool {
        self == PointerTags::Int
    }

    /// Not frozen, not an integer.
    #[inline]
    fn is_unfrozen(self) -> bool {
        self.to_usize() & TAG_UNFROZEN != 0
    }
}

/// The tag patterns no `Value` carries. Anything that must share a word with a `Value` and be
/// told apart from one by the low bits alone tags itself with these: the first two belong to
/// `ThinBoxSliceValue`'s out-of-line storage, the third to `Deferred`.
pub(crate) const TAGS_NEVER_VALUE: [usize; 3] = [0b011, 0b110, 0b111];
/// The word of a `Deferred` field that has not been read yet. Never a `Value` and never a
/// `ThinBoxSliceValue`, so an unread field of either type is told apart by this pattern alone.
pub(crate) const TAG_DEFERRED: usize = TAGS_NEVER_VALUE[2];

// `PointerTags` and `TAGS_NEVER_VALUE` partition the tag space, so a new `Value` encoding has to
// take its pattern from the latter and move that pattern's users off it.
const _: () = {
    // One bit per tag pattern.
    type TagSet = u8;
    assert!(TagSet::BITS as usize == 1 << TAG_BITS);
    const fn bit(tag: usize) -> TagSet {
        assert!(tag & TAG_MASK == tag);
        1 << tag
    }

    let value_tags = bit(PointerTags::Int as usize)
        | bit(PointerTags::StrUnfrozen as usize)
        | bit(PointerTags::StrFrozen as usize)
        | bit(PointerTags::OtherUnfrozen as usize)
        | bit(PointerTags::OtherFrozen as usize);
    let mut never_value_tags: TagSet = 0;
    let mut i = 0;
    while i < TAGS_NEVER_VALUE.len() {
        never_value_tags |= bit(TAGS_NEVER_VALUE[i]);
        i += 1;
    }
    assert!(never_value_tags.count_ones() as usize == TAGS_NEVER_VALUE.len());
    assert!(value_tags & never_value_tags == 0);
    assert!(value_tags | never_value_tags == TagSet::MAX);
};

/// `InlineInt` is shift by this number of bits to the left to be stored in a pointer.
const INT_SHIFT: usize = mem::size_of::<usize>() * 8 - InlineInt::BITS;
const INT_DATA_MASK: usize = ((1usize << InlineInt::BITS) - 1) << INT_SHIFT;
#[allow(clippy::assertions_on_constants)]
const _: () = assert!(INT_SHIFT >= TAG_BITS);

impl<'p> Pointer<'p> {
    #[inline]
    unsafe fn new(ptr: RawPointer) -> Pointer<'p> {
        Pointer {
            ptr,
            _phantom: PhantomData,
        }
    }

    /// `x` is a header pointer with the string tag set or clear and the other tag bits clear.
    #[inline]
    pub(crate) unsafe fn new_unfrozen_ptr_with_str_tag(x: NonNull<()>) -> Self {
        debug_assert!((x.addr().get() & TAG_MASK & !TAG_STR) == 0);
        unsafe { Self::new(RawPointer(x.map_addr(|addr| addr | TAG_UNFROZEN))) }
    }

    /// `x` is a header pointer with the string tag set or clear and the other tag bits clear.
    #[inline]
    pub(crate) fn new_frozen_ptr_with_str_tag(x: NonNull<()>) -> Self {
        debug_assert!((x.addr().get() & TAG_MASK & !TAG_STR) == 0);
        unsafe { Self::new(RawPointer(x)) }
    }

    /// # Safety
    ///
    /// `x` is the header of a value in the unfrozen heap of `'p`, by a pointer with provenance
    /// over the whole value.
    #[inline]
    pub(crate) unsafe fn new_unfrozen(x: NonNull<AValueHeader>, is_string: bool) -> Self {
        unsafe { Self::new(RawPointer::new_unfrozen(x, is_string)) }
    }

    /// # Safety
    ///
    /// `x` is the header of a value in a frozen heap that the heap of `'p` keeps alive, or of a
    /// static, by a pointer with provenance over the whole value.
    #[inline]
    pub(crate) unsafe fn new_frozen(x: NonNull<AValueHeader>, is_str: bool) -> Self {
        unsafe { Self::new(RawPointer::new_frozen(x, is_str)) }
    }

    #[inline]
    pub(crate) fn new_int(x: InlineInt) -> Self {
        unsafe { Self::new(RawPointer::new_int(x)) }
    }

    #[inline]
    pub(crate) fn is_str(self) -> bool {
        self.ptr.is_str()
    }

    #[inline]
    pub(crate) fn is_unfrozen(self) -> bool {
        self.ptr.is_unfrozen()
    }

    /// The value's header, by the value's own pointer, or its inline integer. No live `Value`
    /// points at a forward or a reservation; `AValueHeaderPtr::unpack` checks in debug builds.
    #[inline]
    pub(crate) fn unpack(self) -> Either<AValueHeaderPtr<'p>, &'static PointerI32> {
        if !self.ptr.is_int() {
            // SAFETY: a `Value` that is not an integer points at a value in a heap of `'p`.
            Either::Left(unsafe { AValueHeaderPtr::new(self.ptr.heap_entry_ptr().cast()) })
        } else {
            Either::Right(unsafe { PointerI32::from_raw_pointer_unchecked(self.ptr) })
        }
    }

    /// The header of a live value, by the value's own pointer.
    ///
    /// # Safety
    ///
    /// `self` points at a live value: not an inline integer, a forward or a reservation.
    #[inline]
    pub(crate) unsafe fn value_header_ptr_unchecked(self) -> AValueHeaderPtr<'p> {
        unsafe {
            debug_assert!(!self.ptr.is_int());
            let entry = self.ptr.heap_entry_ptr();
            debug_assert!(entry.as_ref().is_value());
            AValueHeaderPtr::new(entry.cast())
        }
    }

    #[inline]
    pub(crate) fn unpack_int(self) -> Option<InlineInt> {
        self.ptr.unpack_int()
    }

    #[inline]
    pub(crate) fn unpack_ptr(self) -> Option<&'p AValueHeapEntry> {
        if !self.ptr.is_int() {
            Some(unsafe { self.ptr.unpack_ptr_no_int_unchecked() })
        } else {
            None
        }
    }

    /// Unpack integer when it is known to be not a pointer.
    #[inline]
    pub(crate) unsafe fn unpack_pointer_i32_unchecked(self) -> &'static PointerI32 {
        unsafe { self.ptr.unpack_pointer_i32_unchecked() }
    }

    #[inline]
    pub(crate) fn ptr_eq(self, other: Pointer<'_>) -> bool {
        self.ptr == other.ptr
    }

    #[inline]
    pub(crate) fn raw(self) -> RawPointer {
        self.ptr
    }

    #[inline]
    pub(crate) unsafe fn cast_lifetime<'p2>(self) -> Pointer<'p2> {
        Pointer {
            ptr: self.ptr,
            _phantom: PhantomData,
        }
    }
}

#[cfg(test)]
#[test]
fn test_int_tag() {
    fn check(x: InlineInt) {
        assert_eq!(x, RawPointer::new_int(x).unpack_int().unwrap());
    }

    for x in -10..10 {
        check(InlineInt::try_from(x).ok().unwrap());
    }
    check(InlineInt::MAX);
    check(InlineInt::MIN);
}
