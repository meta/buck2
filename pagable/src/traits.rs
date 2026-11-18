/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Core traits for pagable serialization and deserialization.
//!
//! This module contains all the public traits that define the pagable framework:
//!
//! - [`Pagable`] - convenience trait combining serialization and deserialization
//! - [`PagableSerialize`] / [`PagableDeserialize`] - traits for types that can be serialized/deserialized
//! - [`PagableDeserializeOwned`] - trait for types that can be deserialized from any lifetime
//! - [`PagableSerializer`] / [`PagableDeserializer`] - traits for serializer/deserializer implementations

use std::any::Any;
use std::any::TypeId;
use std::sync::Arc;
use std::sync::OnceLock;

use allocative::Allocative;

use crate::PagableDeserializerRecipe;
use crate::arc_erase::ArcEraseDyn;
use crate::hashers::TypeIdDashMap;
use crate::storage::data::DataKey;
use crate::storage::handle::PagableStorageHandle;

// ============================================================================
// StorageContext — typed state shared by one storage backend
// ============================================================================

/// State that may be owned by a [`StorageContext`].
pub trait StorageState: Send + Sync + 'static {}

/// Notified when an arc is associated with a storage key.
///
/// Pagable knows only the content-addressed [`DataKey`]. A layer above it that
/// has its own notion of identity - one that survives page-out and is what
/// page-in resolves against - can use this to index arcs by that identity as
/// they are written and read.
pub type ArcDataKeyObserver = fn(&StorageContext, &dyn ArcEraseDyn, DataKey);

/// Storage-lifetime state shared by serializers and deserializers using the
/// same storage backend.
///
/// Thread-safe: backed by `DashMap` so multiple serializations can run
/// concurrently without external locking.
#[derive(Default)]
pub struct StorageContext {
    states: TypeIdDashMap<Arc<dyn Any + Send + Sync>>,
    arc_data_key_observer: OnceLock<ArcDataKeyObserver>,
}

impl StorageContext {
    /// Register the observer notified whenever an arc is associated with a
    /// storage key. Later calls are ignored, so callers on every page-out and
    /// page-in path can register unconditionally.
    pub fn observe_arc_data_keys(&self, observer: ArcDataKeyObserver) {
        let _ignored = self.arc_data_key_observer.set(observer);
    }

    pub(crate) fn notify_arc_data_key(&self, arc: &dyn ArcEraseDyn, key: DataKey) {
        if let Some(observer) = self.arc_data_key_observer.get() {
            observer(self, arc, key);
        }
    }

    /// Create a new empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the state of type `T`, if initialized.
    pub fn get<T: StorageState>(&self) -> Option<Arc<T>> {
        self.states
            .get(&TypeId::of::<T>())
            .map(|value| Arc::clone(value.value()))
            .map(|value| {
                value
                    .downcast::<T>()
                    .expect("storage state must match its type")
            })
    }

    /// Get the state of type `T`, initializing it atomically if absent.
    ///
    /// Reads before it writes: `entry` takes the shard exclusively even when the
    /// entry is present, and the key is a fixed `TypeId`, so on a path taken once
    /// per deserialized value every caller lands on one contended lock.
    pub fn get_or_init<T: StorageState>(&self, init: impl FnOnce() -> T) -> Arc<T> {
        if let Some(value) = self.get::<T>() {
            return value;
        }
        let value = self
            .states
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Arc::new(init()))
            .clone();
        value
            .downcast::<T>()
            .expect("storage state must match its type")
    }
}

// ============================================================================
// Cursor — captures both byte position and arc index
// ============================================================================

/// A snapshot of the serializer/deserializer position, capturing both the byte
/// stream position and the arc list index.
///
/// This enables correct save/restore of position across seek operations.
#[derive(Allocative, Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PagableCursor {
    /// Position in the byte stream.
    pub byte_pos: usize,
    /// Index into the arc list.
    pub arc_index: usize,
}

// ============================================================================
// Combined Pagable trait
// ============================================================================

/// A convenience trait for types that are pagable serializable/deserializable.
///
/// This trait is automatically implemented for any type that implements:
/// - `Send + Sync` - for thread safety
/// - `PagableSerialize` - for serialization
/// - `PagableDeserialize<'a>` for all lifetimes `'a` - for deserialization
/// - `'static` - no borrowed data
///
/// Use `#[derive(Pagable)]` to derive both PagableSerialize and PagableDeserialize.
pub trait Pagable:
    Send + Sync + PagableSerialize + for<'a> PagableDeserialize<'a> + 'static
{
}

impl<T: Send + Sync + PagableSerialize + for<'a> PagableDeserialize<'a> + 'static> Pagable for T {}
// ============================================================================
// Serialize/Deserialize traits for types
// ============================================================================

