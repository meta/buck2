/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Pointer-derived identity for immortal allocations.
//!
//! One object can carry identities at several scopes. The durable end of the
//! ladder is `ContentIdentity` (`crate::strong_hasher`): a hash of content,
//! valid across processes. `StaticPointerIdentity` is the cheap, volatile
//! end: an address stands in for the object. It exists only for pointees
//! whose canonical allocation is immortal within this process (insertion-only
//! interners that never evict) — immortality is what makes an address a
//! sound identity, since it can never be recycled. The type implements
//! `Hash`/`Eq` for in-memory cache keys and deliberately not `StrongHash`,
//! `Pagable`, or serde: the absence of the durable traits is load-bearing,
//! and the only way from a pointer identity back to a durable one is
//! re-deriving from content.

use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;
use std::marker::PhantomData;
use std::ptr;

use dupe::Dupe;

/// Mix an address into a table-ready hash. Addresses are aligned, so their
/// low bits are constant, and hash tables take their bucket index from the
/// low bits: fold the high half down so every output bit depends on the
/// address.
pub fn mix_addr(addr: usize) -> u64 {
    let x = (addr as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^ (x >> 32)
}

/// Identity of a `T` whose canonical allocation is immortal within this
/// process. Valid for the remaining process lifetime — it may key
/// daemon-lifetime caches — but meaningless to any other process.
pub struct StaticPointerIdentity<T: ?Sized>(usize, PhantomData<fn() -> T>);

impl<T: ?Sized> StaticPointerIdentity<T> {
    /// `canonical` must borrow the object's canonical allocation:
    /// identity equality is only as meaningful as the pointee's
    /// canonicalization. The `'static` bound is load-bearing — an
    /// address is a sound identity only if its allocation can never be
    /// freed, so construction demands the borrow only such an
    /// allocation can honestly provide. The pointee type is
    /// unconstrained because the canonical allocation is typically an
    /// internal representation of the branded `T`, not a `T` itself.
    pub fn new<P: ?Sized>(canonical: &'static P) -> Self {
        Self(ptr::from_ref(canonical).cast::<()>().addr(), PhantomData)
    }
}

impl<T: ?Sized> allocative::Allocative for StaticPointerIdentity<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        visitor.visit_simple_sized::<Self>();
    }
}

impl<T: ?Sized> Clone for StaticPointerIdentity<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for StaticPointerIdentity<T> {}

impl<T: ?Sized> Dupe for StaticPointerIdentity<T> {}

impl<T: ?Sized> PartialEq for StaticPointerIdentity<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: ?Sized> Eq for StaticPointerIdentity<T> {}

impl<T: ?Sized> Hash for StaticPointerIdentity<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(mix_addr(self.0));
    }
}

impl<T: ?Sized> fmt::Debug for StaticPointerIdentity<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StaticPointerIdentity")
            .field(&self.0)
            .finish()
    }
}
