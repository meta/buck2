/*
 * Copyright 2018 The Starlark in Rust Authors.
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

//! Internal `Box<[T]>`-like single-word slice used to back
//! [`ThinBoxSliceValue`](super::packed_impl::ThinBoxSliceValue).
//!
//! Specifically:
//!  1. This type's word always carries one of the tags no `Value` does, so a word can hold
//!     either and be told apart by its low bits.
//!  2. This type is not implicitly dropped - `run_drop` must be called explicitly.

use std::alloc;
use std::alloc::Layout;
use std::fmt::Debug;
use std::hash::Hash;
use std::hash::Hasher;
use std::marker::PhantomData;
use std::mem;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::ops::DerefMut;
use std::ptr;
use std::ptr::NonNull;
use std::slice;

use allocative::Allocative;

use crate::pagable::StarlarkDeserialize;
use crate::pagable::StarlarkDeserializeContext;
use crate::pagable::StarlarkSerialize;
use crate::pagable::StarlarkSerializeContext;
use crate::values::layout::pointer::TAG_MASK;
use crate::values::layout::pointer::TAGS_NEVER_VALUE;

/// Aligned to `TAG_MASK + 1`, so that `data` starts at an address whose tag bits are clear on
/// every target; on 32-bit ones `T`'s own alignment does not guarantee that.
#[repr(C, align(8))]
struct ThinBoxSliceHeader {
    len: usize,
}
const _: () = assert!(mem::align_of::<ThinBoxSliceHeader>() == TAG_MASK + 1);

#[repr(C)]
struct ThinBoxSliceLayout<T> {
    header: ThinBoxSliceHeader,
    data: [T; 0],
}

impl<T> ThinBoxSliceLayout<T> {
    const fn offset_of_data() -> isize {
        // SAFETY: rust guarantees no allocated object can be larger than isize::MAX bytes.
        mem::offset_of!(ThinBoxSliceLayout::<T>, data) as isize
    }
}

/// Tag of a slice whose length is in the allocation's header, and of the empty slice.
const TAG_HEADER: usize = TAGS_NEVER_VALUE[0];
/// Tag of a slice of length two, which has no header: the tag is the length. Length zero is the
/// null address and length one never reaches this type (`PackedImpl` stores it inline). The
/// third never-value pattern is `Deferred`'s, so length three carries a header.
const TAG_LEN_2: usize = TAGS_NEVER_VALUE[1];

/// `Box<[T]>` but thin.
///
/// The word is the address of the first element with a tag in its low bits, so every allocation
/// is aligned to `TAG_MASK + 1`, whatever `T`'s own alignment (a `Value` is only word aligned on
/// 32-bit targets). For all but length two the tag is `TAG_HEADER` and the length sits in a
/// header before the elements; for length two the tag is the length and there is no header.
/// Skipping the header for short slices was measured at 0.8% wall time and 0.2% max RSS on a
/// large analysis (D66773980) when it covered lengths two through four, so changes to the
/// encoding should be benchmarked.
///
/// The current implementation returns what amounts to a null pointer for an
/// empty list. An alternative would be to return a valid pointer to a
/// statically allocated "long"-lengthed object with a length of 0. This would
/// reduce the number of representations, but testing at the time of this
/// writing shows that empty lists are common, and the pointer dereference in
/// reading the length causes a small performance hit. Changes in the future may
/// make this the preferred implementation.
#[repr(transparent)]
pub(super) struct AllocatedThinBoxSlice<T> {
    /// Tagged pointer to the first element, `ThinBoxSliceLayout.data`.
    ptr: *mut (),
    phantom: PhantomData<T>,
}

unsafe impl<T: Sync> Sync for AllocatedThinBoxSlice<T> {}
unsafe impl<T: Send> Send for AllocatedThinBoxSlice<T> {}

impl<T: StarlarkSerialize> StarlarkSerialize for AllocatedThinBoxSlice<T> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        let data: &[T] = self;
        data.len().starlark_serialize(ctx)?;
        for item in data {
            item.starlark_serialize(ctx)?;
        }
        Ok(())
    }
}

impl<'fv, T: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv> for AllocatedThinBoxSlice<T> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let len = usize::starlark_deserialize(ctx)?;
        let mut data = Vec::<T>::with_capacity(len);
        for _ in 0..len {
            data.push(T::starlark_deserialize(ctx)?);
        }
        Ok(Self::from_iter(data))
    }
}

impl<T> AllocatedThinBoxSlice<T> {
    #[inline]
    pub(super) const fn empty() -> AllocatedThinBoxSlice<T> {
        AllocatedThinBoxSlice {
            ptr: ptr::without_provenance_mut(TAG_HEADER),
            phantom: PhantomData,
        }
    }

    /// Whether `word` is the representation of one of these, as opposed to a `Value` or a
    /// `Deferred` field's unread word.
    #[inline]
    pub(super) fn is_word(word: usize) -> bool {
        let tag = word & TAG_MASK;
        tag == TAG_HEADER || tag == TAG_LEN_2
    }

    /// The word, which nothing frees unless the handle is rebuilt from it.
    #[inline]
    pub(super) const fn into_inner(self) -> *mut () {
        self.ptr
    }

    #[inline]
    const fn tag_for_len(len: usize) -> usize {
        match len {
            2 => TAG_LEN_2,
            _ => TAG_HEADER,
        }
    }

    /// Tag and allocation layout for a slice of length `len`.
    #[inline]
    fn layout_for_len(len: usize) -> (usize, Layout) {
        let tag = Self::tag_for_len(len);
        let layout = if tag == TAG_HEADER {
            let (layout, _offset_of_data) = Layout::new::<ThinBoxSliceLayout<T>>()
                .extend(Layout::array::<T>(len).unwrap())
                .unwrap();
            layout
        } else {
            Layout::array::<T>(len)
                .unwrap()
                .align_to(TAG_MASK + 1)
                .unwrap()
        };
        (tag, layout)
    }

    #[inline]
    fn tag(&self) -> usize {
        self.ptr.addr() & TAG_MASK
    }

    #[inline]
    fn as_ptr(&self) -> *mut T {
        self.ptr.map_addr(|addr| addr & !TAG_MASK).cast()
    }

    #[inline]
    fn as_nonnull_ptr(&self) -> *mut T {
        let ptr = self.as_ptr();
        if ptr.is_null() {
            NonNull::<T>::dangling().as_ptr()
        } else {
            ptr
        }
    }

    /// Length of the slice.
    // Not called `len` to avoid overload with `Deref::len`.
    #[inline]
    fn read_len(&self) -> usize {
        if self.as_ptr().is_null() {
            return 0;
        }

        match self.tag() {
            TAG_LEN_2 => 2,
            tag => {
                debug_assert!(tag == TAG_HEADER);
                unsafe {
                    (*self
                        .as_ptr()
                        .byte_offset(-ThinBoxSliceLayout::<T>::offset_of_data())
                        .cast::<ThinBoxSliceLayout<T>>())
                    .header
                    .len
                }
            }
        }
    }

    /// Allocate uninitialized memory for a slice of length `len`.
    #[inline]
    pub(super) fn new_uninit(len: usize) -> AllocatedThinBoxSlice<MaybeUninit<T>> {
        // Both layouts are aligned to `TAG_MASK + 1` and the header's data offset is a multiple
        // of it, which is what keeps the tag bits of every address clear. A zero-sized `T` would
        // make the header-less layouts zero-sized, which `alloc` does not accept.
        const { assert!(mem::size_of::<T>() != 0) };
        if len == 0 {
            AllocatedThinBoxSlice::empty()
        } else {
            let (tag, layout) = Self::layout_for_len(len);
            unsafe {
                let alloc = alloc::alloc(layout);
                if alloc.is_null() {
                    alloc::handle_alloc_error(layout);
                }
                let data_ptr: *mut () = if tag == TAG_HEADER {
                    let alloc = alloc as *mut ThinBoxSliceLayout<T>;
                    (*alloc).header.len = len;
                    alloc
                        .byte_offset(ThinBoxSliceLayout::<T>::offset_of_data())
                        .cast()
                } else {
                    alloc.cast()
                };
                debug_assert!(data_ptr.addr() & TAG_MASK == 0);
                AllocatedThinBoxSlice {
                    ptr: data_ptr.map_addr(|addr| addr | tag),
                    phantom: PhantomData,
                }
            }
        }
    }
}

impl<T> Deref for AllocatedThinBoxSlice<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { slice::from_raw_parts(self.as_nonnull_ptr(), self.read_len()) }
    }
}

impl<T> DerefMut for AllocatedThinBoxSlice<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { slice::from_raw_parts_mut(self.as_nonnull_ptr(), self.read_len()) }
    }
}

impl<T> AllocatedThinBoxSlice<MaybeUninit<T>> {
    #[inline]
    unsafe fn assume_init(self) -> AllocatedThinBoxSlice<T> {
        AllocatedThinBoxSlice {
            ptr: self.ptr,
            phantom: PhantomData,
        }
    }
}

impl<T> AllocatedThinBoxSlice<T> {
    #[inline]
    pub(super) fn run_drop(self) {
        unsafe {
            let len = self.read_len();
            if len != 0 {
                let slice = ptr::slice_from_raw_parts_mut(self.as_nonnull_ptr(), len);
                ptr::drop_in_place(slice);
                let mut alloc = self.as_ptr().cast::<u8>();
                let (tag, layout) = Self::layout_for_len(len);
                if tag == TAG_HEADER {
                    alloc = alloc.byte_offset(-ThinBoxSliceLayout::<T>::offset_of_data());
                }
                alloc::dealloc(alloc, layout);
            }
        }
    }
}

impl<T> Default for AllocatedThinBoxSlice<T> {
    #[inline]
    fn default() -> Self {
        AllocatedThinBoxSlice::empty()
    }
}

impl<T: Debug> Debug for AllocatedThinBoxSlice<T> {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <[T] as Debug>::fmt(&**self, f)
    }
}

impl<T: PartialEq> PartialEq for AllocatedThinBoxSlice<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        <[T] as PartialEq>::eq(&**self, &**other)
    }
}

impl<T: Eq> Eq for AllocatedThinBoxSlice<T> {}

impl<T: PartialOrd> PartialOrd for AllocatedThinBoxSlice<T> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        <[T] as PartialOrd>::partial_cmp(&**self, &**other)
    }
}

impl<T: Hash> Hash for AllocatedThinBoxSlice<T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        <[T] as Hash>::hash(&**self, state)
    }
}

impl<T> FromIterator<T> for AllocatedThinBoxSlice<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let (lower, upper) = iter.size_hint();
        if Some(lower) == upper {
            let mut thin = AllocatedThinBoxSlice::<T>::new_uninit(lower);
            let mut i = 0;
            for item in iter {
                assert!(i < lower, "iterator produced more than promised");
                MaybeUninit::write(&mut thin[i], item);
                i += 1;
            }
            assert_eq!(i, lower, "iterator produced less than promised");
            unsafe { thin.assume_init() }
        } else {
            // TODO(nga): we can collect into partially initialized `ThinBoxSlice`
            //   to get a chance of avoiding last reallocation.
            let vec = Vec::from_iter(iter);
            Self::from_iter(vec)
        }
    }
}

impl<T: Allocative> Allocative for AllocatedThinBoxSlice<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        {
            let ptr_key = allocative::Key::new("ptr");
            if self.is_empty() {
                // Statically allocated data, so just report the pointer itself
                visitor.visit_simple(ptr_key, mem::size_of_val(&self.ptr));
            } else {
                let mut visitor =
                    visitor.enter_unique(allocative::Key::new("ptr"), mem::size_of_val(&self.ptr));
                {
                    let (tag, layout) = Self::layout_for_len(self.len());
                    let mut visitor = visitor.enter(allocative::Key::new("alloc"), layout.size());

                    if tag == TAG_HEADER {
                        visitor.visit_simple(
                            allocative::Key::new("len"),
                            mem::size_of::<ThinBoxSliceHeader>(),
                        );
                    }
                    {
                        let mut visitor = visitor
                            .enter(allocative::Key::new("data"), mem::size_of_val::<[_]>(self));
                        visitor.visit_slice::<T>(self);
                        visitor.exit();
                    }
                    visitor.exit();
                }
                visitor.exit();
            }
        }
        visitor.exit();
    }
}

#[cfg(test)]
mod tests {
    use std::mem;

    use super::AllocatedThinBoxSlice;
    use super::TAG_HEADER;
    use super::ThinBoxSliceHeader;
    use crate::values::layout::pointer::TAG_MASK;
    use crate::values::layout::pointer::TAGS_NEVER_VALUE;

    #[test]
    fn test_empty() {
        let thin = AllocatedThinBoxSlice::<String>::empty();
        assert_eq!(0, thin.len());
        thin.run_drop();
    }

    #[test]
    fn test_from_iter_sized() {
        let thin =
            AllocatedThinBoxSlice::from_iter(["a".to_owned(), "bb".to_owned(), "ccc".to_owned()]);
        assert_eq!(["a".to_owned(), "bb".to_owned(), "ccc".to_owned()], *thin);
        thin.run_drop();
    }

    #[test]
    fn test_from_iter_unknown_size() {
        let thin = AllocatedThinBoxSlice::from_iter(
            ["a".to_owned(), "b".to_owned(), "c".to_owned()]
                .into_iter()
                .filter(|_| true),
        );
        assert_eq!(["a".to_owned(), "b".to_owned(), "c".to_owned()], *thin);
        thin.run_drop();
    }

    /// Length two has no header word; everything longer does.
    #[test]
    fn test_short_lengths_have_no_header() {
        let element = mem::size_of::<String>();
        let header = mem::size_of::<ThinBoxSliceHeader>();
        for len in 0..8 {
            let (tag, layout) = AllocatedThinBoxSlice::<String>::layout_for_len(len);
            let expected = if len == 2 {
                assert_ne!(tag, TAG_HEADER);
                len * element
            } else {
                assert_eq!(tag, TAG_HEADER);
                header + len * element
            };
            assert_eq!(expected, layout.size(), "len {len}");
        }
    }

    /// Every word this type produces is recognizable as one of its own.
    #[test]
    fn test_words_are_tagged() {
        for i in 0..8 {
            let thin = AllocatedThinBoxSlice::from_iter((0..i).map(|j| j.to_string()));
            let word = thin.ptr.addr();
            assert!(AllocatedThinBoxSlice::<String>::is_word(word), "len {i}");
            assert_eq!(word & !TAG_MASK == 0, i == 0, "len {i}");
            thin.run_drop();
        }
    }

    /// The third never-value pattern is left to `Deferred`: a word carrying it is not a slice.
    #[test]
    fn test_deferred_word_is_not_a_slice() {
        for body in [0usize, 8, 1 << 20] {
            assert!(!AllocatedThinBoxSlice::<String>::is_word(
                body | TAGS_NEVER_VALUE[2]
            ));
        }
    }

    /// If there are obvious memory violations, this test will catch them.
    #[test]
    fn test_stress() {
        for i in 0..1000 {
            let thin = AllocatedThinBoxSlice::from_iter((0..i).map(|j| j.to_string()));
            assert_eq!(i, thin.len());
            assert_eq!((0..i).map(|j| j.to_string()).collect::<Vec<_>>(), *thin);
            thin.run_drop();
        }
    }
}
