/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Smart pointers that support transparent paging of data to/from storage.
//!
//! This module provides [`PagableArc`] and [`PinnedPagableArc`], smart pointers that
//! allow data to be transparently paged out to storage when memory pressure is high,
//! and paged back in when accessed.
//!
//! # Overview
//!
//! - [`PagableArc<T>`] - A reference-counted pointer that may or may not keep data in memory.
//!   Similar to `Arc<T>`, but data can be evicted when not actively needed.
//!
//! - [`PinnedPagableArc<T>`] - A reference-counted pointer that guarantees data is in memory.
//!   Provides direct access to the data via `Deref`. Similar to holding a "strong" reference.
//!
//! - [`PinnedPagableArcBorrow<'a, T>`] - A borrowed reference to pinned data, avoiding
//!   reference count manipulation for short-lived access.
//!
//! # Usage
//!
//! ```ignore
//! use pagable::{PinnedPagableArc, PagableArc};
//!
//! // Create pinned data (guaranteed in memory)
//! let pinned = PinnedPagableArc::new(MyData::new(), storage);
//!
//! // Access data directly via Deref
//! println!("{:?}", *pinned);
//!
//! // Convert to a PagableArc that can be paged out
//! let pagable = pinned.into_pagable();
//!
//! // Later, when you need to access the data again:
//! let pinned_again = pagable.pin().await?;
//! ```
//!
//! # Pinning and Unpinning
//!
//! Data is kept in memory as long as there are "pinned" references to it:
//! - Every [`PinnedPagableArc`] keeps data pinned
//! - A [`PagableArc`] starts pinned but can be explicitly unpinned via [`unpin()`](PagableArc::unpin)
//!
//! When all pinned references are released, the data may transition to:
//! - **Unpinned**: Data is still in memory but can be evicted
//! - **PagedOut**: Data has been serialized to storage and evicted from memory
//!
//! # Pinning a PagableArc
//!
//! To access data through a [`PagableArc`], you must first "pin" it to get a
//! [`PinnedPagableArc`]:
//!
//! - [`pin()`](PagableArc::pin) - Async; pages in data if needed
//! - [`pin_sync()`](PagableArc::pin_sync) - Blocking; pages in data if needed
//!
//! # Internal Architecture
//!
//! ```text
//!                    ┌─────────────────┐
//!                    │ PagableArcInner │  (shared, one per unique data)
//!                    │  - pinned_count │
//!                    │  - value        │
//!                    │  - storage      │
//!                    └────────▲────────┘
//!                             │
//!           ┌─────────────────┼─────────────────┐
//!           │                 │                 │
//!    ┌──────┴──────┐   ┌──────┴──────┐   ┌──────┴──────┐
//!    │ PagableArc  │   │ PagableArc  │   │ Pinned      │
//!    │ state=      │   │ state=      │   │ PagableArc  │
//!    │  Unpinned   │   │  Pinned     │   │ (pinned)    │
//!    └─────────────┘   └─────────────┘   └─────────────┘
//! ```
//!
//! The `pinned_count` tracks how many references are keeping the data pinned:
//!
//! ```text
//! pinned_count == (# of Pinned PagableArcs) + (# of PinnedPagableArcs)
//! ```
//!
//! ## State Transitions
//!
//! ```text
//!    ┌─────────┐  alloc_pinned   ┌────────┐  release_pin   ┌──────────┐
//!    │ PagedOut│ ───────────────►│ Pinned │───────────────►│ Unpinned │
//!    └─────────┘  (deserialize)  └────────┘ (count→0,      └──────────┘
//!         ▲                                  no key)             │
//!         │                                                      │
//!         └──────────────────────────────────────────────────────┘
//!                        release_pin (count→0, has key)
//! ```

#![allow(unused)]

use std::any::TypeId;
use std::cell::UnsafeCell;
use std::future::Future;
use std::num::NonZeroU128;
use std::num::NonZeroUsize;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use allocative::Visitor;
use dupe::Clone_;
use dupe::Copy_;
use dupe::Dupe;
use dupe::Dupe_;
use either::Either;
use parking_lot::Mutex;
use strong_hash::StrongHash;

use crate::Pagable;
use crate::PagableDeserialize;
use crate::PagableDeserializer;
use crate::PagableEagerDeserialize;
use crate::PagableEagerSerialize;
use crate::PagableSerialize;
use crate::PagableSerializer;
use crate::arc_erase::ArcErase;
use crate::arc_erase::ArcEraseDyn;
use crate::arc_erase::ArcEraseType;
use crate::arc_erase::ArcSerializeOutcome;
use crate::arc_erase::StdArcEraseType;
use crate::arc_erase::StrongAsWeak;
use crate::arc_erase::WeakErase;
use crate::arc_erase::deserialize_arc;
use crate::storage::data::DataKey;
use crate::storage::handle::PagableStorageHandle;
use crate::storage::traits::PagableStorage;

