/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::alloc;
use std::alloc::Layout;
use std::cmp::Ordering;
use std::hash::Hash;
use std::hash::Hasher;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::ptr::NonNull;
use std::slice;
use std::str;
use std::sync::atomic;
use std::sync::atomic::AtomicU32;

use allocative::Allocative;
use dupe::Dupe;
use strong_hash::StrongHash;

use crate::rtabort;

/// Describes the strategy that an `ArcStr`-like implementation uses to allocate its length.
///
/// # Safety
///
/// - The `pack` and `unpack` implementations must roundtrip,
///   i.e. `unpack_len(pack_len(x))` must equal `x`.
/// - `unpack_len(EMPTY)` must return zero.
/// - `inner_empty()` must return a `static` holding `ArcStrBaseInner::empty()`.
pub(crate) unsafe trait ArcStrLenStrategy: Sized + 'static {
    /// Payload allocated with the string data.
    type AllocatedPayload: Copy + Allocative;
    /// Payload stored in the string value.
    type ValuePayload: Copy;
    /// Payload for an empty string.
    const EMPTY: (Self::AllocatedPayload, Self::ValuePayload);

    /// Get the length of the string.
    fn unpack_len(d: (Self::AllocatedPayload, Self::ValuePayload)) -> u32;
    /// Pack the length of the string into either of payloads.
    fn pack_len(len: u32) -> (Self::AllocatedPayload, Self::ValuePayload);

    /// Empty string data. Must be statically allocated,
    /// and Rust does not allow static allocation in generic code.
    fn inner_empty() -> &'static ArcStrBaseInner<Self>;
}

#[repr(C)]
#[derive(Allocative)]
#[allocative(bound = "")]
pub(crate) struct ArcStrBaseInner<P: ArcStrLenStrategy> {
    pub(crate) refcount: AtomicU32,
    pub(crate) allocated_payload: P::AllocatedPayload,
    // data: [u8],
}

impl<P: ArcStrLenStrategy> ArcStrBaseInner<P> {
    const OFFSET_OF_DATA: usize = mem::size_of::<Self>();

    /// The header shared by all empty strings, so that they need no allocation. Its refcount
    /// stays 0: `Clone` and `Drop` skip the count for empty strings.
    pub(crate) const fn empty() -> Self {
        ArcStrBaseInner {
            refcount: AtomicU32::new(0),
            allocated_payload: P::EMPTY.0,
        }
    }

    fn layout_for_len(len: usize) -> Layout {
        let size = Self::OFFSET_OF_DATA.checked_add(len).unwrap();
        let align = mem::align_of::<Self>();
        Layout::from_size_align(size, align).unwrap()
    }

    unsafe fn layout_for_len_unchecked(len: usize) -> Layout {
        let size = Self::OFFSET_OF_DATA + len;
        let align = mem::align_of::<Self>();
        unsafe { Layout::from_size_align_unchecked(size, align) }
    }
}

pub(crate) struct ArcStrBase<P: ArcStrLenStrategy> {
    /// Points to the end of `ArcStrBaseInner` struct.
    /// When string is empty, `ArcStrBaseInner` is statically allocated.
    data: NonNull<u8>,
    pub(crate) value_payload: P::ValuePayload,
}

unsafe impl<P: ArcStrLenStrategy> Send for ArcStrBase<P> {}
unsafe impl<P: ArcStrLenStrategy> Sync for ArcStrBase<P> {}

impl<P: ArcStrLenStrategy> ArcStrBase<P> {
    pub(crate) fn inner(&self) -> &ArcStrBaseInner<P> {
        unsafe {
            let ptr = self.data.as_ptr().sub(ArcStrBaseInner::<P>::OFFSET_OF_DATA);
            &*(ptr as *const ArcStrBaseInner<P>)
        }
    }

    pub(crate) fn addr(&self) -> usize {
        self.data.addr().get()
    }

    /// Static strings keep a refcount of 0 and never report unique.
    pub(crate) fn is_unique(&self) -> bool {
        self.inner().refcount.load(atomic::Ordering::Acquire) == 1
    }

    #[inline]
    fn len(&self) -> usize {
        P::unpack_len((self.inner().allocated_payload, self.value_payload)) as usize
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        unsafe {
            let bytes = slice::from_raw_parts(self.data.as_ptr(), self.len());
            str::from_utf8_unchecked(bytes)
        }
    }
}

impl<P: ArcStrLenStrategy> PartialEq for ArcStrBase<P> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.data.as_ptr(), other.data.as_ptr()) || self.as_str() == other.as_str()
    }
}

