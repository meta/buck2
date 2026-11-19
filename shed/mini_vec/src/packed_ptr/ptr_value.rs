/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::Arc;

use crate::packed_ptr::repr::Raw;

mod sealed {
    pub trait Sealed {}
}

/// Storage bridge for `PackedPtr`.
///
/// For implementors of `PointerValue`, the only thing to know is that you should set `Storage =
/// PackedStorageRef<Self>` if `Self` is `Copy` and `PackedStorageOwned<Self>` otherwise.
///
/// # Why this exists
///
/// Rust's `Drop` and `Copy` are mutually exclusive on a single type, but we want `PackedPtr<&T>`
/// (which doesn't own anything) to be `Copy`, while `PackedPtr<Box<T>>` (which owns its allocation)
/// needs `Drop` so the `Box` is freed when the `PackedPtr` goes out of scope. There is no way to
/// make a single struct conditionally `Copy` xor `Drop` based on a generic parameter — so we use
/// *two* distinct concrete storage types ([`PackedStorageRef`], non-`Drop` and `Copy`-able;
/// [`PackedStorageOwned`], `Drop`-ing and never `Copy`) and let each [`PointerValue`] implementor
/// pick which one it wants via its `Storage` associated type.
pub trait PackedStorage: Sized + sealed::Sealed {
    /// Little helper to generate "derived" instances of this type.
    ///
    /// Basically, if the implementor of this trait is `Foo<X>`, this is `Foo<N>`.
    type Gen<N: PointerValue>: PackedStorage;
    /// Produce an instance of this storage
    fn from_raw(raw: Raw) -> Self;
    /// Get the underlying data
    fn raw(&self) -> Raw;
}

impl<P: PointerValue> sealed::Sealed for PackedStorageRef<P> {}
impl<P: PointerValue> sealed::Sealed for PackedStorageOwned<P> {}

/// See `PackedStorage`.
#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct PackedStorageRef<P: PointerValue>(pub(super) Raw, pub(super) PhantomData<P>);

impl<P: PointerValue> PackedStorage for PackedStorageRef<P> {
    type Gen<N: PointerValue> = PackedStorageRef<N>;

    fn from_raw(raw: Raw) -> Self {
        Self(raw, PhantomData)
    }

    fn raw(&self) -> Raw {
        self.0
    }
}

/// See `PackedStorage`.
#[repr(transparent)]
pub struct PackedStorageOwned<P: PointerValue>(Raw, PhantomData<P>);

impl<P: PointerValue> PackedStorage for PackedStorageOwned<P> {
    type Gen<N: PointerValue> = PackedStorageOwned<N>;

    fn from_raw(raw: Raw) -> Self {
        Self(raw, PhantomData)
    }

    fn raw(&self) -> Raw {
        self.0
    }
}

/// A type that is a pointer value.
///
/// Intuitively, you should think of this trait as representing a `&T`, `Box<T>`, `Arc<T>`, etc.
///
/// Strictly speaking, implementing this trait has no pre-conditions on the layout of the `Self`
/// type per-se. The only pre-condition is that values of this type can be losslessly converted to a
/// `NonNull<u8>` which looks like a plausible valid pointer. Concretely, that basically means it
/// should be either 1) the address of an actual allocation, or 2) `NonNull::dangling()`.
pub trait PointerValue: 'static + Sized {
    /// Compile-time check that `Self` occupies exactly one machine pointer's
    /// worth of bytes. This rules out wide pointers (e.g. `&[T]`,
    /// `&dyn Trait`).
    const _CHECK_SIZE: () = {
        assert!(std::mem::size_of::<Self>() == std::mem::size_of::<*const ()>());
    };

    /// The type of reference versions of this owned pointer.
    ///
    /// This is usually `&'a T` for some `T`, but may be things like `ArcBorrow<'a>`.
    type Ref<'a>: Sized;

    /// The packed-pointer storage wrapper.
    ///
    /// Set to `PackedStorageRef` for `Copy` types and otherwise `PackedStorageOwned`.
    type Storage: PackedStorage;

    /// Convert to a valid pointer value.
    fn into_pointer(self) -> NonNull<u8>;

    /// Recover from a valid pointer value.
    ///
    /// # Safety
    ///
    /// Argument must be the return value of a previous `into_pointer` call and must obey any
    /// lifetime/non-copy requirements associated with `Self`.
    unsafe fn from_pointer_owned(ptr: NonNull<u8>) -> Self;

    /// Recover from a valid pointer value as a borrow of `Target`.
    ///
    /// # Safety
    ///
    /// Argument must be the return value of a previous `into_pointer` call and must obey any
    /// lifetime/non-copy requirements associated with `Self`.
    unsafe fn from_pointer_ref<'a>(ptr: NonNull<u8>) -> Self::Ref<'a>;
}

/// A type that is an *allocated* pointer value.
///
/// This has all of the same requirements as [`PointerValue`] but additionally guarantees the
/// pointer value is never `NonNull::dangling()`; implementing this on `P` gives you an impl of
/// `PointerValue` for `Option<P>`.
pub trait PointerValueAllocated: PointerValue {
    /// Hook for impls to assert any constraints needed to uphold the "never dangling" invariant
    /// (e.g. `size_of::<T> > 0` for `Box<T>` / `&'static T`).
    ///
    /// Evaluated transitively via `<Option<Self> as PointerValue>::_CHECK_SIZE`, which in turn
    /// fires at every `PackedPtr<Option<Self>>::new` call site.
    const _FOR_ASSERT: () = ();
}

