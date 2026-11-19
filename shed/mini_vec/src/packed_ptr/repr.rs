/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The in-memory representation of a pointer paired with a `u8` of metadata.
//!
//! Two interchangeable representations are selected at compile time, both
//! exposing the same interface: an opaque `Raw` value plus functions that build
//! it from a `(pointer, metadata)` pair and read either component back out. The
//! rest of the crate is written against this interface, so it is identical
//! regardless of which representation is in effect.
//!
//! - **packed** (default): a single `NonNull<u8>` with the metadata stored in
//!   the high 8 bits of the address. One machine word, at the cost of requiring
//!   the top 8 address bits to be free (see the crate docs).
//! - **unpacked** (`--cfg mini_vec_no_ptr_packing`): the pointer and the
//!   metadata byte held as separate fields. Two machine words, but makes no
//!   assumption about the pointer's bits, so it is correct on every target —
//!   including the platforms the packed representation cannot support.

#[cfg(not(mini_vec_no_ptr_packing))]
pub(crate) use self::packed::*;
#[cfg(mini_vec_no_ptr_packing)]
pub(crate) use self::unpacked::*;

#[cfg(not(mini_vec_no_ptr_packing))]
mod packed {
    use std::num::NonZeroUsize;
    use std::ptr::NonNull;

    #[cfg(not(target_pointer_width = "64"))]
    compile_error!(
        "the packed representation requires a 64-bit target; build with `--cfg mini_vec_no_ptr_packing` elsewhere"
    );

    /// Number of low bits that hold the pointer address.
    const META_SHIFT: usize = 56;
    const PTR_MASK: usize = (1 << META_SHIFT) - 1;
    /// Mask covering the high 8 bits used for metadata.
    const META_MASK: usize = !PTR_MASK;

    pub(crate) type Raw = NonNull<u8>;

    #[inline]
    pub(crate) fn pack(ptr: NonNull<u8>, extra: u8) -> Raw {
        let addr = ptr.addr().get();
        assert!(
            addr & META_MASK == 0,
            "PackedPtr requires pointer addresses to fit in 56 bits (got {addr:#x})",
        );
        let new_addr = addr | ((extra as usize) << META_SHIFT);
        // SAFETY: `addr` is non-zero (came from a `NonNull`); ORing in
        // additional bits keeps it non-zero.
        let nonzero = unsafe { NonZeroUsize::new_unchecked(new_addr) };
        ptr.with_addr(nonzero)
    }

    #[inline]
    pub(crate) fn unpack_extra(raw: Raw) -> u8 {
        ((raw.addr().get() >> META_SHIFT) & 0xFF) as u8
    }

    #[inline]
    pub(crate) fn with_extra(raw: Raw, extra: u8) -> Raw {
        let addr = raw.addr().get();
        let new_addr = (addr & PTR_MASK) | ((extra as usize) << META_SHIFT);
        // SAFETY: the low 56 bits of `addr` are non-zero (every constructor
        // packs a `NonNull`'s 56-bit address); preserving them keeps `new_addr`
        // non-zero.
        let nonzero = unsafe { NonZeroUsize::new_unchecked(new_addr) };
        raw.with_addr(nonzero)
    }

    #[inline]
    pub(crate) fn unpack_ptr(raw: Raw) -> NonNull<u8> {
        let addr = raw.addr().get() & PTR_MASK;
        // SAFETY: see `with_extra`.
        let nonzero = unsafe { NonZeroUsize::new_unchecked(addr) };
        raw.with_addr(nonzero)
    }

    #[inline]
    pub(crate) const fn from_ptr_zero_extra(ptr: NonNull<u8>) -> Raw {
        ptr
    }
}

#[cfg(mini_vec_no_ptr_packing)]
mod unpacked {
    use std::ptr::NonNull;

    #[derive(Copy, Clone)]
    pub(crate) struct Raw {
        ptr: NonNull<u8>,
        extra: u8,
    }

    #[inline]
    pub(crate) fn pack(ptr: NonNull<u8>, extra: u8) -> Raw {
        Raw { ptr, extra }
    }

    #[inline]
    pub(crate) fn unpack_extra(raw: Raw) -> u8 {
        raw.extra
    }

    #[inline]
    pub(crate) fn with_extra(raw: Raw, extra: u8) -> Raw {
        Raw {
            ptr: raw.ptr,
            extra,
        }
    }

    #[inline]
    pub(crate) fn unpack_ptr(raw: Raw) -> NonNull<u8> {
        raw.ptr
    }

    #[inline]
    pub(crate) const fn from_ptr_zero_extra(ptr: NonNull<u8>) -> Raw {
        Raw { ptr, extra: 0 }
    }
}