/// A reference-counted smart pointer that supports paging data to/from storage.
///
/// `PagableArc<T>` is similar to `Arc<T>`, but the data it points to can be
/// transparently paged out to storage when memory pressure is high. Unlike
/// [`PinnedPagableArc`], a `PagableArc` does not guarantee that data is in memory.
///
/// # Creating a PagableArc
///
/// Typically, you create a [`PinnedPagableArc`] first (which guarantees data is in memory),
/// then convert it to a `PagableArc` when you no longer need immediate access:
///
/// ```ignore
/// let pinned = PinnedPagableArc::new(data, storage);
/// let pagable = pinned.into_pagable();  // Can now be paged out
/// ```
///
/// # Accessing Data
///
/// To access the data, you must first "pin" it to get a [`PinnedPagableArc`]:
///
/// ```ignore
/// // Async: pages in data if needed
/// let pinned = pagable.pin().await?;
///
/// // Blocking: pages in data if needed (use sparingly)
/// let pinned = pagable.pin_sync()?;
/// ```
///
/// # Pinned vs Unpinned State
///
/// Each `PagableArc` instance tracks whether it is "pinned" (contributing to keeping
/// data in memory) or "unpinned" (allowing data to be evicted):
///
/// - Newly created `PagableArc`s start in the **Pinned** state
/// - Call [`unpin()`](Self::unpin) to transition to **Unpinned**
/// - Pinning transitions back to **Pinned**
///
/// Data remains in memory as long as at least one reference is pinned.
#[derive(allocative::Allocative)]
pub struct PagableArc<T: Pagable> {
    state: PagableArcStateHolder,
    #[allocative(skip)]
    pointer: triomphe::Arc<PagableArcInner<T>>,
}

#[derive(allocative::Allocative)]
struct PagableArcStateHolder(AtomicUsize);

impl PagableArcStateHolder {
    fn get(&self) -> PagableArcState {
        match self.0.load(Ordering::Relaxed) {
            0 => {
                static_assertions::const_assert_eq!(0, PagableArcState::Unpinned as usize);
                PagableArcState::Unpinned
            }
            1 => {
                static_assertions::const_assert_eq!(1, PagableArcState::Pinned as usize);
                PagableArcState::Pinned
            }
            _ => unreachable!(),
        }
    }

    /// Sets the state to Pinned. Returns true if the state changed (was Unpinned).
    fn set_pinned(&self) -> bool {
        self.0
            .compare_exchange(
                PagableArcState::Unpinned as usize,
                PagableArcState::Pinned as usize,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    /// Sets the state to Unpinned. Returns true if the state changed (was Pinned).
    fn set_unpinned(&self) -> bool {
        self.0
            .compare_exchange(
                PagableArcState::Pinned as usize,
                PagableArcState::Unpinned as usize,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
    }
}

/// Per-instance state for a PagableArc. Tracks whether this particular Arc instance
/// is contributing to the pinned_count.
#[derive(Eq, PartialEq)]
enum PagableArcState {
    /// This instance is not keeping the data pinned (does not contribute to pinned_count).
    Unpinned,
    /// This instance is keeping the data pinned (contributes to pinned_count).
    Pinned,
}

impl From<PagableArcState> for PagableArcStateHolder {
    fn from(state: PagableArcState) -> Self {
        Self(AtomicUsize::new(state as usize))
    }
}

impl<T: Pagable + std::fmt::Debug> std::fmt::Debug for PagableArc<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PagableArc")
            .field("state", &self.state.0)
            .field("pointer", &self.pointer)
            .finish()
    }
}

impl<T: Pagable> Clone for PagableArc<T> {
    fn clone(&self) -> Self {
        match self.state.get() {
            PagableArcState::Pinned => {
                // This should succeed, but a race with page_out() could make it fail. In that case we'll just get an Unpinned clone.
                if self.pointer.try_alloc_pinned() {
                    Self {
                        state: PagableArcState::Pinned.into(),
                        pointer: self.pointer.clone(),
                    }
                } else {
                    Self {
                        state: PagableArcState::Unpinned.into(),
                        pointer: self.pointer.clone(),
                    }
                }
            }
            PagableArcState::Unpinned => Self {
                state: PagableArcState::Unpinned.into(),
                pointer: self.pointer.clone(),
            },
        }
    }
}

impl<T: Pagable> Dupe for PagableArc<T> {}

/// A borrowed reference to pinned data, avoiding reference count manipulation.
///
/// This type is similar to `&PinnedPagableArc<T>` but is more efficient for
/// short-lived access patterns as it doesn't increment or decrement the
/// reference count.
///
/// Obtain a `PinnedPagableArcBorrow` via [`PinnedPagableArc::borrow()`].
///
/// # Example
///
/// ```ignore
/// fn process_borrowed(borrowed: PinnedPagableArcBorrow<'_, MyData>) {
///     // Access data via Deref
///     println!("{:?}", &*borrowed);
///
///     // Or clone to get an owned PinnedPagableArc
///     let owned: PinnedPagableArc<MyData> = borrowed.clone_arc();
/// }
/// ```
#[allow(private_interfaces)]
#[derive(Debug, Clone_, Copy_)]
pub struct PinnedPagableArcBorrow<'a, T> {
    inner: triomphe::ArcBorrow<'a, PagableArcInner<T>>,
}

