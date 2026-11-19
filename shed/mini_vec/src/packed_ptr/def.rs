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

use crate::packed_ptr::ptr_value::PackedStorage;
use crate::packed_ptr::ptr_value::PackedStorageOwned;
use crate::packed_ptr::ptr_value::PackedStorageRef;
use crate::packed_ptr::ptr_value::PointerValue;
use crate::packed_ptr::repr;

impl<P: PointerValue> Drop for PackedStorageOwned<P> {
    fn drop(&mut self) {
        let raw = repr::unpack_ptr(self.raw());
        // SAFETY: `raw` is the value originally returned by `into_pointer` (held inside the packed
        // encoding); we are inside `Drop`, so taking ownership of the `P` and letting it drop is
        // sound.
        drop(unsafe { P::from_pointer_owned(raw) });
    }
}

impl<P: PointerValue + Clone> Clone for PackedStorageOwned<P> {
    fn clone(&self) -> Self {
        let raw = repr::unpack_ptr(self.raw());
        let extra = repr::unpack_extra(self.raw());
        // Make a `P` so that we can clone it, but we absolutely do not want to drop it (since we
        // don't actually own a `P`).
        let p = std::mem::ManuallyDrop::new(unsafe { P::from_pointer_owned(raw) });
        Self::from_raw(repr::pack(P::clone(&*p).into_pointer(), extra))
    }
}

// === user-facing newtype ===

/// A pointer value carrying a `u8` of metadata.
///
/// By default both occupy a single 64-bit word — the metadata in the pointer's high bits — but with
/// `--cfg mini_vec_no_ptr_packing` they are stored as separate fields. Either way the API and
/// behavior are identical. See the [crate docs](crate) for full details.
#[repr(transparent)]
pub struct PackedPtr<P: PointerValue>(<P as PointerValue>::Storage);

impl<P: PointerValue> PackedPtr<P> {
    /// Combine `ptr` and `extra` metadata into the storage representation.
    ///
    /// # Panics
    ///
    /// Unless built with `mini_vec_no_ptr_packing`, panics if the top 8 bits of the pointer are
    /// non-zero.
    ///
    /// For the general platform support story, see the [crate docs](crate), though there's one
    /// additional thing worth calling out here: The platform support story at the crate level is
    /// written with *allocated data pointers* in mind. It is far more common for this precondition
    /// to be false of other kinds of pointers, such as vtable or instruction/function pointers on
    /// any platforms with Arm PAC, including much Apple silicone.
    ///
    /// If your `P = Box` or `P = Arc` this is a non-issue, but with `P = NonNull<u8>` for example,
    /// more care is needed.
    pub fn new(ptr: P, extra: u8) -> Self {
        let _ = <P as PointerValue>::_CHECK_SIZE;
        let packed = repr::pack(ptr.into_pointer(), extra);
        Self(<P::Storage as PackedStorage>::from_raw(packed))
    }

    /// Read the packed `u8` metadata.
    #[inline]
    pub fn extra(&self) -> u8 {
        repr::unpack_extra(self.0.raw())
    }

    /// Replace the packed `u8` metadata in place. Does not touch the
    /// underlying pointer value.
    #[inline]
    pub fn set_extra(&mut self, extra: u8) {
        let new_raw = repr::with_extra(self.0.raw(), extra);
        // The replacement storage holds the same pointer as the old one, so
        // forget the old storage to keep its `Drop` (for owning storage) from
        // freeing a pointer the replacement now owns.
        std::mem::forget(std::mem::replace(
            &mut self.0,
            P::Storage::from_raw(new_raw),
        ))
    }