impl<P: ArcStrLenStrategy> Eq for ArcStrBase<P> {}

impl<P: ArcStrLenStrategy> Hash for ArcStrBase<P> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state)
    }
}

impl<P: ArcStrLenStrategy> StrongHash for ArcStrBase<P> {
    #[inline]
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().strong_hash(state)
    }
}

impl<P: ArcStrLenStrategy> PartialOrd for ArcStrBase<P> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<P: ArcStrLenStrategy> Ord for ArcStrBase<P> {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl<P: ArcStrLenStrategy> Default for ArcStrBase<P> {
    #[inline]
    fn default() -> ArcStrBase<P> {
        ArcStrBase {
            // SAFETY: One past the end of the static, which is where its (empty) data would be.
            data: unsafe { NonNull::from(P::inner_empty()).add(1).cast::<u8>() },
            value_payload: P::EMPTY.1,
        }
    }
}

impl<P: ArcStrLenStrategy> Clone for ArcStrBase<P> {
    #[inline]
    fn clone(&self) -> ArcStrBase<P> {
        if !self.is_empty() {
            // This is what `Arc::clone` does.
            let old_count = self
                .inner()
                .refcount
                .fetch_add(1, atomic::Ordering::Relaxed);
            const MAX_REFCOUNT: u32 = i32::MAX as u32;
            if old_count > MAX_REFCOUNT {
                rtabort!("refcount overflow");
            }
        }
        ArcStrBase {
            data: self.data,
            value_payload: self.value_payload,
        }
    }
}

impl<P: ArcStrLenStrategy> Dupe for ArcStrBase<P> {}

impl<P: ArcStrLenStrategy> Drop for ArcStrBase<P> {
    #[inline]
    fn drop(&mut self) {
        let len = self.len();
        if len != 0 {
            unsafe {
                let arc_str_inner = self
                    .data
                    .as_ptr()
                    .sub(ArcStrBaseInner::<P>::OFFSET_OF_DATA)
                    .cast::<ArcStrBaseInner<P>>();
                // Again, copy-paste from `Arc::drop`.
                if (*arc_str_inner)
                    .refcount
                    .fetch_sub(1, atomic::Ordering::Release)
                    != 1
                {
                    return;
                }
                atomic::fence(atomic::Ordering::Acquire);
                let layout = ArcStrBaseInner::<P>::layout_for_len_unchecked(len);
                alloc::dealloc(arc_str_inner.cast::<u8>(), layout);
            }
        }
    }
}

impl<'a, P: ArcStrLenStrategy> From<&'a str> for ArcStrBase<P> {
    fn from(s: &'a str) -> ArcStrBase<P> {
        if s.is_empty() {
            ArcStrBase::default()
        } else {
            let layout = ArcStrBaseInner::<P>::layout_for_len(s.len());
            let len: u32 = s.len().try_into().unwrap();
            let (allocated_payload, value_payload) = P::pack_len(len);
            unsafe {
                let alloc = alloc::alloc(layout);
                if alloc.is_null() {
                    alloc::handle_alloc_error(layout);
                }
                ptr::write(
                    alloc.cast::<ArcStrBaseInner<P>>(),
                    ArcStrBaseInner {
                        refcount: AtomicU32::new(1),
                        allocated_payload,
                    },
                );
                let data = alloc.add(ArcStrBaseInner::<P>::OFFSET_OF_DATA);
                ptr::copy_nonoverlapping(s.as_ptr(), data, s.len());
                ArcStrBase {
                    data: NonNull::new(data).unwrap(),
                    value_payload,
                }
            }
        }
    }
}

impl<P: ArcStrLenStrategy> Allocative for ArcStrBase<P> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        if !self.is_empty() {
            if let Some(mut visitor) = visitor.enter_shared(
                allocative::Key::new("data"),
                mem::size_of::<*const u8>(),
                self.data.as_ptr() as *const (),
            ) {
                {
                    struct ArcStrInner<P>(PhantomData<P>);

                    let mut visitor = visitor.enter(
                        allocative::Key::for_type_name::<ArcStrInner<P>>(),
                        ArcStrBaseInner::<P>::layout_for_len(self.len()).size(),
                    );
                    visitor.visit_field(allocative::Key::new("header"), self.inner());
                    visitor.visit_field(allocative::Key::new("str"), self.as_str());
                    visitor.exit();
                }
                visitor.exit();
            }
        }
        visitor.exit();
    }
}