impl<'a, T> Dupe for PinnedPagableArcBorrow<'a, T> {}

impl<'a, T: Pagable> PinnedPagableArcBorrow<'a, T> {
    /// Clones this borrowed reference into an owned `PinnedPagableArc`.
    ///
    /// This increments the reference count and returns a new pinned reference.
    pub fn clone_arc(self) -> PinnedPagableArc<T> {
        self.inner.add_pinned_unchecked();
        PinnedPagableArc::new_from_ptr(self.inner.clone_arc())
    }

    /// Gets a reference to the underlying data.
    pub fn get(&self) -> &T {
        self.inner.pinned_access()
    }
}

impl<T: Pagable> std::ops::Deref for PinnedPagableArcBorrow<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.inner.pinned_access()
    }
}

impl<T: Pagable> PagableArc<T> {
    /// Creates a new `PagableArc` in the pinned state.
    ///
    /// The data is guaranteed to be in memory until this reference (or any derived
    /// pinned references) are dropped or explicitly unpinned via [`unpin()`](Self::unpin).
    ///
    /// # Arguments
    ///
    /// * `v` - The value to store
    /// * `storage` - The storage backend for paging operations
    pub fn new(v: T, storage: PagableStorageHandle) -> Self {
        let pointer = triomphe::Arc::new(PagableArcInner::new(v, storage));
        Self {
            state: PagableArcStateHolder(AtomicUsize::new(1)),
            pointer,
        }
    }

    pub(crate) fn new_paged_out(key: &DataKey, storage: PagableStorageHandle) -> Self {
        let pointer = triomphe::Arc::new(PagableArcInner::new_paged_out(key, storage));
        Self {
            state: PagableArcStateHolder(AtomicUsize::new(0)),
            pointer,
        }
    }

    /// Tests whether two `PagableArc`s point to the same allocation.
    ///
    /// Returns `true` if both arcs point to the same underlying data, regardless
    /// of their individual pinned/unpinned state.
    pub fn ptr_eq(left: &Self, right: &Self) -> bool {
        triomphe::Arc::ptr_eq(&left.pointer, &right.pointer)
    }

    /// Pins the data, blocking if deserialization is needed.
    ///
    /// This is a synchronous operation that will block the current thread if
    /// deserialization is required. Prefer [`pin()`](Self::pin) when possible.
    ///
    /// On success, this `PagableArc` transitions to the pinned state.
    ///
    /// # Errors
    ///
    /// Returns an error if deserialization from storage fails.
    #[cfg(any(feature = "tokio", test))]
    pub fn pin_sync(&self) -> crate::Result<PinnedPagableArc<T>>
    where
        T: Pagable,
    {
        self.pointer.alloc_pinned_blocking()?;
        if self.state.set_pinned() {
            self.pointer.add_pinned_unchecked();
        }
        Ok(PinnedPagableArc::new_from_ptr(self.pointer.clone()))
    }

    /// Pins the data, asynchronously paging in if needed.
    ///
    /// This is the preferred way to access data from a `PagableArc`. If the data
    /// has been paged out, it will be asynchronously deserialized from storage.
    ///
    /// On success, this `PagableArc` transitions to the pinned state.
    ///
    /// # Errors
    ///
    /// Returns an error if deserialization from storage fails.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let pinned = pagable.pin().await?;
    /// println!("Data: {:?}", *pinned);
    /// ```
    pub async fn pin(&self) -> anyhow::Result<PinnedPagableArc<T>>
    where
        T: Pagable,
    {
        self.pointer.alloc_pinned_async().await?;
        if self.state.set_pinned() {
            self.pointer.add_pinned_unchecked();
        }
        Ok(PinnedPagableArc::new_from_ptr(self.pointer.clone()))
    }

