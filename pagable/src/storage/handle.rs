/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::TypeId;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::Weak;

use dupe::Dupe;

use crate::Pagable;
use crate::PagableDeserializer;
use crate::PagableDeserializerRecipe;
use crate::arc_erase::ArcErase;
use crate::arc_erase::ArcEraseDyn;
use crate::context::PagableDeserializerImpl;
use crate::deferred_value::HeldCell;
use crate::deser_recipe::PagableDeserializerRecipeImpl;
use crate::pagable_arc::PagableArc;
use crate::storage::data::DataKey;
use crate::storage::data::PagableData;
use crate::storage::traits::PagableStorage;
use crate::traits::StorageContext;

/// Handle for interacting with pagable storage.
///
/// This is a typed wrapper around the `PagableStorage` trait object that provides
/// a cleaner, type-safe API for consumers. It's cheaply cloneable via `Dupe`.
#[derive(Clone, Dupe)]
pub struct PagableStorageHandle {
    backing_storage: Arc<dyn PagableStorage>,
}

/// A storage reference that cannot keep the storage's own cached values alive.
#[derive(Clone)]
pub struct WeakPagableStorageHandle {
    backing_storage: Weak<dyn PagableStorage>,
}

impl WeakPagableStorageHandle {
    /// Retain the storage for an operation, or return `None` after it closes.
    pub fn upgrade(&self) -> Option<PagableStorageHandle> {
        self.backing_storage
            .upgrade()
            .map(PagableStorageHandle::new)
    }
}

impl PagableStorageHandle {
    /// Runs independent blocking loads against this storage at the same time
    /// on the current multi-thread Tokio runtime's blocking pool and returns
    /// the first failure. Returns only once every load has finished, so a
    /// load may use data the caller keeps alive for the call. Blocks the
    /// calling thread; a runtime worker hands off its queue first.
    #[cfg(feature = "tokio")]
    pub fn load_many(
        &self,
        loads: Vec<Box<dyn FnOnce() -> crate::Result<()> + Send>>,
    ) -> crate::Result<()> {
        let runtime = tokio::runtime::Handle::current();
        let outcomes = tokio::task::block_in_place(|| {
            runtime.block_on(crate::prepare::prepare_all(loads, |load| load()))
        })
        .map_err(anyhow::Error::new)?;
        outcomes
            .into_iter()
            .find_map(Result::err)
            .map_or(Ok(()), Err)
    }

    /// Refer to storage from one of its cached values without an ownership cycle.
    pub fn downgrade(&self) -> WeakPagableStorageHandle {
        WeakPagableStorageHandle {
            backing_storage: Arc::downgrade(&self.backing_storage),
        }
    }

    /// Deserializes data from storage for the given key.
    ///
    /// This method will be implemented to fetch data from storage and deserialize it
    /// into the requested type.
    pub async fn deserialize_pagable_data<T: Pagable>(&self, key: &DataKey) -> anyhow::Result<T> {
        // Fetch the raw data from storage
        let data = self.backing_storage.fetch_data(key).await?;

        let mut deserializer = self.root_deserializer(*key, &data);

        // Deserialize the pagable data
        T::pagable_deserialize(&mut deserializer)
    }

    /// Schedules a pagable arc for eviction to storage.
    ///
    /// Called internally when a `PagableArc` becomes fully unpinned.
    pub fn schedule_for_paging<T: Pagable>(&self, data: PagableArc<T>) {
        let boxed = Box::new(data);
        self.backing_storage.schedule_for_paging(boxed as _)
    }

    /// Creates a new handle wrapping the given storage implementation.
    pub fn new(backing_storage: Arc<dyn PagableStorage>) -> Self {
        Self { backing_storage }
    }

    /// Starts deserializing the root value stored as `data`.
    ///
    /// `root_key` is not needed: the deserializer works from the bytes alone.
    /// It stays in the signature because starlark's tests also build against
    /// the published crate, which takes it, and must compile either way.
    pub fn root_deserializer<'de, 's>(
        &'s self,
        root_key: DataKey,
        data: &'de PagableData,
    ) -> PagableDeserializerImpl<'de, 's> {
        let _ = root_key;
        PagableDeserializerImpl::new(data, self)
    }