    /// Borrow the packed pointer's target. The borrow is tied to `&self`.
    #[inline]
    pub fn as_ref(&self) -> P::Ref<'_> {
        let raw = repr::unpack_ptr(self.0.raw());
        // SAFETY: `raw` is the value packed at construction time. `self`
        // owns the underlying `P`, so the referent is alive. The lifetime
        // of the returned `Ref<'_>` is bounded by `&self`.
        unsafe { P::from_pointer_ref(raw) }
    }

    /// Run `f` against a borrow of the inner `P` itself (not just its target).
    ///
    /// Unlike [`as_ref`](Self::as_ref), which hands out a `P::Ref<'_>`, this exposes a `&P` making
    /// it a bit easier to use with generics.
    pub(crate) fn with_inner<R>(&self, f: impl FnOnce(&P) -> R) -> R {
        let raw = repr::unpack_ptr(self.0.raw());
        // SAFETY: `raw` came from `into_pointer` at construction time. We wrap the reconstructed `P`
        // in `ManuallyDrop` and only lend out `&P`, so it is never dropped and ownership is not
        // duplicated — `self` remains the sole owner. (Same reconstruct-borrow pattern as the
        // owning storage's `Clone`.)
        let p = std::mem::ManuallyDrop::new(unsafe { P::from_pointer_owned(raw) });
        f(&p) // `&ManuallyDrop<P>` deref-coerces to `&P`
    }

    /// Recover the original pointer, dropping the metadata.
    #[inline]
    pub fn into_inner(self) -> P {
        // `PackedPtr` has no `Drop` impl, so we can move the storage out.
        let storage = self.0;
        let raw = repr::unpack_ptr(storage.raw());
        // Forget the storage so its own `Drop` (for owning storage) does not
        // free the `P` we are about to hand back to the caller.
        std::mem::forget(storage);
        // SAFETY: `raw` came from a previous `into_pointer` call, and we have
        // taken ownership of the underlying `P` by forgetting `storage`, so it
        // will not be freed twice.
        unsafe { P::from_pointer_owned(raw) }
    }

    /// Recover the original pointer and metadata, consuming the
    /// `PackedPtr`.
    pub fn into_parts(self) -> (P, u8) {
        let extra = self.extra();
        (self.into_inner(), extra)
    }
}

impl<P> PackedPtr<P>
where
    P: PointerValue<Storage = PackedStorageRef<P>> + Copy,
{
    /// Construct a `PackedPtr` carrying **zero** metadata from a bare pointer.
    ///
    /// This is the same as `PackedPtr::new(ptr, 0)`, except that it's const compatible.
    ///
    /// # Safety
    ///
    ///  - All the preconditions of `PackedPtr::new`
    ///  - All the preconditions for converting `raw` into a `P`
    pub const unsafe fn from_ptr_zero_extra(raw: NonNull<u8>) -> Self {
        Self(PackedStorageRef(
            repr::from_ptr_zero_extra(raw),
            PhantomData,
        ))
    }

    /// The pointer and the metadata byte as the one pointer the packed representation stores
    /// them in: the original pointer with the metadata in the high bits of its address, so it
    /// is not for dereferencing. Round-trips through
    /// [`from_packed_ptr`](Self::from_packed_ptr).
    ///
    /// Only the packed representation has this. Code that needs its handles to be one word (to
    /// store them in a lock-free table, say) uses it, and thereby does not build with
    /// `mini_vec_no_ptr_packing`; that is the intended way of stating the requirement.
    #[cfg(not(mini_vec_no_ptr_packing))]
    pub fn to_packed_ptr(self) -> NonNull<u8> {
        self.0.0
    }

    /// Recover a `PackedPtr` from [`to_packed_ptr`](Self::to_packed_ptr).
    ///
    /// # Safety
    ///
    /// `ptr` must have been returned by `to_packed_ptr` on a `PackedPtr<P>`, and whatever
    /// requirements `P` places on its pointee must still hold.
    #[cfg(not(mini_vec_no_ptr_packing))]
    pub unsafe fn from_packed_ptr(ptr: NonNull<u8>) -> Self {
        Self(PackedStorageRef(ptr, PhantomData))
    }
}

impl<P: PointerValue> Copy for PackedPtr<P> where P::Storage: Copy {}

impl<P: PointerValue> Clone for PackedPtr<P>
where
    P::Storage: Clone,
{
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

// SAFETY: `PackedRef<P>` semantically owns a `P` (encoded in the packed
// word); like a `P` field, it is `Send`/`Sync` whenever `P` is. The
// `NonNull<u8>` field would otherwise inhibit auto-derivation.
unsafe impl<P: PointerValue + Send> Send for PackedPtr<P> {}
unsafe impl<P: PointerValue + Sync> Sync for PackedPtr<P> {}