/// Trait for types that can be serialized using the pagable framework.
///
/// Implement this trait to define how a type is serialized. The serializer
/// provides access to both serde serialization and support for arc identity preservation.
///
/// Use `#[derive(PagableSerialize)]` for automatic implementation.
pub trait PagableSerialize {
    /// Serialize this value using the provided serializer.
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()>;

    /// Write a slice of `Self` as a `Vec<Self>`, length prefix included.
    ///
    /// The counterpart to [`PagableDeserialize::pagable_deserialize_vec`], and
    /// subject to the same rule: an override must produce the bytes the default
    /// would have.
    fn pagable_serialize_vec(
        values: &[Self],
        serializer: &mut dyn PagableSerializer,
    ) -> crate::Result<()>
    where
        Self: Sized,
    {
        <usize as serde::Serialize>::serialize(&values.len(), serializer.serde())?;
        for value in values {
            value.pagable_serialize(serializer)?;
        }
        Ok(())
    }

    /// Serialize this value as the payload of an Arc node.
    ///
    /// Page-out calls this hook after [`PagableSerializer::serialize_arc`] records
    /// an Arc edge. The default uses the same representation as the value itself.
    /// For example, when one allocation is held as both `Arc<Concrete>` and
    /// `Arc<dyn Trait>`, the generated typetag override stores the dyn view as a
    /// type tag plus a reference to the canonical `Arc<Concrete>`. This stores the
    /// concrete body once and records the relationship needed for page-in to
    /// reconstruct both views using the same Arc allocation.
    fn pagable_serialize_arc_payload(
        self: Arc<Self>,
        serializer: &mut dyn PagableSerializer,
    ) -> crate::Result<()> {
        self.pagable_serialize(serializer)
    }
}

/// Trait for types that should be serialized eagerly.
///
/// This is used for types where serialization should happen immediately
/// rather than being deferred.
pub trait PagableEagerSerialize {
    /// Eagerly serialize this value using the provided serializer.
    fn eager_pagable_serialize<S: PagableSerializer>(
        &self,
        serializer: &mut S,
    ) -> crate::Result<()>;
}

/// Trait for types that can be deserialized using the pagable framework.
///
/// Implement this trait to define how a type is deserialized. The deserializer
/// provides access to both serde deserialization and arc identity preservation.
///
/// Use `#[derive(PagableDeserialize)]` for automatic implementation.
pub trait PagableDeserialize<'de>: Sized {
    /// Deserialize a value using the provided deserializer.
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self>;

    /// Read a `Vec<Self>`, length prefix included.
    ///
    /// A specialization hook, since Rust has none: `u8` overrides it to take
    /// the run as one slice rather than a dispatch per byte. An override must
    /// read exactly what the default writes.
    fn pagable_deserialize_vec<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Vec<Self>> {
        let len = <usize as serde::Deserialize>::deserialize(deserializer.serde())?;
        let mut values = Vec::with_capacity(len);
        for _ in 0..len {
            values.push(Self::pagable_deserialize(deserializer)?);
        }
        Ok(values)
    }
}

/// Trait for types that can be deserialized from any lifetime.
///
/// This is analogous to serde's `DeserializeOwned` trait. It is automatically
/// implemented for any type that implements `PagableDeserialize<'de>` for all
/// lifetimes `'de`.
pub trait PagableDeserializeOwned: for<'de> PagableDeserialize<'de> {}
impl<T> PagableDeserializeOwned for T where T: for<'de> PagableDeserialize<'de> {}

/// Trait for types that can be deserialized into a [`Box<Self>`].
///
/// This trait returns `Box<Self>` instead of `Self` which can be used for unsized types including
/// trait objects (`dyn Trait`) which can't be returned by value.
///
/// This enables deserialization of `Arc<T>` where `T: ?Sized`.
pub trait PagableBoxDeserialize<'de> {
    /// Deserialize a value into a boxed instance.
    ///
    /// This method is called when deserializing unsized types that must be heap-allocated.
    fn deserialize_box<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Box<Self>>;

    /// Deserialize an Arc from its stored payload.
    ///
    /// The default deserializes the value into a box and moves it into a new Arc.
    /// Implementations can override this when an Arc view has a different payload.
    /// For example, the generated `Arc<dyn Trait>` typetag override reads the type
    /// tag, deserializes the referenced canonical `Arc<Concrete>`, and coerces that
    /// Arc into the dyn view. This lets both views use the same page-in allocation.
    fn deserialize_arc_payload<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Arc<Self>> {
        Ok(Arc::from(Self::deserialize_box(deserializer)?))
    }
}

