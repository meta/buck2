/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Provides [`MiniVec`].

mod drain;
mod impls;
mod into_iter;

use std::alloc;
use std::alloc::Layout;
use std::cmp;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::ptr::NonNull;
use std::slice;

use allocative::Allocative;
pub use drain::Drain;
pub use into_iter::IntoIter;

use crate::packed_ptr::PackedPtr;

/// Capacity-nibble value that signals extended mode (length and capacity stored
/// in the heap header rather than in the packed pointer).
const EXT_SENTINEL: u8 = 0xF;

/// Maximum capacity that can be encoded inline.
const MAX_INLINE_CAP: usize = (EXT_SENTINEL as usize) - 1;

// The cap/len metadata occupies the single byte `PackedPtr` stores in the
// high 8 bits of the pointer, split into two nibbles: the capacity in the
// low nibble (where `EXT_SENTINEL` flags extended mode) and the length in
// the high nibble.

/// Combine the capacity nibble (low) and length nibble (high) into the
/// single metadata byte stored by [`PackedPtr`].
#[inline]
fn pack_meta(cap_nibble: u8, len_nibble: u8) -> u8 {
    debug_assert!(cap_nibble <= 0xF && len_nibble <= 0xF);
    (len_nibble << 4) | cap_nibble
}

/// Capacity nibble (low nibble of the [`PackedPtr`] metadata byte).
#[inline]
fn cap_nibble_of(extra: u8) -> u8 {
    extra & 0xF
}

/// Length nibble (high nibble of the [`PackedPtr`] metadata byte).
#[inline]
fn len_nibble_of(extra: u8) -> u8 {
    extra >> 4
}

/// In-allocation header used in extended mode.
#[repr(C)]
struct Header {
    len: usize,
    cap: usize,
}

/// A `Vec`-like sequence whose stack footprint is a single machine word.
///
/// `MiniVec<T>` is semantically equivalent to [`Vec<T>`], but built on top of `PackedPtr` to reduce
/// stack usage.
///
/// When the capacity of a `MiniVec` is less than 15, the capacity and length are stored directly in
/// the `PackedPtr` and the heap layout is exactly identical to a `Vec`. When the capacity reaches
/// 15, there is no longer enough bits available to store the capacity and length inline; thus,
/// `MiniVec` reverts to storing these values as a header in the heap allocation.
///
/// The overall effect of this is that `MiniVec`s memory usage has better constants that can make a
/// significant difference in programs with large numbers of vecs with few elements each. However,
/// some common operations such as a deref or `.len()` are a bit more complex and thus may be
/// slower; whether this matters is extremely workload dependent.
pub struct MiniVec<T> {
    /// Packed pointer + cap/len nibbles in the high 8 bits.
    raw: PackedPtr<NonNull<u8>>,
    _marker: PhantomData<T>,
}

// SAFETY: `MiniVec<T>` owns its `T`s exactly like `Vec<T>` does.
unsafe impl<T: Send> Send for MiniVec<T> {}
// SAFETY: `MiniVec<T>` aliases its `T`s through `&self`/`&mut self` exactly
// like `Vec<T>` does.
unsafe impl<T: Sync> Sync for MiniVec<T> {}
// `MiniVec`s are not structurally pinned
impl<T> Unpin for MiniVec<T> {}

struct Unpacked<T> {
    /// Pointer to the first element. May be dangling when `cap == 0`.
    elements: NonNull<T>,
    len: usize,
    cap: usize,
    /// Pointer to the heap allocation start in extended mode; `None` in inline
    /// mode. Used for deallocation.
    header: Option<NonNull<Header>>,
}

// Hand-written so the derived `T: Clone + Copy` bounds aren't added — the
// fields are all `Copy` regardless of `T`.
impl<T> Copy for Unpacked<T> {}
impl<T> Clone for Unpacked<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> MiniVec<T> {
    /// Compile-time refusal of zero-sized types.
    ///
    /// Supporting ZSTs would require either always allocating a header (to
    /// hold the otherwise-implicit length) or a separate code path; neither
    /// matches the goal of being a thin pointer-packed `Vec`. Use `Vec<T>`
    /// for zero-sized element types.
    const _ASSERT_NOT_ZST: () = assert!(
        size_of::<T>() != 0,
        "`MiniVec` does not support zero-sized types",
    );

    /// Construct an empty `MiniVec` without allocating.
    #[inline]
    pub const fn new() -> Self {
        let _ = Self::_ASSERT_NOT_ZST;
        // `NonNull::<T>::dangling()` returns a non-null, properly aligned
        // pointer at `align_of::<T>()`. That address fits trivially in the
        // low 56 bits, so the high 8 bits — i.e. the inline cap and len
        // nibbles — are zero, which is exactly the empty/unallocated state.
        let dangling: NonNull<T> = NonNull::dangling();
        // SAFETY: `dangling.cast::<u8>()` is a valid `NonNull::into_pointer()`
        // result for `NonNull<u8>` (it *is* a `NonNull<u8>`), and zero metadata
        // is exactly the empty/unallocated state that `PackedPtr::new(dangling
        // .cast(), 0)` would produce. The dangling address has no high bits set,
        // satisfying `from_ptr_zero_extra`'s packed-representation requirement.
        let raw = unsafe { PackedPtr::from_ptr_zero_extra(dangling.cast::<u8>()) };
        MiniVec {
            raw,
            _marker: PhantomData,
        }
    }

    /// Construct a `MiniVec` with at least the given capacity. Allocates if
    /// `cap > 0`.
    pub fn with_capacity(cap: usize) -> Self {
        let _ = Self::_ASSERT_NOT_ZST;
        let mut v = Self::new();
        if cap > 0 {
            v.realloc(cap);
        }
        v
    }

    /// Number of elements stored.
    #[inline]
    pub fn len(&self) -> usize {
        let extra = self.raw.extra();
        let cap_nibble = cap_nibble_of(extra);
        if cap_nibble == EXT_SENTINEL {
            // SAFETY: `cap_nibble == EXT_SENTINEL` means we are in extended
            // mode, so the packed pointer points to a valid `Header`
            // written by `realloc` when promotion happened, and the
            // allocation is alive because `self` owns it.
            unsafe {
                let header = self.header_ptr_unchecked();
                (*header.as_ptr()).len
            }
        } else {
            len_nibble_of(extra) as usize
        }
    }

    /// Total number of elements the underlying allocation can hold without
    /// reallocating.
    #[inline]
    pub fn capacity(&self) -> usize {
        let cap_nibble = cap_nibble_of(self.raw.extra());
        if cap_nibble == EXT_SENTINEL {
            // SAFETY: same as in `len`.
            unsafe {
                let header = self.header_ptr_unchecked();
                (*header.as_ptr()).cap
            }
        } else {
            cap_nibble as usize
        }
    }

    /// `true` iff [`len`](Self::len) is zero.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `true` iff length and capacity live in the heap header rather than
    /// the packed pointer.
    ///
    /// Exposed primarily so tests and diagnostics can observe the mode
    /// transition; ordinary callers should not need it.
    #[inline]
    pub fn is_extended(&self) -> bool {
        cap_nibble_of(self.raw.extra()) == EXT_SENTINEL
    }