    /// Transitions this reference to the unpinned state.
    ///
    /// After calling this method, this `PagableArc` no longer contributes to keeping
    /// the data in memory. If this was the last pinned reference, the data becomes
    /// eligible for eviction.
    ///
    /// This is a no-op if the reference is already unpinned.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let pagable = pinned.into_pagable();  // Starts pinned
    /// pagable.unpin();  // Now unpinned, data may be evicted
    /// ```
    pub fn unpin(&self) {
        if self.state.set_unpinned() {
            PagableArcInner::release_pin(&self.pointer);
        }
    }

    pub(crate) fn is_paged_out(&self) -> bool {
        self.pointer.is_paged_out()
    }

    pub(crate) fn get_data_key(&self) -> Option<DataKey> {
        self.pointer.get_data_key()
    }

    pub(crate) fn set_data_key(&self, key: DataKey) {
        self.pointer.set_data_key(key);
    }

    /// Returns the number of pinned references to the underlying data.
    /// This is the count of `PagableArc` instances in the `Pinned` state,
    /// plus any `PinnedPagableArc` instances. Used for testing and debugging.
    pub(crate) fn pinned_count(&self) -> usize {
        self.pointer.pinned_count()
    }

    /// Returns true if this `PagableArc` is in the pinned state.
    /// Asserts that the underlying pointer's state is consistent with the local state.
    pub(crate) fn is_pinned(&self) -> bool {
        if self.state.get() == PagableArcState::Pinned {
            assert!(self.pointer.is_pinned());
            true
        } else {
            false
        }
    }
}

impl<T: Pagable> Drop for PagableArc<T> {
    fn drop(&mut self) {
        match self.state.get() {
            PagableArcState::Pinned => PagableArcInner::release_pin(&self.pointer),
            PagableArcState::Unpinned => {}
        }
    }
}

/// A reference-counted smart pointer that guarantees data is in memory.
///
/// `PinnedPagableArc<T>` is similar to `Arc<T>` - it keeps data pinned in memory
/// and provides direct access via `Deref`. Unlike [`PagableArc`], a `PinnedPagableArc`
/// always guarantees that the data is accessible.
///
/// # Creating a PinnedPagableArc
///
/// ```ignore
/// // Create directly
/// let pinned = PinnedPagableArc::new(MyData::new(), storage);
///
/// // Or pin from a PagableArc
/// let pinned = pagable.pin().await?;
/// ```
///
/// # Accessing Data
///
/// `PinnedPagableArc<T>` implements `Deref<Target = T>`, so you can access
/// the data directly:
///
/// ```ignore
/// let pinned = PinnedPagableArc::new(vec![1, 2, 3], storage);
/// println!("Length: {}", pinned.len());  // Deref to Vec<i32>
/// ```
///
/// # Converting to PagableArc
///
/// Use [`into_pagable()`](Self::into_pagable) to convert to a [`PagableArc`] that
/// can be paged out:
///
/// ```ignore
/// let pagable = pinned.into_pagable();
/// pagable.unpin();  // Data may now be evicted
/// ```
///
/// # Before paging a field of this type
///
/// Nothing in buck2 deserializes a `PinnedPagableArc` yet, and the
/// deserialized-arc cache is not ready for one: `PinnedPagableArcWeak` cannot
/// upgrade a paged-out allocation, so a page-in binds a fresh allocation while
/// the handles still referring to the first keep their own copy. The fix is to
/// pin the existing allocation with the freshly deserialized value instead.
pub struct PinnedPagableArc<T: Pagable> {
    pointer: triomphe::Arc<PagableArcInner<T>>,
}

impl<T: Pagable + std::fmt::Debug> std::fmt::Debug for PinnedPagableArc<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Arc")
            .field("pointer", &self.pointer)
            .finish()
    }
}

unsafe impl<T: Pagable> Send for PinnedPagableArc<T> {}
unsafe impl<T: Pagable> Sync for PinnedPagableArc<T> {}

impl<T: Pagable> Allocative for PinnedPagableArc<T> {
    fn visit<'a, 'b: 'a>(&self, _visitor: &'a mut Visitor<'b>) {}
}

