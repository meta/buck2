/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A one-word, weak-less `Arc`: `triomphe::Arc` behind a newtype that also implements the traits
//! the lock-free containers of dice need.

use std::ops::Deref;
use std::ptr::NonNull;

use allocative::Allocative;
use arc_swap::RefCnt;
use dupe::Dupe;
use lock_free_hashtable::atomic_value::AtomicValue;
use mini_vec::packed_ptr::PackedStorageOwned;
use mini_vec::packed_ptr::PointerValue;
use mini_vec::packed_ptr::PointerValueAllocated;

#[derive(Debug, Eq, PartialEq, Hash, Allocative)]
pub struct Arc<T: ?Sized>(triomphe::Arc<T>);

impl<T> Arc<T> {
    #[inline]
    pub fn new(value: T) -> Self {
        Arc(triomphe::Arc::new(value))
    }

    #[inline]
    pub fn borrow_arc(&self) -> ArcBorrow<'_, T> {
        ArcBorrow(self.0.borrow_arc())
    }
}

impl<T: ?Sized> Arc<T> {
    #[inline]
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        triomphe::Arc::ptr_eq(&this.0, &other.0)
    }
}

impl<T: Clone> Arc<T> {
    #[inline]
    pub fn make_mut(&mut self) -> &mut T {
        triomphe::Arc::make_mut(&mut self.0)
    }
}

impl<T: ?Sized> Clone for Arc<T> {
    #[inline]
    fn clone(&self) -> Self {
        Arc(self.0.clone())
    }
}

impl<T: ?Sized> Dupe for Arc<T> {}

unsafe impl<T> RefCnt for Arc<T> {
    type Base = T;

    #[inline]
    fn into_ptr(me: Self) -> *mut Self::Base {
        <triomphe::Arc<T> as RefCnt>::into_ptr(me.0)
    }

    #[inline]
    fn as_ptr(me: &Self) -> *mut Self::Base {
        <triomphe::Arc<T> as RefCnt>::as_ptr(&me.0)
    }

    #[inline]
    unsafe fn from_ptr(ptr: *const Self::Base) -> Self {
        Self(unsafe { <triomphe::Arc<T> as RefCnt>::from_ptr(ptr) })
    }
}

impl<T: ?Sized> Deref for Arc<T> {
    type Target = triomphe::Arc<T>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// Not `?Sized` because triomphe doesn't really support it
pub struct ArcBorrow<'a, T>(triomphe::ArcBorrow<'a, T>);

impl<'a, T> ArcBorrow<'a, T> {
    #[inline]
    pub fn clone_arc(self) -> Arc<T> {
        Arc(self.0.clone_arc())
    }

    /// # Safety
    ///
    /// `raw` must point to the `T` inside a live `triomphe::Arc<T>` whose count stays positive
    /// for `'a`.
    #[inline]
    pub unsafe fn from_ptr(raw: *const T) -> Self {
        ArcBorrow(unsafe { triomphe::ArcBorrow::from_ptr(raw) })
    }

    #[inline]
    pub fn get(self) -> &'a T {
        self.0.get()
    }
}

impl<T> Clone for ArcBorrow<'_, T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Dupe for ArcBorrow<'_, T> {}

impl<T> Copy for ArcBorrow<'_, T> {}

impl<'a, T> Deref for ArcBorrow<'a, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> AtomicValue for Arc<T> {
    type Ref<'a>
        = ArcBorrow<'a, T>
    where
        Self: 'a;

    #[inline]
    fn into_raw(this: Self) -> NonNull<()> {
        // SAFETY: `Arc::into_raw` never returns null.
        unsafe { NonNull::new_unchecked(triomphe::Arc::into_raw(this.0).cast_mut()).cast() }
    }

    #[inline]
    unsafe fn from_raw(raw: NonNull<()>) -> Self {
        Arc(unsafe { triomphe::Arc::from_raw(raw.cast::<T>().as_ptr()) })
    }

    #[inline]
    unsafe fn deref<'a>(raw: NonNull<()>) -> Self::Ref<'a> {
        unsafe { ArcBorrow::from_ptr(raw.cast::<T>().as_ptr()) }
    }
}

impl<T: 'static> PointerValue for Arc<T> {
    type Ref<'a> = &'a T;
    type Storage = PackedStorageOwned<Self>;

    fn into_pointer(self) -> NonNull<u8> {
        // SAFETY: `Arc::into_raw` returns a non-null pointer.
        unsafe { NonNull::new_unchecked(triomphe::Arc::into_raw(self.0).cast::<u8>().cast_mut()) }
    }

    unsafe fn from_pointer_owned(ptr: NonNull<u8>) -> Self {
        // SAFETY: `ptr` came from `Arc::into_raw` (via `into_pointer`).
        unsafe { Self(triomphe::Arc::from_raw(ptr.cast::<T>().as_ptr())) }
    }

    unsafe fn from_pointer_ref<'a>(ptr: NonNull<u8>) -> &'a T {
        // SAFETY: `ptr` points to the `T` inside an existing `Arc`.
        unsafe { ptr.cast::<T>().as_ref() }
    }
}

impl<T: 'static> PointerValueAllocated for Arc<T> {}