    /// Borrow the elements as a slice.
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        let unpacked = self.unpack();
        // SAFETY: `unpack` returns a pointer to `len` initialized, properly
        // aligned `T`s in memory we own; the borrow is tied to `&self`.
        unsafe { slice::from_raw_parts(unpacked.elements.as_ptr(), unpacked.len) }
    }

    /// Mutably borrow the elements as a slice.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        let unpacked = self.unpack();
        // SAFETY: same as `as_slice`, plus exclusivity from `&mut self`.
        unsafe { slice::from_raw_parts_mut(unpacked.elements.as_ptr(), unpacked.len) }
    }

    /// Pointer to the first element.
    #[inline]
    pub fn as_ptr(&self) -> *const T {
        self.unpack().elements.as_ptr()
    }

    /// Mutable pointer to the first element.
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.unpack().elements.as_ptr()
    }

    /// Append `value` to the end, growing the allocation if necessary.
    pub fn push(&mut self, value: T) {
        let mut unpacked = self.unpack();
        if unpacked.len == unpacked.cap {
            self.grow_one();
            unpacked = self.unpack();
        }
        // SAFETY: there is now space for one more element at index `len`,
        // and we have exclusive access via `&mut self`.
        unsafe {
            ptr::write(unpacked.elements.as_ptr().add(unpacked.len), value);
        }
        // SAFETY: `len + 1 <= cap` because of the grow above (or because
        // `len < cap` already), and the slot is now initialized.
        unsafe {
            self.set_len(unpacked.len + 1);
        }
    }

    /// Remove and return the last element, or `None` if the vector is empty.
    pub fn pop(&mut self) -> Option<T> {
        let unpacked = self.unpack();
        if unpacked.len == 0 {
            return None;
        }
        let new_len = unpacked.len - 1;
        // SAFETY: element at `new_len` is initialized; we are taking
        // ownership of it. The `set_len` below makes it logically gone, so
        // it won't be dropped twice when `self` is dropped.
        let value = unsafe { ptr::read(unpacked.elements.as_ptr().add(new_len)) };
        // SAFETY: `new_len < len <= cap`, and elements `[0..new_len]` are
        // still initialized.
        unsafe {
            self.set_len(new_len);
        }
        Some(value)
    }

    /// Drop all elements but keep the allocation.
    pub fn clear(&mut self) {
        self.truncate(0);
    }

    /// Insert `value` at index `idx`, shifting all elements at indices
    /// `>= idx` one position to the right. Panics if `idx > len()`.
    pub fn insert(&mut self, idx: usize, value: T) {
        let len = self.len();
        assert!(idx <= len, "insertion index (is {idx}) > len (is {len})");
        if len == self.capacity() {
            self.grow_one();
        }
        let unpacked = self.unpack();
        // SAFETY:
        // - `idx <= len <= cap`, so `elements + idx` is in bounds.
        // - After `grow_one`, there is capacity for one more element, so
        //   `elements + len` is also in bounds.
        // - `ptr::copy` allows source/destination overlap.
        unsafe {
            let p = unpacked.elements.as_ptr().add(idx);
            ptr::copy(p, p.add(1), len - idx);
            ptr::write(p, value);
            self.set_len(len + 1);
        }
    }

    /// Remove consecutive duplicate elements, leaving only the first of
    /// each run.
    ///
    /// Equivalent to [`Vec::dedup`].
    pub fn dedup(&mut self)
    where
        T: PartialEq,
    {
        let len = self.len();
        if len <= 1 {
            return;
        }
        // Initially, `set_len` 0 so that if we panic anywhere below things only leak
        unsafe {
            self.set_len(0);
        }
        let unpacked = self.unpack();
        let base = unpacked.elements.as_ptr();
        let mut write: usize = 1;
        let mut read: usize = 1;
        while read < len {
            // SAFETY: `read < len <= cap`, and `write - 1 < write <= read`,
            // so both pointers reference initialized elements within the
            // allocation. The two pointers are distinct (proof: at the top
            // of every iteration `write <= read`, and `write - 1 < read`).
            unsafe {
                let read_p = base.add(read);
                let prev_p = base.add(write - 1);
                if *read_p == *prev_p {
                    // Duplicate: drop and skip. The slot at `read` will
                    // not be reused (its bytes will sit beyond the new
                    // length and `set_len` below excludes them from
                    // future drops).
                    ptr::drop_in_place(read_p);
                } else {
                    if read != write {
                        ptr::copy_nonoverlapping(read_p, base.add(write), 1);
                    }
                    write += 1;
                }
            }
            read += 1;
        }
        // SAFETY: all kept elements live in `[0..write)`; the trailing
        // bytes are either bit-copies of moved elements (whose owners are
        // now at lower indices) or have been explicitly dropped above.
        unsafe {
            self.set_len(write);
        }
    }

    /// Remove the elements in `range` and return an iterator that yields
    /// them. Mirrors [`Vec::drain`].
    ///
    /// On the iterator's `Drop`, any unconsumed elements in the range are
    /// dropped and the tail is shifted into the gap.
    pub fn drain<R>(&mut self, range: R) -> Drain<'_, T>
    where
        R: std::ops::RangeBounds<usize>,
    {
        let len = self.len();
        let start = match range.start_bound() {
            std::ops::Bound::Included(&n) => n,
            std::ops::Bound::Excluded(&n) => n.checked_add(1).expect("range start overflow"),
            std::ops::Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            std::ops::Bound::Included(&n) => n.checked_add(1).expect("range end overflow"),
            std::ops::Bound::Excluded(&n) => n,
            std::ops::Bound::Unbounded => len,
        };
        assert!(start <= end, "drain range start ({start}) > end ({end})");
        assert!(end <= len, "drain range end ({end}) > len ({len})");
        // SAFETY: `start <= end <= len`, and we hand `Drain` exclusive
        // access to `self` for the iterator's lifetime.
        unsafe { Drain::new(self, start, end) }
    }

    /// Truncate the vector to `new_len`, dropping elements at indices
    /// `[new_len..len)`. No-op if `new_len >= len()`.
    pub fn truncate(&mut self, new_len: usize) {
        let unpacked = self.unpack();
        if new_len >= unpacked.len {
            return;
        }
        // Lower the recorded length *before* dropping the tail, so a panic
        // in an element's `Drop` cannot cause the same elements to be
        // dropped a second time.
        // SAFETY: `new_len < len <= cap`, and elements `[0..new_len]` are
        // still initialized.
        unsafe {
            self.set_len(new_len);
        }
        let tail = ptr::slice_from_raw_parts_mut(
            // SAFETY: `new_len < len <= cap`, so `elements + new_len` is
            // in bounds of the allocation.
            unsafe { unpacked.elements.as_ptr().add(new_len) },
            unpacked.len - new_len,
        );
        // SAFETY: the tail slice covers `len - new_len` initialized
        // elements which we now own (the recorded length excludes them).
        unsafe {
            ptr::drop_in_place(tail);
        }
    }

    /// Reserve capacity for at least `additional` more elements, possibly
    /// promoting to extended mode if the new capacity exceeds the inline
    /// limit.
    pub fn reserve(&mut self, additional: usize) {
        let unpacked = self.unpack();
        let needed = unpacked
            .len
            .checked_add(additional)
            .expect("capacity overflow");
        if needed > unpacked.cap {
            // Grow at least 2× the existing capacity to amortize.
            let new_cap = cmp::max(needed, unpacked.cap.saturating_mul(2)).max(4);
            self.realloc(new_cap);
        }
    }

    /// Shrink the capacity to match the current length, releasing any
    /// unused buffer space. May demote an extended-mode vector to inline
    /// mode if the length now fits.
    pub fn shrink_to_fit(&mut self) {
        let len = self.len();
        if self.capacity() != len {
            self.realloc(len);
        }
    }

    /// Iterator over `&T`.
    #[inline]
    pub fn iter(&self) -> slice::Iter<'_, T> {
        self.as_slice().iter()
    }

    /// Iterator over `&mut T`.
    #[inline]
    pub fn iter_mut(&mut self) -> slice::IterMut<'_, T> {
        self.as_mut_slice().iter_mut()
    }

    /// Convert into a standard `Vec<T>`.
    ///
    /// In inline mode the underlying allocation has exactly `Vec`'s layout
    /// (`Layout::array::<T>(cap)`), so the buffer is handed to `Vec`
    /// without copying. In extended mode the heap header sits in front of
    /// the elements and `Vec` cannot adopt it, so the elements are copied
    /// into a fresh allocation.
    pub fn into_vec(self) -> Vec<T> {
        // Suppress our `Drop` so we can manage the allocation ourselves —
        // either by handing it to `Vec` (inline) or by freeing the header
        // allocation directly (extended). Without `ManuallyDrop` we'd
        // double-free in the inline path.
        let me = mem::ManuallyDrop::new(self);
        let cap_nibble = cap_nibble_of(me.raw.extra());
        let unpacked = me.unpack();

        if unpacked.cap == 0 {
            // No allocation; nothing to hand over and nothing to free.
            return Vec::new();
        }

        if cap_nibble != EXT_SENTINEL {
            // Inline mode: the buffer is exactly `Layout::array::<T>(cap)`,
            // i.e. the layout `Vec` will use to deallocate.
            //
            // SAFETY:
            // - `unpacked.elements.as_ptr()` came from
            //   `alloc::alloc(Layout::array::<T>(unpacked.cap))` — either
            //   from `MiniVec::realloc`'s inline branch or from a `Vec`
            //   we adopted in `From<Vec<T>>` (which uses the same layout).
            // - The first `unpacked.len` elements are initialized;
            //   `unpacked.len <= unpacked.cap`.
            // - `me` is `ManuallyDrop`, so our `Drop` will not run and we
            //   will not double-free the buffer or re-drop the elements
            //   that `Vec` now owns.
            unsafe { Vec::from_raw_parts(unpacked.elements.as_ptr(), unpacked.len, unpacked.cap) }
        } else {
            // Extended mode: copy the elements out, then free our header
            // allocation directly.
            let len = unpacked.len;
            let mut v = Vec::with_capacity(len);
            // SAFETY:
            // - source has `len` initialized `T`s in our buffer.
            // - destination is a freshly allocated, non-overlapping buffer
            //   of capacity `>= len`.
            unsafe {
                ptr::copy_nonoverlapping(unpacked.elements.as_ptr(), v.as_mut_ptr(), len);
                v.set_len(len);
            }
            // SAFETY: every element has just been moved into `v`, so no
            // destructors are owed; we still own the (extended-mode) heap
            // allocation and `dealloc` is the right way to free it.
            unsafe { dealloc::<T>(&unpacked) };
            v
        }
    }

    // ===== private helpers =====

    /// Decode the packed pointer into its constituent parts.
    #[inline]
    fn unpack(&self) -> Unpacked<T> {
        let extra = self.raw.extra();
        let cap_nibble = cap_nibble_of(extra);
        let base = self.raw.into_inner();
        if cap_nibble == EXT_SENTINEL {
            let header_ptr = base.cast::<Header>();
            // SAFETY: in extended mode the header was written by `realloc`
            // when the vector was promoted, and the allocation is still
            // alive because `self` owns it.
            let (len, cap) = unsafe {
                let h = &*header_ptr.as_ptr();
                (h.len, h.cap)
            };
            let offset = extended_elements_offset::<T>();
            // SAFETY: the allocation's layout reserves `offset` bytes for
            // the header + alignment padding; advancing by `offset` stays
            // within the allocation and yields a `T`-aligned pointer.
            let elements_raw = unsafe { base.as_ptr().add(offset).cast::<T>() };
            // SAFETY: `elements_raw` is non-null because `base` is non-null
            // and `offset < isize::MAX`.
            let elements = unsafe { NonNull::new_unchecked(elements_raw) };
            Unpacked {
                elements,
                len,
                cap,
                header: Some(header_ptr),
            }
        } else {
            let cap = cap_nibble as usize;
            let len = len_nibble_of(extra) as usize;
            // In inline mode the base pointer *is* the elements pointer,
            // because the inline allocation is just `[T; cap]`.
            let elements = base.cast::<T>();
            Unpacked {
                elements,
                len,
                cap,
                header: None,
            }
        }
    }

    /// In extended mode, return the header pointer.
    ///
    /// # Safety
    /// Caller must have verified that the vector is in extended mode (i.e.
    /// `cap_nibble == EXT_SENTINEL`).
    #[inline]
    unsafe fn header_ptr_unchecked(&self) -> NonNull<Header> {
        self.raw.into_inner().cast::<Header>()
    }

    /// Update the recorded length.
    ///
    /// # Safety
    /// `new_len <= capacity()`, and the first `new_len` elements of the
    /// buffer must be initialized.
    #[inline]
    unsafe fn set_len(&mut self, new_len: usize) {
        let extra = self.raw.extra();
        let cap_nibble = cap_nibble_of(extra);
        if cap_nibble == EXT_SENTINEL {
            // SAFETY: extended mode → the heap header is valid and writable
            // (we own the allocation).
            unsafe {
                let header = self.header_ptr_unchecked();
                (*header.as_ptr()).len = new_len;
            }
        } else {
            debug_assert!(new_len <= cap_nibble as usize);
            self.raw.set_extra(pack_meta(cap_nibble, new_len as u8));
        }
    }

    /// Grow capacity by at least one slot. Used as the slow path of `push`.
    #[cold]
    fn grow_one(&mut self) {
        let cap = self.capacity();
        let new_cap = match cap {
            0 => 4,
            n => n.checked_mul(2).expect("capacity overflow"),
        };
        self.realloc(new_cap);
    }

    /// Reallocate to exactly `new_cap`, preserving the current `len`
    /// elements.
    fn realloc(&mut self, new_cap: usize) {
        let old = self.unpack();
        let len = old.len;
        assert!(new_cap >= len, "MiniVec::realloc: new_cap < len");

        // When the allocation stays in the same mode — inline → inline or extended → extended — the
        // layout keeps its alignment and element offset, so the resize is handed to
        // `alloc::realloc`, letting the allocator grow or shrink the block in place and skip
        // copying the elements. The general path below (fresh allocation + element copy + free)
        // handles the cases that genuinely move the elements: the first allocation, promotion
        // (inline → extended), demotion (extended → inline), and freeing (`new_cap == 0`).
        //
        // Note that we do not attempt to `realloc` when we switch modes even if the alignment works
        // out to technically allow it. The practical reason is that it'd only help a little since
        // we'd need to memcpy anyway, but also our inline allocations are likely to be small enough
        // that allocators are using region/size-class based allocation strategies, from which
        // `realloc` doesn't have much room for efficiency anyway.
        match old.header {
            None if old.cap > 0 && new_cap > 0 && new_cap <= MAX_INLINE_CAP => {
                // inline → inline: both layouts are `Layout::array::<T>`, so
                // the elements stay at offset 0 across the resize.
                let old_layout = Layout::array::<T>(old.cap).expect("capacity overflow");
                let new_layout = Layout::array::<T>(new_cap).expect("capacity overflow");
                // SAFETY: `old.cap > 0` with `old.header == None` means
                // `old.elements` is the inline allocation made with
                // `old_layout`; both layouts use `align_of::<T>()`, and
                // `new_layout.size() > 0` since `new_cap > 0` and `T` is not
                // a ZST.
                let alloc_ptr =
                    unsafe { realloc_or_abort(old.elements.cast::<u8>(), old_layout, new_layout) };
                // `len <= new_cap <= MAX_INLINE_CAP < 16`, so the nibble
                // casts are lossless.
                self.raw = PackedPtr::new(alloc_ptr, pack_meta(new_cap as u8, len as u8));
                return;
            }
            Some(header) if new_cap > MAX_INLINE_CAP => {
                // extended → extended: both layouts are a `Header` followed
                // by `[T; cap]` with the same alignment, so the header and
                // the elements keep their offsets across the resize.
                let old_layout = extended_layout::<T>(old.cap).0;
                let new_layout = extended_layout::<T>(new_cap).0;
                // SAFETY: in extended mode `header` is the allocation made
                // with `old_layout`; the layouts share alignment and
                // `new_layout.size() >= size_of::<Header>() > 0`.
                let alloc_ptr =
                    unsafe { realloc_or_abort(header.cast::<u8>(), old_layout, new_layout) };
                // `realloc` preserved the whole header (it lives at offset 0,
                // within the preserved prefix), so `len` is intact; only the
                // capacity changed.
                // SAFETY: `alloc_ptr` points to the live, writable `Header`
                // at the start of the just-resized allocation.
                unsafe {
                    (*alloc_ptr.as_ptr().cast::<Header>()).cap = new_cap;
                }
                // The inline length nibble is irrelevant in extended mode, so
                // we leave it at 0.
                self.raw = PackedPtr::new(alloc_ptr, pack_meta(EXT_SENTINEL, 0));
                return;
            }
            _ => {}
        }

        let new_packed: PackedPtr<NonNull<u8>> = if new_cap == 0 {
            PackedPtr::new(NonNull::<T>::dangling().cast::<u8>(), 0)
        } else if new_cap > MAX_INLINE_CAP {
            let (layout, offset) = extended_layout::<T>(new_cap);
            // SAFETY: `layout.size() >= size_of::<Header>() > 0`.
            let raw = unsafe { alloc::alloc(layout) };
            if raw.is_null() {
                alloc::handle_alloc_error(layout);
            }
            // SAFETY: `alloc` returned a non-null pointer on success.
            let alloc_ptr = unsafe { NonNull::new_unchecked(raw) };
            // SAFETY: the allocation is at least `size_of::<Header>()`
            // bytes large and aligned to at least `align_of::<Header>()`
            // (the combined `Layout::extend` carries the max alignment),
            // and is uninitialized memory we just got from the allocator.
            unsafe {
                ptr::write(
                    alloc_ptr.as_ptr().cast::<Header>(),
                    Header { len, cap: new_cap },
                );
            }
            if len > 0 {
                // SAFETY:
                // - source has `len` initialized `T`s in our old buffer.
                // - destination has space for at least `new_cap >= len` `T`s
                //   starting at `offset`, properly aligned.
                // - the two allocations do not overlap (we just allocated
                //   the destination).
                unsafe {
                    let dst = alloc_ptr.as_ptr().add(offset).cast::<T>();
                    ptr::copy_nonoverlapping(old.elements.as_ptr(), dst, len);
                }
            }
            // The inline length nibble is irrelevant in extended mode (the
            // real length lives in the header), so we leave it at 0.
            PackedPtr::new(alloc_ptr, pack_meta(EXT_SENTINEL, 0))
        } else {
            let layout = Layout::array::<T>(new_cap).expect("capacity overflow");
            // SAFETY: `layout.size() > 0` since `new_cap > 0` and we already
            // verified `T` is not a ZST.
            let raw = unsafe { alloc::alloc(layout) };
            if raw.is_null() {
                alloc::handle_alloc_error(layout);
            }
            // SAFETY: `alloc` returned a non-null pointer on success.
            let alloc_ptr = unsafe { NonNull::new_unchecked(raw.cast::<T>()) };
            if len > 0 {
                // SAFETY: same reasoning as the extended branch.
                unsafe {
                    ptr::copy_nonoverlapping(old.elements.as_ptr(), alloc_ptr.as_ptr(), len);
                }
            }
            // `len <= new_cap <= MAX_INLINE_CAP < 16`, so the nibble casts
            // are lossless.
            PackedPtr::new(alloc_ptr.cast::<u8>(), pack_meta(new_cap as u8, len as u8))
        };

        // SAFETY: every element of the old buffer has just been bit-copied
        // into the new buffer (or there were none), and we own the old
        // allocation, so freeing it is safe.
        unsafe {
            dealloc::<T>(&old);
        }

        self.raw = new_packed;
    }

    /// Try to adopt `vec`'s allocation as an extended-mode `MiniVec`,
    /// avoiding a fresh allocation and an element copy: the elements are slid
    /// forward within the buffer to make room for the [`Header`] at the
    /// front, and the trailing spare capacity becomes the `MiniVec`'s
    /// capacity. Only a `memmove` of the live elements is performed.
    ///
    /// This is sound only when the buffer `MiniVec` will later free with
    /// `extended_layout::<T>(cap)` is byte-for-byte the `Layout::array::<T>`
    /// the `Vec` allocated with, which constrains `T`:
    /// - `align_of::<T>() >= align_of::<Header>()`, so the extended layout's
    ///   alignment (the max of the two) equals the `Vec`'s.
    /// - the element offset (`Header` size rounded up to `T`'s alignment) is a
    ///   whole number of element slots, so a `cap` exists that makes the
    ///   extended layout's size equal the `Vec`'s buffer size exactly.
    ///
    /// Plus enough spare capacity to fit the header. Returns `Err(vec)`,
    /// handing ownership back, when any condition fails so the caller can
    /// fall back to copying.
    fn try_from_vec_extended(vec: Vec<T>) -> Result<Self, Vec<T>> {
        let offset = extended_elements_offset::<T>();
        let elem_size = size_of::<T>();
        if align_of::<T>() < align_of::<Header>() || !offset.is_multiple_of(elem_size) {
            return Err(vec);
        }
        // Number of element slots the header occupies at the front.
        let shift = offset / elem_size;

        let len = vec.len();
        let vec_cap = vec.capacity();
        // Require:
        //  1. Room to slide the elements past the header
        //  2. That extended mode is actually the correct encoding for this capacity
        if vec_cap < len + shift || vec_cap - shift <= MAX_INLINE_CAP {
            return Err(vec);
        }
        let new_cap = vec_cap - shift;

        let mut vec = mem::ManuallyDrop::new(vec);
        let base = vec.as_mut_ptr();
        // SAFETY: `base` starts a `Vec` allocation of `vec_cap` `T` slots;
        // `shift + len <= vec_cap` (checked above) keeps both `base.add(shift)`
        // and the `len`-element destination in bounds. Source and destination
        // overlap, so `ptr::copy` (a memmove) is required.
        unsafe {
            ptr::copy(base, base.add(shift), len);
        }
        // SAFETY: `base` is `align_of::<T>() >= align_of::<Header>()` aligned,
        // and the leading `offset >= size_of::<Header>()` bytes are now vacated
        // (the elements moved to `base.add(shift)`), so the header overwrites
        // no element.
        unsafe {
            ptr::write(base.cast::<Header>(), Header { len, cap: new_cap });
        }
        // SAFETY: `vec_cap > 0`, so `base` is a real, non-null buffer pointer;
        // the cast only changes the pointee type, preserving address and
        // provenance.
        let alloc_ptr = unsafe { NonNull::new_unchecked(base.cast::<u8>()) };
        // The inline length nibble is irrelevant in extended mode (the real
        // length lives in the header), so we leave it at 0. `vec` is
        // `ManuallyDrop`, so the buffer is now owned solely by the `MiniVec`.
        Ok(MiniVec {
            raw: PackedPtr::new(alloc_ptr, pack_meta(EXT_SENTINEL, 0)),
            _marker: PhantomData,
        })
    }
}