impl<T: Pagable + Eq + PartialEq> Eq for PinnedPagableArc<T> {}
impl<T: Pagable + PartialEq> PartialEq for PinnedPagableArc<T> {
    fn eq(&self, other: &Self) -> bool {
        triomphe::Arc::ptr_eq(&self.pointer, &other.pointer)
            || self.pointer.pinned_access() == other.pointer.pinned_access()
    }
}

impl<T: Pagable> PinnedPagableArc<T> {
    /// Creates a new `PinnedPagableArc` with the given value.
    ///
    /// The data is immediately allocated and pinned in memory.
    ///
    /// # Arguments
    ///
    /// * `v` - The value to store
    /// * `storage` - The storage backend for paging operations
    pub fn new(v: T, storage: PagableStorageHandle) -> Self {
        let pointer = triomphe::Arc::new(PagableArcInner::new(v, storage));
        Self { pointer }
    }

    /// Creates a borrowed reference to the pinned data.
    ///
    /// This avoids incrementing/decrementing the reference count, making it
    /// more efficient for short-lived access patterns.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let pinned = PinnedPagableArc::new(data, storage);
    /// let borrowed = PinnedPagableArc::borrow(&pinned);
    /// process(&*borrowed);  // No refcount manipulation
    /// ```
    pub fn borrow(v: &PinnedPagableArc<T>) -> PinnedPagableArcBorrow<'_, T> {
        PinnedPagableArcBorrow {
            inner: triomphe::Arc::borrow_arc(&v.pointer),
        }
    }

    fn new_from_ptr(pointer: triomphe::Arc<PagableArcInner<T>>) -> Self {
        Self { pointer }
    }

    /// Tests whether two `PinnedPagableArc`s point to the same allocation.
    ///
    /// Returns `true` if both arcs point to the same underlying data.
    pub fn ptr_eq(left: &Self, right: &Self) -> bool {
        triomphe::Arc::ptr_eq(&left.pointer, &right.pointer)
    }

    /// Converts this pinned reference into a `PagableArc`.
    ///
    /// The returned `PagableArc` starts in the pinned state (keeping the data in memory).
    /// Call [`unpin()`](PagableArc::unpin) on it to allow the data to be evicted.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let pinned = PinnedPagableArc::new(data, storage);
    /// // ... use data ...
    ///
    /// // Done with immediate access, convert to allow paging
    /// let pagable = pinned.into_pagable();
    /// pagable.unpin();
    /// ```
    pub fn into_pagable(self) -> PagableArc<T>
    where
        T: Pagable,
    {
        self.pointer.add_pinned_unchecked();
        PagableArc {
            state: PagableArcState::Pinned.into(),
            pointer: self.pointer.clone(),
        }
    }
}

impl<T: Pagable> Drop for PinnedPagableArc<T> {
    fn drop(&mut self) {
        PagableArcInner::release_pin(&self.pointer);
    }
}

impl<T: Pagable> Clone for PinnedPagableArc<T> {
    fn clone(&self) -> Self {
        self.pointer.add_pinned_unchecked();
        Self::new_from_ptr(self.pointer.clone())
    }
}

impl<T: Pagable> Dupe for PinnedPagableArc<T> {}

impl<T: Pagable> Deref for PinnedPagableArc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.pointer.pinned_access()
    }
}

/// The shared backing data for all PagableArc/PinnedPagableArc instances pointing
/// to the same data. Contains the actual value (or PagedOut state) and the pinned count.
struct PagableArcInner<T> {
    /// Count of references that are keeping the data pinned in memory.
    pinned_count: AtomicUsize,
    /// Protects mutations to `value`. The fast path (pinned_count > 0) doesn't need
    /// the lock, but transitions when count reaches 0 require it.
    lock: Mutex<()>,
    data: UnsafeCell<PagableArcInnerData<T>>,
    storage: PagableStorageHandle,
}

unsafe impl<T> Sync for PagableArcInner<T> {}

impl<T: std::fmt::Debug> std::fmt::Debug for PagableArcInner<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _lock = self.lock.lock();
        f.debug_struct("PagableArcInner")
            .field("pinned", &self.pinned_count)
            .field("value", unsafe { &*self.data.get() })
            .finish()
    }
}

#[derive(Debug)]
struct PagableArcInnerData<T> {
    key: Option<DataKey>,
    value: PagableArcInnerState<T>,
}

impl<T> PagableArcInnerData<T> {
    fn new_paged_out(key: &DataKey) -> Self {
        Self {
            key: (*key).into(),
            value: PagableArcInnerState::PagedOut,
        }
    }