    /// Snapshot the live arcs of one concrete type that are bound to a stored
    /// key.
    ///
    /// The returned strong references keep the snapshot stable after the cache
    /// locks are released.
    #[doc(hidden)]
    pub fn deserialized_arcs<T: ArcErase>(&self) -> Vec<T> {
        self.backing_storage.arc_cache().snapshot::<T>()
    }

    /// Fetch and deserialize the arc stored under `key`.
    ///
    /// Takes no cursor, because none is needed: the arc is found by key, not by
    /// position. That is what lets a caller holding only a key it read earlier
    /// (see [`PagableDeserializer::take_arc_key`]) restore the arc long after
    /// the deserializer that read it is gone.
    pub fn deserialize_arc_by_key(
        &self,
        key: DataKey,
        type_id: TypeId,
        deserialize_fn: for<'a> fn(
            &mut dyn PagableDeserializer<'a>,
            Arc<dyn PagableDeserializerRecipe>,
        ) -> crate::Result<Box<dyn ArcEraseDyn>>,
    ) -> crate::Result<Box<dyn ArcEraseDyn>> {
        let storage = self.backing_storage();
        storage.arc_cache().get_or_page_in(type_id, key, || {
            let _held = HeldCell::enter();
            let data = storage.fetch_data_blocking(&key)?;
            let mut deserializer = PagableDeserializerImpl::new(&data, self);
            let recipe: Arc<dyn PagableDeserializerRecipe> =
                Arc::new(PagableDeserializerRecipeImpl::new(data.dupe()));
            let arc = deserialize_fn(&mut deserializer, recipe)?;
            storage.associate_arc_with_data_key(&*arc, key);
            Ok(arc)
        })
    }

    /// Bind an arc under `key` without fetching its row: the cached arc if one
    /// exists, else the one `make` builds, which every later lookup of `key`
    /// then shares.
    ///
    /// For an arc type that can stand in for its row before reading it — a
    /// Starlark heap knows its identity from the slot that named it and reads
    /// the rest on demand — so two referrers of the same key get one
    /// allocation even though neither paid for a fetch.
    pub fn bind_arc_by_key_lazily(
        &self,
        key: DataKey,
        type_id: TypeId,
        make: impl FnOnce() -> Box<dyn ArcEraseDyn>,
    ) -> Box<dyn ArcEraseDyn> {
        let storage = self.backing_storage();
        storage
            .arc_cache()
            .get_or_page_in(type_id, key, || {
                let _held = HeldCell::enter();
                let arc = make();
                // Keyed before it is published: a lookup that finds the arc must
                // find its row's key on it, or a page-out in that window would
                // serialize the arc itself in place of the row.
                storage.associate_arc_with_data_key(&*arc, key);
                Ok::<_, Infallible>(arc)
            })
            .unwrap_or_else(|never| match never {})
    }

    /// Fetch the data under `key` as a reopenable recipe, for a caller that
    /// will parse it itself rather than through
    /// [`deserialize_arc_by_key`](Self::deserialize_arc_by_key).
    pub fn fetch_recipe_by_key(
        &self,
        key: DataKey,
    ) -> crate::Result<Arc<dyn PagableDeserializerRecipe>> {
        let data = self.backing_storage().fetch_data_blocking(&key)?;
        Ok(Arc::new(PagableDeserializerRecipeImpl::new(data)))
    }

    /// Entries in the arc cache, live or not yet swept.
    pub fn arc_cache_entries(&self) -> usize {
        self.backing_storage.arc_cache().len()
    }

    /// Storage-lifetime state, for a caller holding a handle rather than a
    /// deserializer.
    pub fn storage_context(&self) -> &StorageContext {
        self.backing_storage.storage_context()
    }

    pub(crate) fn backing_storage(&self) -> &dyn PagableStorage {
        &*self.backing_storage
    }
}