impl<T: Allocative> MiniVec<T> {
    /// Report this vector's heap allocation to an allocative `Visitor`.
    ///
    /// The caller must already have entered a node for the owning value (e.g.
    /// via `Visitor::enter_self_sized`); this adds the `ptr` child and the heap
    /// contents beneath it. The whole allocation is accounted for: the
    /// elements, any unused capacity, and — in extended mode — the in-heap
    /// `Header` plus the padding that aligns the elements behind it. Nothing is
    /// emitted when the vector has not allocated.
    ///
    /// Shared by the `MiniVec` and `MiniBoxSlice` `Allocative` impls so the
    /// layout-dependent accounting (easy to get subtly wrong) lives next to the
    /// layout code instead of being duplicated per wrapper.
    pub(crate) fn visit_allocation<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let cap = self.capacity();
        if cap == 0 {
            // No heap allocation to report.
            return;
        }
        let mut visitor = visitor.enter_unique(allocative::Key::new("ptr"), size_of::<*const T>());
        if self.is_extended() {
            // The extended-mode allocation begins with a `Header` (`len` +
            // `cap`), followed by padding when `T` is more aligned than the
            // header. `extended_elements_offset` is exactly the number of heap
            // bytes that precede the elements, so the difference is the
            // padding.
            visitor.visit_simple(allocative::Key::new("header"), size_of::<Header>());
            let padding = extended_elements_offset::<T>() - size_of::<Header>();
            if padding > 0 {
                visitor.visit_simple(allocative::Key::new("header_padding"), padding);
            }
        }
        {
            // One "data" node spans every slot; `visit_slice` walks the
            // initialized prefix and any unused tail surfaces as this node's
            // own bar in the flame graph.
            let data_size = size_of::<T>() * cap;
            let mut visitor = visitor.enter(allocative::Key::new("data"), data_size);
            visitor.visit_slice::<T>(self.as_slice());
            visitor.exit();
        }
        visitor.exit();
    }
}