    fn new_pinned(value: std::sync::Arc<T>) -> Self {
        Self {
            key: None,
            value: PagableArcInnerState::Pinned(value),
        }
    }

    fn unpin(&mut self) -> bool {
        if self.key.is_some() {
            self.value = PagableArcInnerState::PagedOut;
            false
        } else {
            take_mut::take(&mut self.value, |v| {
                PagableArcInnerState::Unpinned(v.unwrap_into_ready())
            });
            true
        }
    }

    fn try_pin(&mut self) -> bool {
        take_mut::take(&mut self.value, |v| match v {
            PagableArcInnerState::Pinned(v) | PagableArcInnerState::Unpinned(v) => {
                PagableArcInnerState::Pinned(v)
            }
            PagableArcInnerState::PagedOut => PagableArcInnerState::PagedOut,
        });
        matches!(self.value, PagableArcInnerState::Pinned(_))
    }

    fn pin(&mut self, data: std::sync::Arc<T>) {
        take_mut::take(&mut self.value, |v| {
            PagableArcInnerState::Pinned(match v {
                PagableArcInnerState::Pinned(v) | PagableArcInnerState::Unpinned(v) => v,
                PagableArcInnerState::PagedOut => data,
            })
        })
    }
}

#[derive(Debug)]
enum PagableArcInnerState<T> {
    Pinned(std::sync::Arc<T>),
    Unpinned(std::sync::Arc<T>),
    PagedOut,
}

impl<T> PagableArcInnerState<T> {
    fn unwrap_ready(&self) -> &T {
        match self {
            PagableArcInnerState::Pinned(t) => t,
            PagableArcInnerState::Unpinned(_) => panic!("Unpinned state is not ready"),
            PagableArcInnerState::PagedOut => panic!("PagedOut state is not ready"),
        }
    }

    fn unwrap_into_ready(self) -> std::sync::Arc<T> {
        match self {
            PagableArcInnerState::Pinned(t) => t,
            PagableArcInnerState::Unpinned(_) => panic!("Unpinned state is not ready"),
            PagableArcInnerState::PagedOut => panic!("PagedOut state is not ready"),
        }
    }
}

// On 64-bit platforms, the struct packs into 8 usizes. On 32-bit wasm32,
// fixed-size fields like DataKey stay 16 bytes while pointers shrink to 4,
// so the struct occupies 10 usizes instead. Other 32-bit targets like armv7 have different
// Mutex layout and pack tighter, so we do not assert a fixed size there.
#[cfg(target_pointer_width = "64")]
static_assertions::assert_eq_size!(PagableArcInner<[usize; 4]>, [usize; 8]);
#[cfg(target_arch = "wasm32")]
static_assertions::assert_eq_size!(PagableArcInner<[usize; 4]>, [usize; 10]);

impl<T: Pagable> PagableArcInner<T> {
    pub fn new_paged_out(key: &DataKey, storage: PagableStorageHandle) -> Self {
        Self {
            pinned_count: AtomicUsize::new(0),
            lock: Mutex::new(()),
            data: UnsafeCell::new(PagableArcInnerData::new_paged_out(key)),
            storage,
        }
    }

    fn new(value: T, storage: PagableStorageHandle) -> Self {
        Self {
            pinned_count: AtomicUsize::new(1),
            lock: Mutex::new(()),
            data: UnsafeCell::new(PagableArcInnerData::new_pinned(std::sync::Arc::new(value))),
            storage,
        }
    }

    /// Gets a reference to the underlying value. Should only be called by something that keeps the data pinned and the reference is only valid while the data is pinned.
    fn pinned_access(&self) -> &T {
        unsafe { &*self.data.get() }.value.unwrap_ready()
    }

    /// Adds a pinned reference assuming that the data is already pinned (pinned_count >= 1).
    ///
    /// # Safety requirement (not unsafe, but must be upheld for correctness)
    /// The caller must ensure that pinned_count >= 1 before calling. This is guaranteed if:
    /// - The caller holds a PinnedPagableArc (which keeps pinned_count >= 1)
    /// - The caller just successfully called try_alloc_pinned() or alloc_pinned_async()
    ///
    /// # Memory ordering
    /// Uses Relaxed ordering because synchronization is provided by either:
    /// - The lock that was just released by alloc_pinned, or
    /// - The existing pinned reference the caller holds
    fn add_pinned_unchecked(&self) {
        let s = self.pinned_count.fetch_add(1, Ordering::Relaxed);
        assert!(s >= 1);
    }