/// Trait for types that should be deserialized eagerly.
///
/// This is used for types where deserialization should happen immediately
/// rather than being deferred.
pub trait PagableEagerDeserialize<'de>: Sized {
    /// Eagerly deserialize a value using the provided deserializer.
    fn eager_pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self>;
}

// ============================================================================
// Serializer/Deserializer context traits
// ============================================================================

/// Trait for serializers that support pagable serialization.
///
/// Implementors provide access to an underlying serde serializer and the ability
/// to preserve arc instance equality across serialization.
pub trait PagableSerializer {
    /// Get a mutable reference to the underlying postcard serializer.
    fn serde(&mut self) -> &mut postcard::Serializer<crate::flavors::PagableVecFlavor>;

    /// Serialize an Arc, preserving its identity for deduplication.
    ///
    /// Implementations should track Arc identity so that the same Arc serialized
    /// multiple times results in shared references after deserialization.
    fn serialize_arc(&mut self, arc: &dyn ArcEraseDyn) -> crate::Result<()>;

    /// Current cursor position (byte position + arc index).
    fn position(&mut self) -> PagableCursor;

    /// Overwrite bytes at a previously written position.
    ///
    /// # Safety
    /// Caller must ensure `pos + bytes.len()` does not exceed the current byte
    /// position, and that the overwritten region is semantically valid for the
    /// format.
    unsafe fn write_at(&mut self, pos: usize, bytes: &[u8]) {
        self.serde().output.write_at(pos, bytes);
    }

    /// Access state owned by the storage backend.
    fn storage_context(&self) -> &StorageContext;
}

static_assertions::assert_obj_safe!(PagableSerializer);

/// Trait for deserializers that support pagable deserialization.
///
/// Object-safe; `serde()` exposes the concrete `postcard` deserializer.
pub trait PagableDeserializer<'de> {
    /// Get the underlying serde deserializer.
    ///
    /// Concrete rather than type-erased, mirroring [`PagableSerializer::serde`]:
    /// erasing it cost a boxed trait object per read, and what page-in reads is
    /// overwhelmingly scalars.
    fn serde(&mut self) -> &mut postcard::Deserializer<'de, crate::flavors::PagableSlice<'de>>;

    /// Current cursor position (byte position + arc index).
    fn position(&self) -> PagableCursor;

    /// Seek to a previously saved cursor position.
    ///
    /// # Safety
    /// Caller must ensure `cursor` was obtained from a prior `position()` call
    /// on the same deserializer, and that the cursor represents a valid state
    /// (i.e., a byte boundary at the start of a serialized value with the
    /// correct arc index).
    unsafe fn seek(&mut self, cursor: PagableCursor);

    /// Deserialize an Arc, restoring shared references for deduplicated Arcs.
    ///
    /// If the same Arc was serialized multiple times via `serialize_arc`, this method
    /// should return clones that point to the same allocation (preserving identity).
    ///
    /// On a backend with stable arc keys this splits in two: [`take_arc_key`]
    /// reads the slot's key and [`PagableStorageHandle::deserialize_arc_by_key`]
    /// restores the arc later. A successful key read has consumed the slot,
    /// so restore that arc by key rather than through this method, which would
    /// consume the next slot. A `None` key read consumed nothing; fall back to
    /// this method.
    ///
    /// [`take_arc_key`]: Self::take_arc_key
    /// [`PagableStorageHandle::deserialize_arc_by_key`]: crate::storage::handle::PagableStorageHandle::deserialize_arc_by_key
    ///
    /// Takes a function pointer that performs the actual deserialization. The
    /// function receives both a type-erased deserializer and an
    /// `Arc<dyn PagableDeserializerRecipe>`. Callbacks that want to defer further
    /// reads (e.g. partial-deser) can retain or stash the recipe `Arc` for later
    /// reopening.
    ///
    /// The `type_id` parameter provides the TypeId of the Arc being deserialized,
    /// which is needed for storage cache lookups.
    fn deserialize_arc(
        &mut self,
        type_id: std::any::TypeId,
        deserialize_fn: for<'a> fn(
            &mut dyn PagableDeserializer<'a>,
            Arc<dyn PagableDeserializerRecipe>,
        ) -> crate::Result<Box<dyn ArcEraseDyn>>,
    ) -> crate::Result<Box<dyn ArcEraseDyn>>;

    /// Consume the next nested arc slot and return its storage key without
    /// fetching or deserializing the blob. Pay for it later, if at all, via
    /// [`PagableStorageHandle::deserialize_arc_by_key`].
    ///
    /// `None` means this backend has no stable key for the slot - nothing is
    /// consumed, and the caller must fall back to
    /// [`deserialize_arc`](Self::deserialize_arc).
    ///
    /// Consuming a slot advances only the cursor's arc index, not its byte
    /// position: the arc's payload is stored in its own row, and the parent's
    /// bytes hold nothing for it. A backend with stable keys returns an error
    /// if no slot remains, without advancing the cursor.
    ///
    /// [`PagableStorageHandle::deserialize_arc_by_key`]: crate::storage::handle::PagableStorageHandle::deserialize_arc_by_key
    fn take_arc_key(&mut self) -> crate::Result<Option<DataKey>>;

    /// Returns a reference to the storage handle used for paging operations.
    ///
    /// This allows deserializers to create [`PagableArc`](crate::PagableArc) instances
    /// that are connected to the appropriate storage backend for future paging.
    fn storage(&self) -> PagableStorageHandle;

    /// Returns this deserializer as a trait object.
    ///
    /// This is useful when you need to pass the deserializer to code that
    /// works with `dyn PagableDeserializer` rather than generic types.
    fn as_dyn(&mut self) -> &mut dyn PagableDeserializer<'de>;

    /// Access state owned by the storage backend.
    fn storage_context(&self) -> &StorageContext;
}

