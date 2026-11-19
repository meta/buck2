/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Describes the types we can store in `LockFreeRawTable`.

use std::num::NonZero;
use std::num::NonZeroU32;
use std::num::NonZeroU64;
use std::ptr::NonNull;
use std::sync::Arc;

/// Generalized non-null pointer.
///
/// Anything which can be stored in `LockFreeRawTable`. The table stores each value as one
/// pointer-sized word, held in an `AtomicPtr`, so values that are pointers keep their provenance.
/// Values that are not pointers travel as the address of a pointer without provenance.
pub trait AtomicValue {
    /// Dereferenced value.
    type Ref<'a>: Copy
    where
        Self: 'a;

    /// The word the table stores. Null marks an empty slot, hence `NonNull`.
    fn into_raw(this: Self) -> NonNull<()>;
    /// Construct the value from its word.
    unsafe fn from_raw(raw: NonNull<()>) -> Self;
    /// Dereference the word.
    unsafe fn deref<'a>(raw: NonNull<()>) -> Self::Ref<'a>;
}

impl<T> AtomicValue for Box<T> {
    type Ref<'a>
        = &'a T
    where
        Self: 'a;

    #[inline]
    fn into_raw(this: Self) -> NonNull<()> {
        NonNull::from(Box::leak(this)).cast()
    }

    #[inline]
    unsafe fn from_raw(raw: NonNull<()>) -> Self {
        unsafe { Box::from_raw(raw.cast::<T>().as_ptr()) }
    }

    #[inline]
    unsafe fn deref<'a>(raw: NonNull<()>) -> Self::Ref<'a> {
        unsafe { raw.cast::<T>().as_ref() }
    }
}

impl<T> AtomicValue for Arc<T> {
    type Ref<'a>
        = &'a T
    where
        Self: 'a;

    #[inline]
    fn into_raw(this: Self) -> NonNull<()> {
        // SAFETY: `Arc::into_raw` never returns null.
        unsafe { NonNull::new_unchecked(Arc::into_raw(this).cast_mut()).cast() }
    }

    #[inline]
    unsafe fn from_raw(raw: NonNull<()>) -> Self {
        unsafe { Arc::from_raw(raw.cast::<T>().as_ptr()) }
    }

    #[inline]
    unsafe fn deref<'a>(raw: NonNull<()>) -> Self::Ref<'a> {
        unsafe { raw.cast::<T>().as_ref() }
    }
}

#[cfg(target_pointer_width = "64")]
impl AtomicValue for NonZeroU64 {
    type Ref<'a> = NonZeroU64;

    #[inline]
    fn into_raw(this: NonZeroU64) -> NonNull<()> {
        // SAFETY: a non-zero `u64` is a non-zero `usize` on this platform.
        NonNull::without_provenance(unsafe { NonZero::new_unchecked(this.get() as usize) })
    }

    #[inline]
    unsafe fn from_raw(raw: NonNull<()>) -> NonZeroU64 {
        unsafe { NonZeroU64::new_unchecked(raw.addr().get() as u64) }
    }

    #[inline]
    unsafe fn deref<'a>(raw: NonNull<()>) -> Self::Ref<'a> {
        unsafe { Self::from_raw(raw) }
    }
}

impl AtomicValue for NonZeroU32 {
    type Ref<'a> = NonZeroU32;

    #[inline]
    fn into_raw(this: NonZeroU32) -> NonNull<()> {
        // SAFETY: a non-zero `u32` is a non-zero `usize` on every platform this crate
        // supports.
        NonNull::without_provenance(unsafe { NonZero::new_unchecked(this.get() as usize) })
    }

    #[inline]
    unsafe fn from_raw(raw: NonNull<()>) -> NonZeroU32 {
        unsafe { NonZeroU32::new_unchecked(raw.addr().get() as u32) }
    }

    #[inline]
    unsafe fn deref<'a>(raw: NonNull<()>) -> Self::Ref<'a> {
        unsafe { Self::from_raw(raw) }
    }
}

/// Raw pointers stored in the table.
#[derive(Copy, Clone)]
pub struct RawPtr<T>(pub NonNull<T>);

impl<T> AtomicValue for RawPtr<T> {
    type Ref<'a>
        = NonNull<T>
    where
        Self: 'a;

    #[inline]
    fn into_raw(this: RawPtr<T>) -> NonNull<()> {
        this.0.cast()
    }

    #[inline]
    unsafe fn from_raw(raw: NonNull<()>) -> RawPtr<T> {
        RawPtr(raw.cast())
    }

    #[inline]
    unsafe fn deref<'a>(raw: NonNull<()>) -> Self::Ref<'a>
    where
        Self: 'a,
    {
        raw.cast()
    }
}