    /// Async version of alloc_pinned. Adds one to pinned_count on success.
    ///
    /// # Note on concurrent calls
    /// If multiple threads call this when data is PagedOut, each will deserialize
    /// independently (duplicate work) but correctness is maintained - each caller
    /// gets their pin counted. The lock only protects the final write, not the
    /// deserialization itself. Only one of the deserialized values will actually
    /// be used (the others are discarded).
    #[allow(clippy::manual_async_fn)]
    fn alloc_pinned_async<'a>(&'a self) -> impl Future<Output = anyhow::Result<()>> + Send + 'a
    where
        T: Pagable,
    {
        async move {
            if self.try_alloc_pinned() {
                return Ok(());
            }

            let lock = self.lock.lock();
            let data = unsafe { &mut *self.data.get() };
            if !data.try_pin() {
                let key = data.key.unwrap();
                drop((data, lock));

                let value: std::sync::Arc<T> =
                    std::sync::Arc::new(self.storage.deserialize_pagable_data(&key).await?);

                let _lock = self.lock.lock();
                let data = unsafe { &mut *self.data.get() };
                data.pin(value);
            }

            self.pinned_count.fetch_add(1, Ordering::Release);

            Ok(())
        }
    }

    /// Ensures data is pinned, blocking if deserialization is needed.
    /// Adds one to pinned_count on success.
    #[cfg(any(feature = "tokio", test))]
    fn alloc_pinned_blocking(&self) -> anyhow::Result<()>
    where
        T: Pagable,
    {
        if self.try_alloc_pinned() {
            return Ok(());
        }
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { self.alloc_pinned_async().await })
        })
    }

    /// Tries to allocate a pin without paging in data. Returns true if successful.
    fn try_alloc_pinned(&self) -> bool {
        if self
            .pinned_count
            .try_update(Ordering::Acquire, Ordering::Relaxed, |v| {
                if v == 0 { None } else { Some(v + 1) }
            })
            .is_ok()
        {
            return true;
        }

        {
            let _lock = self.lock.lock();
            let data = unsafe { &mut *self.data.get() };
            if data.try_pin() {
                self.pinned_count.fetch_add(1, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Releases a pin. If this was the last pin, may transition value to Unpinned or PagedOut.
    ///
    /// # Algorithm
    /// 1. Atomically decrement pinned_count
    /// 2. If old value was 1 (we just went to 0):
    ///    a. Acquire lock
    ///    b. **Double-check** that pinned_count is still 0 (another thread may have
    ///       incremented it between our decrement and lock acquisition)
    ///    c. If still 0: transition Pinned → Unpinned (or PagedOut if has_paged_out)
    ///
    /// # Why the double-check
    /// Consider this race:
    /// - Thread A: fetch_sub returns 1 (count now 0)
    /// - Thread B: try_alloc_pinned slow path, acquires lock, increments count to 1
    /// - Thread A: acquires lock, but count is now 1, so we must NOT transition
    fn release_pin(ptr: &triomphe::Arc<Self>) {
        if ptr.pinned_count.fetch_sub(1, Ordering::Release) == 1 {
            std::sync::atomic::fence(Ordering::Acquire);
            let _lock = ptr.lock.lock();
            if ptr.pinned_count.load(Ordering::Relaxed) == 0 {
                let data: &mut _ = unsafe { &mut *ptr.data.get() };
                if data.unpin() {
                    let d = PagableArc {
                        state: PagableArcState::Unpinned.into(),
                        pointer: ptr.clone(),
                    };
                    ptr.storage.schedule_for_paging(d);
                }
            }
        }
    }

    pub(crate) fn is_paged_out(&self) -> bool {
        let _lock = self.lock.lock();
        match unsafe { &*self.data.get() }.value {
            PagableArcInnerState::PagedOut => true,
            PagableArcInnerState::Pinned(_) | PagableArcInnerState::Unpinned(_) => false,
        }
    }

    pub(crate) fn get_data_key(&self) -> Option<DataKey> {
        let _lock = self.lock.lock();
        unsafe { &*self.data.get() }.key
    }

    pub(crate) fn set_data_key(&self, key: DataKey) {
        let _lock = self.lock.lock();
        let mut data = unsafe { &mut *self.data.get() };
        data.key = key.into();
        take_mut::take(&mut data.value, |v| match v {
            PagableArcInnerState::Unpinned(_) | PagableArcInnerState::PagedOut => {
                PagableArcInnerState::PagedOut
            }
            v => v,
        });
    }

    /// Returns the current pinned reference count. This count represents the number
    /// of `PagableArc` instances in `Pinned` state plus any `PinnedPagableArc` instances.
    pub(crate) fn pinned_count(&self) -> usize {
        self.pinned_count.load(Ordering::Relaxed)
    }

    /// Returns true if the pinned count is greater than zero.
    /// Asserts that when pinned_count > 0, the inner state is actually `Pinned`,
    /// ensuring consistency between the atomic counter and the state machine.
    pub(crate) fn is_pinned(&self) -> bool {
        if self.pinned_count.load(Ordering::Relaxed) > 0 {
            let _lock = self.lock.lock();
            if self.pinned_count.load(Ordering::Relaxed) > 0 {
                assert!(matches!(
                    unsafe { &*self.data.get() }.value,
                    PagableArcInnerState::Pinned(_)
                ));
                return true;
            }
        }
        false
    }
}

impl<T: Pagable> PagableSerialize for PagableArc<T> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> anyhow::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl<'de, T: Pagable> PagableDeserialize<'de> for PagableArc<T> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        deserialize_arc::<Self, _>(deserializer)
    }
}

impl<T: Pagable> ArcErase for PagableArc<T> {
    type Weak = StrongAsWeak<Self>;

    fn dupe_strong(&self) -> Self {
        self.dupe()
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        self.pointer.as_ptr() as usize
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        // An unpinned handle preserves identity without holding the value in
        // memory, which is all a weak reference would do.
        Some(StrongAsWeak::new(Self {
            state: PagableArcState::Unpinned.into(),
            pointer: self.pointer.clone(),
        }))
    }

    fn is_unique(&self) -> bool {
        self.pointer.is_unique()
    }

    fn set_data_key(&self, k: DataKey) {
        self.set_data_key(k);
    }

    fn data_key(&self) -> Option<DataKey> {
        self.get_data_key()
    }

    fn needs_paging_out(&self) -> bool {
        self.get_data_key().is_none()
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn PagableSerializer,
    ) -> anyhow::Result<ArcSerializeOutcome> {
        #[cfg(any(feature = "tokio", test))]
        {
            let strong = self.pin_sync()?;
            <T as PagableSerialize>::pagable_serialize(&strong, ser)?;
            Ok(ArcSerializeOutcome::Serialized)
        }
        #[cfg(not(any(feature = "tokio", test)))]
        {
            Err(anyhow::anyhow!(
                "Cannot serialize PagableArc without tokio feature"
            ))
        }
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> anyhow::Result<Self> {
        Ok(Self::new(
            <T as PagableDeserialize>::pagable_deserialize(deser)?,
            deser.storage().dupe(),
        ))
    }
}

impl<T: Pagable> PagableSerialize for PinnedPagableArc<T> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> anyhow::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl<'de, T: Pagable> PagableDeserialize<'de> for PinnedPagableArc<T> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        deserialize_arc::<Self, _>(deserializer)
    }
}