static_assertions::assert_obj_safe!(PagableDeserializer<'_>);

impl<'de, D: PagableDeserializer<'de> + ?Sized> PagableDeserializer<'de> for &mut D {
    fn serde(&mut self) -> &mut postcard::Deserializer<'de, crate::flavors::PagableSlice<'de>> {
        <D as PagableDeserializer<'de>>::serde(self)
    }

    fn deserialize_arc(
        &mut self,
        type_id: TypeId,
        deserialize_fn: for<'a> fn(
            &mut dyn PagableDeserializer<'a>,
            Arc<dyn PagableDeserializerRecipe>,
        ) -> crate::Result<Box<dyn ArcEraseDyn>>,
    ) -> crate::Result<Box<dyn ArcEraseDyn>> {
        <D as PagableDeserializer<'de>>::deserialize_arc(self, type_id, deserialize_fn)
    }

    fn take_arc_key(&mut self) -> crate::Result<Option<DataKey>> {
        <D as PagableDeserializer<'de>>::take_arc_key(self)
    }

    fn position(&self) -> PagableCursor {
        <D as PagableDeserializer<'de>>::position(self)
    }

    unsafe fn seek(&mut self, cursor: PagableCursor) {
        unsafe { <D as PagableDeserializer<'de>>::seek(self, cursor) }
    }

    fn storage(&self) -> PagableStorageHandle {
        <D as PagableDeserializer<'de>>::storage(self)
    }

    fn as_dyn(&mut self) -> &mut dyn PagableDeserializer<'de> {
        self
    }

    fn storage_context(&self) -> &StorageContext {
        <D as PagableDeserializer<'de>>::storage_context(self)
    }
}

static_assertions::assert_impl_all!(dyn PagableDeserializer<'static>: PagableDeserializer<'static>);

#[cfg(test)]
mod tests {
    #[cfg(not(miri))]
    use std::sync::Barrier;
    #[cfg(not(miri))]
    use std::sync::atomic::AtomicUsize;
    #[cfg(not(miri))]
    use std::sync::atomic::Ordering;

    use super::*;

    struct TestState(usize);

    impl StorageState for TestState {}

    #[test]
    fn storage_context_initializes_state_once() {
        let context = StorageContext::new();

        assert!(context.get::<TestState>().is_none());
        let state = context.get_or_init(|| TestState(1));

        assert_eq!(state.0, 1);
        assert!(Arc::ptr_eq(&state, &context.get_or_init(|| TestState(2)),));
    }

    // `parking_lot` contention uses exposed provenance and platform APIs unsupported by Miri.
    #[cfg(not(miri))]
    #[test]
    fn storage_context_initializes_state_once_concurrently() {
        const THREADS: usize = 8;

        let context = Arc::new(StorageContext::new());
        let barrier = Arc::new(Barrier::new(THREADS));
        let initialization_count = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let context = Arc::clone(&context);
                let barrier = Arc::clone(&barrier);
                let initialization_count = Arc::clone(&initialization_count);
                std::thread::spawn(move || {
                    barrier.wait();
                    context.get_or_init(|| {
                        initialization_count.fetch_add(1, Ordering::Relaxed);
                        TestState(1)
                    })
                })
            })
            .collect();

        let states: Vec<_> = handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .expect("state initialization thread should succeed")
            })
            .collect();
        let first = &states[0];

        assert_eq!(initialization_count.load(Ordering::Relaxed), 1);
        assert!(states.iter().all(|state| Arc::ptr_eq(first, state)));
    }
}