impl<P: PointerValueAllocated> PointerValue for Option<P> {
    /// The whole point of `PointerValueAllocated` is to license the `None ↔
    /// NonNull::dangling()` encoding below — force the impl's
    /// invariant assertion (e.g. `size_of::<T> > 0`) and propagate `P`'s
    /// own size check so nested wrappers don't lose transitive verification.
    const _CHECK_SIZE: () = {
        let _ = <P as PointerValueAllocated>::_FOR_ASSERT;
        let _ = <P as PointerValue>::_CHECK_SIZE;
        assert!(std::mem::size_of::<Self>() == std::mem::size_of::<*const ()>());
    };

    type Ref<'a> = Option<P::Ref<'a>>;

    type Storage = <P::Storage as PackedStorage>::Gen<Option<P>>;

    fn into_pointer(self) -> NonNull<u8> {
        match self {
            Some(p) => p.into_pointer(),
            None => NonNull::dangling(),
        }
    }

    unsafe fn from_pointer_owned(ptr: NonNull<u8>) -> Self {
        if ptr == NonNull::dangling() {
            None
        } else {
            // SAFETY: We're just a wrapper over the underlying thing
            unsafe { Some(P::from_pointer_owned(ptr)) }
        }
    }

    unsafe fn from_pointer_ref<'a>(ptr: NonNull<u8>) -> Self::Ref<'a> {
        if ptr == NonNull::dangling() {
            None
        } else {
            // SAFETY: We're just a wrapper over the underlying thing
            unsafe { Some(P::from_pointer_ref(ptr)) }
        }
    }
}

// -- Standard impls

// SAFETY of the impls below: each `into_pointer` returns a `NonNull<u8>`
// whose address is the original pointer's address with provenance over
// that allocation; `from_pointer_*` round-trip through the matching
// `Box::from_raw` / `Arc::from_raw` / `NonNull::as_ref`, preserving
// provenance and never producing a null reference.

impl<T: 'static> PointerValue for NonNull<T> {
    type Ref<'a> = NonNull<T>;
    type Storage = PackedStorageRef<Self>;

    fn into_pointer(self) -> NonNull<u8> {
        self.cast::<u8>()
    }

    unsafe fn from_pointer_owned(ptr: NonNull<u8>) -> Self {
        ptr.cast::<T>()
    }

    unsafe fn from_pointer_ref<'a>(ptr: NonNull<u8>) -> Self::Ref<'a> {
        ptr.cast::<T>()
    }
}

impl<T: 'static> PointerValue for Box<T> {
    type Ref<'a> = &'a T;
    type Storage = PackedStorageOwned<Self>;

    fn into_pointer(self) -> NonNull<u8> {
        // SAFETY: `Box::into_raw` returns a non-null pointer.
        unsafe { NonNull::new_unchecked(Box::into_raw(self).cast::<u8>()) }
    }

    unsafe fn from_pointer_owned(ptr: NonNull<u8>) -> Self {
        // SAFETY: `ptr` came from `Box::into_raw` (via `into_pointer`)
        // and per caller contract has not been freed.
        unsafe { Box::from_raw(ptr.cast::<T>().as_ptr()) }
    }

    unsafe fn from_pointer_ref<'a>(ptr: NonNull<u8>) -> &'a T {
        // SAFETY: `ptr` points to the `T` owned by an existing `Box`;
        // the returned reference's lifetime is bounded by the caller.
        unsafe { ptr.cast::<T>().as_ref() }
    }
}

/// `Box`s are allocated if the size > 0
impl<T: 'static> PointerValueAllocated for Box<T> {
    const _FOR_ASSERT: () = assert!(std::mem::size_of::<T>() > 0);
}

impl<T: 'static> PointerValue for Arc<T> {
    type Ref<'a> = &'a T;
    type Storage = PackedStorageOwned<Self>;

    fn into_pointer(self) -> NonNull<u8> {
        // SAFETY: `Arc::into_raw` returns a non-null pointer.
        unsafe { NonNull::new_unchecked(Arc::into_raw(self).cast::<u8>().cast_mut()) }
    }

    unsafe fn from_pointer_owned(ptr: NonNull<u8>) -> Self {
        // SAFETY: `ptr` came from `Arc::into_raw` (via `into_pointer`).
        unsafe { Arc::from_raw(ptr.cast::<T>().as_ptr()) }
    }

    unsafe fn from_pointer_ref<'a>(ptr: NonNull<u8>) -> &'a T {
        // SAFETY: `ptr` points to the `T` inside an existing `Arc`.
        unsafe { ptr.cast::<T>().as_ref() }
    }
}

/// `Arc`s are always allocated
impl<T: 'static> PointerValueAllocated for Arc<T> {}

impl<T: 'static> PointerValue for &'static T {
    type Ref<'a> = &'a T;
    type Storage = PackedStorageRef<Self>;

    fn into_pointer(self) -> NonNull<u8> {
        NonNull::from(self).cast::<u8>()
    }

    unsafe fn from_pointer_owned(ptr: NonNull<u8>) -> Self {
        // SAFETY: `ptr` came from `NonNull::from(&'static T)`; the referent
        // outlives any caller because it is `'static`.
        unsafe { ptr.cast::<T>().as_ref() }
    }

    unsafe fn from_pointer_ref<'a>(ptr: NonNull<u8>) -> &'a T {
        // SAFETY: same as above; the returned lifetime is bounded by the
        // caller.
        unsafe { ptr.cast::<T>().as_ref() }
    }
}

/// `&'static T` is allocated iff `T` is non-ZST — for ZST `T`, `NonNull::from(&T)`
/// equals `NonNull::dangling()` and the `Option` `None`/`Some` discriminant collapses.
impl<T: 'static> PointerValueAllocated for &'static T {
    const _FOR_ASSERT: () = assert!(std::mem::size_of::<T>() > 0);
}