/// Layout of an extended-mode allocation (`Header` followed by `[T; cap]`),
/// returning `(full_layout, elements_offset)`.
fn extended_layout<T>(cap: usize) -> (Layout, usize) {
    let header = Layout::new::<Header>();
    let array = Layout::array::<T>(cap).expect("capacity overflow");
    header.extend(array).expect("layout overflow")
}

/// Offset (in bytes) of the elements within an extended-mode allocation.
/// The offset depends only on `align_of::<T>()`, not on `cap`.
#[inline]
fn extended_elements_offset<T>() -> usize {
    // Use cap = 1 so the array contributes a non-zero size, which keeps
    // `Layout::extend` on its well-trodden path.
    extended_layout::<T>(1).1
}

/// `alloc::realloc` that aborts on allocation failure, returning a non-null
/// pointer to a block of `new_layout.size()` bytes.
///
/// # Safety
/// `ptr` must denote a currently-live allocation created with `old_layout`,
/// and `new_layout` must share `old_layout`'s alignment — `realloc` cannot
/// change an allocation's alignment. `new_layout.size()` must be non-zero.
unsafe fn realloc_or_abort(
    ptr: NonNull<u8>,
    old_layout: Layout,
    new_layout: Layout,
) -> NonNull<u8> {
    debug_assert_eq!(
        old_layout.align(),
        new_layout.align(),
        "realloc cannot change an allocation's alignment",
    );
    // SAFETY: forwarded from the caller's contract — `ptr`/`old_layout`
    // describe the live allocation and `new_layout.size()` is non-zero.
    let raw = unsafe { alloc::realloc(ptr.as_ptr(), old_layout, new_layout.size()) };
    // `handle_alloc_error` diverges, so on the success path `raw` is non-null.
    NonNull::new(raw).unwrap_or_else(|| alloc::handle_alloc_error(new_layout))
}