/// Weak form of a [`PinnedPagableArc`]: an unpinned handle, so a cache holding
/// it leaves the value free to page out.
pub struct PinnedPagableArcWeak<T: Pagable>(PagableArc<T>);

impl<T: Pagable> WeakErase for PinnedPagableArcWeak<T> {
    fn is_expired(&self) -> bool {
        self.0.pointer.is_unique()
    }

    /// Pins the value if it is resident. A paged-out value is not read back;
    /// the caller deserializes afresh instead.
    fn upgrade_weak(&self) -> Option<Box<dyn ArcEraseDyn>> {
        if !self.0.pointer.try_alloc_pinned() {
            return None;
        }
        Some(Box::new(PinnedPagableArc::new_from_ptr(
            self.0.pointer.clone(),
        )))
    }
}

impl<T: Pagable> ArcErase for PinnedPagableArc<T> {
    type Weak = PinnedPagableArcWeak<T>;

    fn dupe_strong(&self) -> Self {
        self.dupe()
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        self.pointer.as_ptr() as usize
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        Some(PinnedPagableArcWeak(PagableArc {
            state: PagableArcState::Unpinned.into(),
            pointer: self.pointer.clone(),
        }))
    }

    fn is_unique(&self) -> bool {
        self.pointer.is_unique()
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn PagableSerializer,
    ) -> anyhow::Result<ArcSerializeOutcome> {
        <T as PagableSerialize>::pagable_serialize(self, ser)?;
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> anyhow::Result<Self> {
        Ok(Self::new(
            <T as PagableDeserialize>::pagable_deserialize(deser)?,
            deser.storage().dupe(),
        ))
    }
}