/// Free the allocation described by `unpacked`, if any.
///
/// # Safety
/// `unpacked` must describe an allocation owned by a `MiniVec<T>`, and the
/// elements in it must already have been moved out or dropped.
unsafe fn dealloc<T>(unpacked: &Unpacked<T>) {
    if unpacked.cap == 0 {
        // No allocation (dangling pointer).
        return;
    }
    match unpacked.header {
        Some(header_ptr) => {
            let layout = extended_layout::<T>(unpacked.cap).0;
            // SAFETY: caller guarantees this allocation came from `realloc`
            // in extended mode with this `cap`, hence this layout.
            unsafe {
                alloc::dealloc(header_ptr.as_ptr().cast::<u8>(), layout);
            }
        }
        None => {
            let layout = Layout::array::<T>(unpacked.cap).expect("capacity overflow");
            // SAFETY: caller guarantees this allocation came from `realloc`
            // in inline mode with this `cap`, hence this layout.
            unsafe {
                alloc::dealloc(unpacked.elements.as_ptr().cast::<u8>(), layout);
            }
        }
    }
}

impl<T> From<Vec<T>> for MiniVec<T> {
    /// Build a `MiniVec` from a `Vec`, reusing the existing allocation
    /// when possible.
    ///
    /// If the `Vec`'s capacity fits in inline mode (`<= 14`), the buffer is
    /// adopted verbatim — no element copy or reallocation. Above that limit
    /// the buffer can still be adopted as an extended-mode allocation when
    /// `T`'s alignment and the spare capacity allow the elements to be slid
    /// past an in-buffer `Header` (see `Self::try_from_vec_extended`);
    /// failing that, the elements are copied through the iterator path.
    fn from(vec: Vec<T>) -> Self {
        let _ = Self::_ASSERT_NOT_ZST;
        let cap = vec.capacity();
        if cap == 0 {
            // Vec has no backing allocation to reuse.
            return Self::new();
        }
        if cap <= MAX_INLINE_CAP {
            // Vec's allocation is exactly `Layout::array::<T>(cap)`, which
            // is also MiniVec's inline-mode layout. Steal it.
            let mut vec = mem::ManuallyDrop::new(vec);
            let len = vec.len();
            let ptr = vec.as_mut_ptr();
            // SAFETY: `cap > 0` implies `Vec` holds a real, non-null
            // buffer pointer; the cast to `NonNull<u8>` only changes the
            // pointee type, preserving the address and provenance.
            let nonnull = unsafe { NonNull::new_unchecked(ptr.cast::<u8>()) };
            // `cap <= MAX_INLINE_CAP < 16` and `len <= cap`, so both nibble
            // casts are lossless. After this `PackedPtr::new`, the
            // `MiniVec` owns the buffer; `vec` is `ManuallyDrop` so it
            // will not be freed a second time.
            let raw = PackedPtr::new(nonnull, pack_meta(cap as u8, len as u8));
            return MiniVec {
                raw,
                _marker: PhantomData,
            };
        }
        // Above the inline limit. Adopt the buffer in place if the layout
        // works out, otherwise copy through the iterator path (which builds
        // an extended-mode `MiniVec` directly).
        Self::try_from_vec_extended(vec).unwrap_or_else(|vec| vec.into_iter().collect())
    }
}

// SAFETY: The `may_dangle` gives us partially conservative handling wrt dropck by rustc. The
// soundness argument for this is simple: `Vec` has the same thing and we don't do anything more
// intersting than `Vec` does. Rustonomicon has a bit more details on what this is.
//
// `may_dangle` is nightly-only, so without `rust_nightly` we fall back to a plain `Drop` impl and
// the stricter dropck rules that come with it.
#[cfg(rust_nightly)]
unsafe impl<#[may_dangle] T> Drop for MiniVec<T> {
    fn drop(&mut self) {
        self.drop_impl();
    }
}

#[cfg(not(rust_nightly))]
impl<T> Drop for MiniVec<T> {
    fn drop(&mut self) {
        self.drop_impl();
    }
}

impl<T> MiniVec<T> {
    fn drop_impl(&mut self) {
        let unpacked = self.unpack();
        // Use a guard so the allocation is freed even if dropping an
        // element panics.
        struct DeallocGuard<T> {
            unpacked: Unpacked<T>,
        }
        impl<T> Drop for DeallocGuard<T> {
            fn drop(&mut self) {
                // SAFETY: we own this allocation; elements were just dropped
                // (or panicked-out, in which case they leak per `Vec`'s
                // convention but the allocation is still freed).
                unsafe { dealloc::<T>(&self.unpacked) };
            }
        }
        let _guard = DeallocGuard::<T> { unpacked };
        // SAFETY: drop the `len` initialized elements in place.
        unsafe {
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(
                unpacked.elements.as_ptr(),
                unpacked.len,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    #[test]
    fn size_is_one_pointer() {
        // The whole point: `MiniVec<T>` packs into a single word regardless of
        // `T`, vs. `Vec<T>` which is three. With packing disabled it is instead
        // a pointer plus a metadata byte, padded to two words.
        #[cfg(not(mini_vec_no_ptr_packing))]
        const WORDS: usize = 1;
        #[cfg(mini_vec_no_ptr_packing)]
        const WORDS: usize = 2;
        let expected = WORDS * size_of::<*const u8>();
        assert_eq!(size_of::<MiniVec<u8>>(), expected);
        assert_eq!(size_of::<MiniVec<u64>>(), expected);
        assert_eq!(size_of::<MiniVec<String>>(), expected);
    }

    #[test]
    fn empty_does_not_allocate() {
        let v = MiniVec::<u32>::new();
        assert_eq!(v.len(), 0);
        assert_eq!(v.capacity(), 0);
        assert!(v.is_empty());
        assert!(!v.is_extended());
    }

    #[test]
    fn push_pop_inline() {
        let mut v = MiniVec::<u32>::new();
        for i in 0..8 {
            v.push(i);
        }
        assert_eq!(v.len(), 8);
        assert!(
            !v.is_extended(),
            "8 elements still fit inline (cap doubles 4 -> 8, at or below the inline limit of 14)"
        );
        for i in (0..8).rev() {
            assert_eq!(v.pop(), Some(i));
        }
        assert_eq!(v.pop(), None);
        assert!(v.is_empty());
    }

    #[test]
    fn promotion_to_extended() {
        let mut v = MiniVec::<u32>::new();
        for i in 0..1000_u32 {
            v.push(i);
        }
        assert_eq!(v.len(), 1000);
        assert!(
            v.is_extended(),
            "1000 elements forces capacity above the inline limit (14)"
        );
        assert!(v.capacity() >= 1000);
        for (i, x) in v.iter().enumerate() {
            assert_eq!(*x as usize, i);
        }
    }

    #[test]
    fn iter_inline_and_extended() {
        let small: MiniVec<u32> = (0..10).collect();
        assert!(!small.is_extended());
        assert_eq!(small.iter().sum::<u32>(), (0..10_u32).sum::<u32>());

        let big: MiniVec<u32> = (0..500).collect();
        assert!(big.is_extended());
        assert_eq!(big.iter().sum::<u32>(), (0..500_u32).sum::<u32>());
    }

    #[test]
    fn drop_runs_extended() {
        struct DropCounter<'a>(&'a Cell<usize>);
        impl Drop for DropCounter<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let count = Cell::new(0);
        {
            let mut v = MiniVec::new();
            for _ in 0..1000 {
                v.push(DropCounter(&count));
            }
            assert!(v.is_extended());
        }
        assert_eq!(
            count.get(),
            1000,
            "every element should be dropped exactly once"
        );
    }

    #[test]
    fn drop_runs_inline() {
        struct DropCounter<'a>(&'a Cell<usize>);
        impl Drop for DropCounter<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let count = Cell::new(0);
        {
            let mut v = MiniVec::new();
            for _ in 0..7 {
                v.push(DropCounter(&count));
            }
            assert!(!v.is_extended());
        }
        assert_eq!(count.get(), 7);
    }

    #[test]
    fn truncate() {
        let mut v: MiniVec<u32> = (0..10).collect();
        v.truncate(3);
        assert_eq!(v.as_slice(), &[0, 1, 2]);
        assert_eq!(v.len(), 3);

        // Truncating to >= len is a no-op.
        v.truncate(10);
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn into_vec_round_trip() {
        let v: MiniVec<u32> = (0..1000).collect();
        let vec = v.into_vec();
        assert_eq!(vec.len(), 1000);
        assert!(vec.iter().enumerate().all(|(i, x)| *x as usize == i));
    }

    #[test]
    fn from_vec_round_trip() {
        let original: Vec<u32> = (0..500).collect();
        let mv = MiniVec::from(original.clone());
        assert!(mv.is_extended());
        let back: Vec<u32> = mv.into();
        assert_eq!(back, original);
    }

    #[test]
    fn from_vec_inline_reuses_buffer() {
        // Vec with cap fitting in inline mode: the buffer should pass
        // through `From<Vec<T>>` without any element copy or reallocation.
        let mut v: Vec<i32> = Vec::with_capacity(12);
        v.extend(0..10);
        let original_ptr = v.as_ptr();
        let original_cap = v.capacity();

        let mv: MiniVec<i32> = MiniVec::from(v);
        assert!(!mv.is_extended());
        assert_eq!(mv.capacity(), original_cap);
        assert_eq!(mv.len(), 10);
        assert_eq!(
            mv.as_ptr(),
            original_ptr,
            "inline From<Vec<T>> must reuse the buffer pointer",
        );
        assert!(mv.iter().enumerate().all(|(i, x)| *x == i as i32));
    }

    #[test]
    fn into_vec_inline_reuses_buffer() {
        // MiniVec in inline mode -> Vec should hand the buffer over
        // without a copy.
        let mv: MiniVec<i32> = (0..10).collect();
        assert!(!mv.is_extended());
        let original_ptr = mv.as_ptr();
        let original_cap = mv.capacity();

        let v: Vec<i32> = mv.into_vec();
        assert_eq!(
            v.as_ptr(),
            original_ptr,
            "inline into_vec must reuse buffer"
        );
        assert_eq!(v.capacity(), original_cap);
        assert_eq!(v.len(), 10);
        assert!(v.iter().enumerate().all(|(i, x)| *x == i as i32));
    }

    #[test]
    fn vec_round_trip_inline_preserves_buffer() {
        // Vec -> MiniVec -> Vec: the same buffer survives the round trip
        // when everything stays inline, even with capacity to spare
        // (`cap == MAX_INLINE_CAP`, `len < cap`).
        let mut v: Vec<i32> = Vec::with_capacity(MAX_INLINE_CAP);
        v.extend(0..10);
        let original_ptr = v.as_ptr();
        let original_cap = v.capacity();

        let mv: MiniVec<i32> = MiniVec::from(v);
        assert!(!mv.is_extended());
        let back: Vec<i32> = mv.into_vec();
        assert_eq!(back.as_ptr(), original_ptr);
        assert_eq!(back.capacity(), original_cap);
        assert_eq!(back.len(), 10);
    }

    #[test]
    fn from_vec_above_inline_limit_falls_back_to_copy() {
        // `u32` is less aligned than the extended-mode `Header`, so the Vec's
        // allocation can't be adopted in place (its layout could never match
        // what `MiniVec` would free it with). The result must still be a
        // correct, extended-mode MiniVec built by copying.
        let mut v: Vec<u32> = Vec::with_capacity(500);
        v.extend(0..500);
        let original_ptr = v.as_ptr();
        let mv: MiniVec<u32> = MiniVec::from(v);
        assert!(mv.is_extended());
        assert_eq!(mv.len(), 500);
        assert!(mv.iter().enumerate().all(|(i, x)| *x as usize == i));
        assert_ne!(
            mv.as_ptr(),
            original_ptr,
            "under-aligned element type must not adopt the buffer in place",
        );
    }

    #[test]
    fn from_vec_above_inline_limit_reuses_buffer() {
        // `u64` is aligned to the `Header`, and `size_of` divides the element
        // offset, so a Vec with spare capacity is adopted in place: the
        // elements slide forward by `shift` slots to clear room for the
        // header, and no fresh allocation happens.
        let shift = extended_elements_offset::<u64>() / size_of::<u64>();
        let mut v: Vec<u64> = Vec::with_capacity(100);
        v.extend(0..50);
        let original_ptr = v.as_ptr();
        let vec_cap = v.capacity();

        let mv: MiniVec<u64> = MiniVec::from(v);
        assert!(mv.is_extended());
        assert_eq!(mv.len(), 50);
        // The header consumed `shift` slots off the front.
        assert_eq!(mv.capacity(), vec_cap - shift);
        // SAFETY: `original_ptr` is in bounds of the (now adopted) allocation
        // for `shift <= vec_cap`; this only computes the address, not a deref.
        let shifted = unsafe { original_ptr.add(shift) };
        assert_eq!(
            mv.as_ptr(),
            shifted,
            "aligned element type with spare capacity must reuse the buffer",
        );
        assert!(mv.iter().enumerate().all(|(i, x)| *x as usize == i));
    }

    #[test]
    fn from_vec_above_inline_limit_no_spare_falls_back() {
        // Aligned element type but the Vec is full (len == cap), so there is
        // no room to slide the elements past the header. Must fall back to a
        // correct copy rather than corrupt the buffer. Round-tripping through
        // a boxed slice guarantees capacity == length (no spare).
        let v: Vec<u64> = (0..200).collect::<Vec<u64>>().into_boxed_slice().into_vec();
        assert_eq!(
            v.len(),
            v.capacity(),
            "boxed-slice round trip removes spare"
        );

        let mv: MiniVec<u64> = MiniVec::from(v);
        assert!(mv.is_extended());
        assert_eq!(mv.len(), 200);
        assert!(mv.iter().enumerate().all(|(i, x)| *x as usize == i));
    }

    #[test]
    fn from_vec_reuse_drops_correctly() {
        // The in-place reuse path memmoves non-`Copy` elements (`Rc` is one
        // pointer, so it hits the reuse path). Doing so must neither clone nor
        // drop any element: the buffer's bytes are just relocated.
        let r = Rc::new(());
        let mut v: Vec<Rc<()>> = Vec::with_capacity(100);
        for _ in 0..50 {
            v.push(r.clone());
        }
        assert_eq!(Rc::strong_count(&r), 51);

        let mv: MiniVec<Rc<()>> = MiniVec::from(v);
        assert!(mv.is_extended());
        assert_eq!(
            Rc::strong_count(&r),
            51,
            "adopting the buffer must not clone or drop elements",
        );
        drop(mv);
        assert_eq!(
            Rc::strong_count(&r),
            1,
            "all clones dropped with the MiniVec"
        );
    }

    #[test]
    fn from_empty_vec_does_not_allocate() {
        let v: Vec<u32> = Vec::new();
        let mv = MiniVec::from(v);
        assert_eq!(mv.len(), 0);
        assert_eq!(mv.capacity(), 0);
        assert!(!mv.is_extended());
    }

    #[test]
    fn into_vec_drops_correctly_in_extended_mode() {
        // Extended mode must copy elements out and free the header
        // allocation, without dropping the moved-out elements twice.
        let r = Rc::new(());
        let mv: MiniVec<Rc<()>> = (0..1000).map(|_| r.clone()).collect();
        assert!(mv.is_extended());
        assert_eq!(Rc::strong_count(&r), 1001);
        let v: Vec<Rc<()>> = mv.into_vec();
        // Each Rc was moved exactly once (no double-drop, no leak).
        assert_eq!(Rc::strong_count(&r), 1001);
        drop(v);
        assert_eq!(Rc::strong_count(&r), 1);
    }

    #[test]
    fn into_vec_drops_correctly_in_inline_mode() {
        // Inline mode hands the buffer to Vec verbatim — same invariant.
        let r = Rc::new(());
        let mv: MiniVec<Rc<()>> = (0..10).map(|_| r.clone()).collect();
        assert!(!mv.is_extended());
        assert_eq!(Rc::strong_count(&r), 11);
        let v = mv.into_vec();
        assert_eq!(Rc::strong_count(&r), 11);
        drop(v);
        assert_eq!(Rc::strong_count(&r), 1);
    }

    #[test]
    fn extension_boundary() {
        // Walk right up to the inline cap limit, then push one more element
        // to force promotion to extended mode. The contents must be
        // preserved across the transition.
        let mut v = MiniVec::<u8>::with_capacity(MAX_INLINE_CAP);
        assert!(!v.is_extended());
        assert_eq!(v.capacity(), MAX_INLINE_CAP);

        for i in 0..(MAX_INLINE_CAP as u8) {
            v.push(i);
        }
        assert!(
            !v.is_extended(),
            "exactly cap elements should still be inline"
        );
        assert_eq!(v.len(), MAX_INLINE_CAP);

        // One more element forces promotion.
        v.push(255);
        assert!(v.is_extended());
        assert_eq!(v.len(), MAX_INLINE_CAP + 1);
        assert_eq!(v[0], 0);
        assert_eq!(v[MAX_INLINE_CAP - 1], (MAX_INLINE_CAP - 1) as u8);
        assert_eq!(v[MAX_INLINE_CAP], 255);
    }

    #[test]
    fn with_capacity_above_inline_limit_starts_extended() {
        let v = MiniVec::<u32>::with_capacity(MAX_INLINE_CAP + 1);
        assert!(v.is_extended());
        assert!(v.capacity() > MAX_INLINE_CAP);
        assert_eq!(v.len(), 0);
    }

    #[test]
    fn rc_drop_count() {
        let r = Rc::new(());
        let v: MiniVec<Rc<()>> = (0..50).map(|_| r.clone()).collect();
        assert_eq!(Rc::strong_count(&r), 51, "50 clones in vec + 1 original");
        drop(v);
        assert_eq!(Rc::strong_count(&r), 1, "all clones dropped with the vec");
    }

    #[test]
    fn rc_drop_count_extended() {
        let r = Rc::new(());
        let v: MiniVec<Rc<()>> = (0..1000).map(|_| r.clone()).collect();
        assert!(v.is_extended());
        assert_eq!(Rc::strong_count(&r), 1001);
        drop(v);
        assert_eq!(Rc::strong_count(&r), 1);
    }

    #[test]
    fn high_alignment() {
        // Exercises the alignment handling in `extended_layout`: a 64-byte
        // aligned `T` should still get correctly aligned elements after the
        // 16-byte header.
        #[repr(align(64))]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        struct Aligned64(u32);

        let v: MiniVec<Aligned64> = (0..1000).map(|i| Aligned64(i as u32)).collect();
        assert!(v.is_extended());
        for (i, x) in v.iter().enumerate() {
            assert_eq!(x.0, i as u32);
            assert_eq!(
                (x as *const _ as usize) & 63,
                0,
                "elements must be 64-byte aligned"
            );
        }
    }

    #[test]
    fn iter_mut_modifies_in_place() {
        let mut v: MiniVec<u32> = (0..10).collect();
        for x in v.iter_mut() {
            *x *= 2;
        }
        let expected: Vec<u32> = (0..10).map(|i| i * 2).collect();
        assert_eq!(v.as_slice(), expected.as_slice());
    }

    #[test]
    fn shrink_to_fit_extended_stays_extended() {
        // Over-reserve, then shrink: the extended → extended realloc path
        // brings capacity back down to the length while preserving every
        // element.
        let mut v: MiniVec<u32> = (0..100).collect();
        assert!(v.is_extended());
        v.reserve(1000);
        assert!(v.capacity() >= 1100);
        v.shrink_to_fit();
        assert_eq!(v.capacity(), 100, "capacity should shrink to exactly len");
        assert!(v.is_extended(), "100 > inline limit, so still extended");
        assert!(v.iter().enumerate().all(|(i, x)| *x as usize == i));
    }

    #[test]
    fn shrink_to_fit_demotes_to_inline() {
        // An extended vector truncated below the inline limit demotes back to
        // inline mode on `shrink_to_fit` (the extended → inline copy path),
        // keeping its elements.
        let mut v: MiniVec<u32> = (0..100).collect();
        assert!(v.is_extended());
        v.truncate(5);
        v.shrink_to_fit();
        assert!(!v.is_extended(), "len 5 fits inline after shrinking");
        assert_eq!(v.capacity(), 5);
        assert_eq!(v.as_slice(), &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn shrink_to_fit_inline() {
        // Inline over-capacity shrinks in place via the inline → inline
        // realloc path.
        let mut v: MiniVec<u32> = MiniVec::with_capacity(MAX_INLINE_CAP);
        v.push(1);
        v.push(2);
        assert!(!v.is_extended());
        assert_eq!(v.capacity(), MAX_INLINE_CAP);
        v.shrink_to_fit();
        assert_eq!(v.capacity(), 2);
        assert_eq!(v.as_slice(), &[1, 2]);
    }

    #[test]
    fn shrink_to_fit_extended_drops_correctly() {
        // Shrinking in extended mode must not drop or duplicate any element:
        // the elements are preserved in place by `realloc`, not copied.
        let r = Rc::new(());
        let mut v: MiniVec<Rc<()>> = (0..100).map(|_| r.clone()).collect();
        assert!(v.is_extended());
        v.reserve(1000);
        assert_eq!(Rc::strong_count(&r), 101);
        v.shrink_to_fit();
        assert_eq!(
            Rc::strong_count(&r),
            101,
            "shrink must not drop any element"
        );
        drop(v);
        assert_eq!(Rc::strong_count(&r), 1);
    }

    #[test]
    fn push_after_clear() {
        let mut v: MiniVec<u32> = (0..1000).collect();
        let cap_before = v.capacity();
        v.clear();
        assert_eq!(v.len(), 0);
        // `clear` keeps the allocation, so capacity is unchanged.
        assert_eq!(v.capacity(), cap_before);
        v.push(42);
        assert_eq!(v.as_slice(), &[42]);
    }

    #[test]
    fn insert_basic() {
        let mut v: MiniVec<u32> = MiniVec::new();
        v.insert(0, 10);
        v.insert(1, 30);
        v.insert(1, 20);
        v.insert(0, 5);
        assert_eq!(v.as_slice(), &[5, 10, 20, 30]);
    }

    #[test]
    fn insert_at_end_equals_push() {
        let mut v: MiniVec<u32> = (0..5).collect();
        v.insert(5, 99);
        assert_eq!(v.as_slice(), &[0, 1, 2, 3, 4, 99]);
    }

    #[test]
    fn insert_grows_capacity() {
        // Forces multiple grows including the inline → extended boundary.
        let mut v: MiniVec<u32> = MiniVec::new();
        for i in 0..400 {
            v.insert(0, i);
        }
        let expected: Vec<u32> = (0..400).rev().collect();
        assert_eq!(v.as_slice(), expected.as_slice());
        assert!(v.is_extended());
    }

    // With panic=abort the panic kills the in-process Miri harness.
    #[cfg(not(all(miri, panic = "abort")))]
    #[test]
    #[should_panic(expected = "insertion index")]
    fn insert_out_of_bounds_panics() {
        let mut v: MiniVec<u32> = (0..3).collect();
        v.insert(4, 99);
    }

    #[test]
    fn dedup_basic() {
        let mut v: MiniVec<u32> = vec![1, 1, 2, 3, 3, 3, 4, 5, 5].into();
        v.dedup();
        assert_eq!(v.as_slice(), &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn dedup_already_unique_no_change() {
        let mut v: MiniVec<u32> = (0..10).collect();
        v.dedup();
        assert_eq!(v.len(), 10);
        assert!(v.iter().enumerate().all(|(i, x)| *x as usize == i));
    }

    #[test]
    fn dedup_all_same() {
        let mut v: MiniVec<u32> = vec![7; 100].into();
        v.dedup();
        assert_eq!(v.as_slice(), &[7]);
    }

    #[test]
    fn dedup_empty_and_single() {
        let mut v: MiniVec<u32> = MiniVec::new();
        v.dedup();
        assert_eq!(v.len(), 0);
        v.push(42);
        v.dedup();
        assert_eq!(v.as_slice(), &[42]);
    }

    #[test]
    fn dedup_drops_correctly() {
        // Each duplicate must be dropped exactly once.
        let r = Rc::new(());
        let mut v: MiniVec<Rc<()>> = MiniVec::new();
        for _ in 0..6 {
            v.push(r.clone());
        }
        // Refs: original + 6 in vec = 7.
        assert_eq!(Rc::strong_count(&r), 7);
        v.dedup();
        // After dedup we keep 1 copy; the other 5 must be dropped.
        assert_eq!(v.len(), 1);
        assert_eq!(Rc::strong_count(&r), 2);
        drop(v);
        assert_eq!(Rc::strong_count(&r), 1);
    }

    #[test]
    fn drain_full_range() {
        let mut v: MiniVec<u32> = (0..5).collect();
        let drained: Vec<u32> = v.drain(..).collect();
        assert_eq!(drained, vec![0, 1, 2, 3, 4]);
        assert_eq!(v.len(), 0);
    }

    #[test]
    fn drain_middle_range() {
        let mut v: MiniVec<u32> = (0..10).collect();
        let drained: Vec<u32> = v.drain(3..7).collect();
        assert_eq!(drained, vec![3, 4, 5, 6]);
        assert_eq!(v.as_slice(), &[0, 1, 2, 7, 8, 9]);
    }

    #[test]
    fn drain_empty_range_no_op() {
        let mut v: MiniVec<u32> = (0..5).collect();
        let drained: Vec<u32> = v.drain(2..2).collect();
        assert!(drained.is_empty());
        assert_eq!(v.as_slice(), &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn drain_unconsumed_still_removes() {
        // Even if the iterator is dropped without being consumed, the
        // elements in the range must be removed and the tail shifted.
        let mut v: MiniVec<u32> = (0..10).collect();
        drop(v.drain(2..6));
        assert_eq!(v.as_slice(), &[0, 1, 6, 7, 8, 9]);
    }

    #[test]
    fn drain_drops_unconsumed_elements() {
        // Unconsumed elements in the drain range must still be dropped.
        let r = Rc::new(());
        let mut v: MiniVec<Rc<()>> = MiniVec::new();
        for _ in 0..5 {
            v.push(r.clone());
        }
        assert_eq!(Rc::strong_count(&r), 6);
        drop(v.drain(1..4));
        // 3 dropped (range), 2 kept in vec, plus the original.
        assert_eq!(Rc::strong_count(&r), 3);
        assert_eq!(v.len(), 2);
        drop(v);
        assert_eq!(Rc::strong_count(&r), 1);
    }

    #[test]
    fn drain_partial_consume_drops_remainder() {
        // Consume a prefix of the drained iterator; the rest must still be
        // dropped, and the tail shifted in correctly.
        let r = Rc::new(());
        let mut v: MiniVec<Rc<()>> = (0..6).map(|_| r.clone()).collect();
        assert_eq!(Rc::strong_count(&r), 7);
        {
            let mut iter = v.drain(1..5);
            // Consume the first two of the four-element range.
            drop(iter.next());
            drop(iter.next());
        } // drop iter — should drop the remaining 2 in the range
        assert_eq!(v.len(), 2);
        // 4 dropped from the drain range, 2 kept in v, + 1 original = 3.
        assert_eq!(Rc::strong_count(&r), 3);
        drop(v);
        assert_eq!(Rc::strong_count(&r), 1);
    }

    #[test]
    fn drain_extended_mode_full() {
        let mut v: MiniVec<u32> = (0..1000).collect();
        assert!(v.is_extended());
        let drained: Vec<u32> = v.drain(..).collect();
        assert_eq!(drained.len(), 1000);
        assert_eq!(v.len(), 0);
    }

    #[test]
    fn drain_leaked_does_not_double_drop() {
        // mem::forget on the Drain leaks the in-range elements but must
        // not leave the vec in an unsafe state: subsequent drop of v must
        // not double-drop or read freed memory. The vec's len was lowered
        // to the start of the drain range when the iterator was created,
        // so the trailing elements are simply forgotten.
        struct DropCounter<'a>(&'a Cell<usize>);
        impl Drop for DropCounter<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let count = Cell::new(0);
        let mut v: MiniVec<DropCounter<'_>> = (0..5).map(|_| DropCounter(&count)).collect();
        std::mem::forget(v.drain(1..4));
        // Vec length is now 1 (start of drain range). The 3 in-range and
        // 1 tail elements are leaked.
        assert_eq!(v.len(), 1);
        assert_eq!(count.get(), 0);
        drop(v);
        // Only the 1 element still tracked by v gets dropped; rest leaked.
        assert_eq!(
            count.get(),
            1,
            "only the live element is dropped; forgotten elements are neither dropped nor double-dropped"
        );
    }
}
