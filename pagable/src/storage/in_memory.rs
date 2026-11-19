/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use dashmap::DashMap;
use dupe::Dupe;

use crate::arc_erase::ArcEraseDyn;
use crate::arc_erase::ArcSerializeOutcome;
use crate::storage::data::DataKey;
use crate::storage::data::PagableData;
use crate::storage::support::SerializerForPaging;
use crate::storage::traits::CommitFrontier;
use crate::storage::traits::DeserializedArcCache;
use crate::storage::traits::PagableStorage;
use crate::storage::traits::WriteTicket;
use crate::traits::StorageContext;

/// In-memory storage backend for testing and development.
///
/// This storage implementation keeps all paged-out data in memory using concurrent
/// hash maps. It provides:
/// - Serialized data storage indexed by content-addressable `DataKey`
/// - Arc caching to avoid redundant deserialization
/// - Background serialization queue via mpsc channel
///
/// This is primarily useful for testing the pagable framework and for scenarios
/// where you want paging behavior without actual disk I/O.
pub struct InMemoryPagableStorage {
    pending: InMemoryPagableStoragePendingPageOut,
    handle: Arc<InMemoryPagableStorageHandle>,
}

/// Internal state for tracking pending paging operations.
struct InMemoryPagableStoragePendingPageOut {
    pending_messages: std::sync::mpsc::Receiver<Box<dyn ArcEraseDyn>>,
    pending: Vec<Box<dyn ArcEraseDyn>>,
}

struct InMemoryPagableStorageCache {
    data: DashMap<DataKey, Arc<PagableData>>,
    arcs: DeserializedArcCache,
    is_dropped: AtomicBool,
}

impl InMemoryPagableStorageCache {
    fn new() -> Self {
        Self {
            data: DashMap::new(),
            arcs: DeserializedArcCache::new(),
            is_dropped: AtomicBool::new(false),
        }
    }

    fn dropped(&self) {
        self.is_dropped.store(true, Ordering::Release);
        self.data.clear();
        self.arcs.clear();
    }

    fn insert_data(&self, key: DataKey, data: Arc<PagableData>) {
        self.data.insert(key, data);
        if self.is_dropped.load(Ordering::Acquire) {
            self.data.clear();
        }
    }

    fn get_data(&self, key: &DataKey) -> Option<Arc<PagableData>> {
        self.data.get(key).map(|v| v.dupe())
    }
}

/// Shared handle implementing `PagableStorage` for `InMemoryPagableStorage`.
///
/// This handle is held by `PagableArc` instances and provides access to the
/// shared cache. It implements `PagableStorage` so that arcs can fetch data
/// and cache deserialized arcs. The handle can outlive the owning
/// `InMemoryPagableStorage`; when that is dropped, the cache is cleared
/// and subsequent operations degrade gracefully.
struct InMemoryPagableStorageHandle {
    sender: std::sync::mpsc::Sender<Box<dyn ArcEraseDyn>>,
    cache: InMemoryPagableStorageCache,
    storage_context: StorageContext,
}

impl InMemoryPagableStorage {
    /// Creates a new in-memory storage backend.
    pub fn new() -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();

        Self {
            handle: Arc::new(InMemoryPagableStorageHandle {
                sender,
                cache: InMemoryPagableStorageCache::new(),
                storage_context: StorageContext::new(),
            }),
            pending: InMemoryPagableStoragePendingPageOut {
                pending_messages: receiver,
                pending: Vec::new(),
            },
        }
    }

    pub fn handle(&self) -> Arc<dyn PagableStorage> {
        self.handle.dupe()
    }

    /// Access state owned by this storage instance.
    /// This context is passed to `SerializerForPaging` during `page_out_pending`.
    pub fn storage_context(&self) -> &StorageContext {
        &self.handle.storage_context
    }

    /// Returns the number of arcs queued for paging but not yet serialized.
    ///
    /// This drains the channel and returns the total count of pending arcs.
    /// Note that arcs may still be in-flight in the channel when this returns.
    pub fn pending_paging_count(&mut self) -> usize {
        while let Ok(v) = self.pending.pending_messages.try_recv() {
            self.pending.pending.push(v);
        }
        self.pending.pending.len()
    }

    /// Processes all pending arcs, recursively serializing them and their dependencies.
    ///
    /// This method:
    /// 1. Drains the pending queue
    /// 2. For each arc, recursively serializes it and all nested arcs
    /// 3. Computes content-addressable `DataKey`s via blake3 hashing
    /// 4. Stores the serialized data and updates arcs with their keys
    ///
    /// # Panics
    ///
    /// Panics if serialization of any arc fails. This should only occur if there's
    /// a bug in the implementation of `PagableSerialize` for a type.
    ///
    /// This is typically called explicitly in tests or by a background thread in production.
    pub fn page_out_pending(&mut self) {
        loop {
            while let Ok(v) = self.pending.pending_messages.try_recv() {
                self.pending.pending.push(v);
            }
            if let Some(v) = self.pending.pending.pop() {
                if !v.needs_paging_out() {
                    continue;
                }
                let mut finished: HashMap<usize, DataKey> = HashMap::new();
                enum Task {
                    Start(Box<dyn ArcEraseDyn>),
                    Finish((Box<dyn ArcEraseDyn>, Vec<u8>, Vec<Box<dyn ArcEraseDyn>>)),
                }

                let mut tasks = vec![Task::Start(v)];

                while let Some(task) = tasks.pop() {
                    match task {
                        Task::Start(v) => {
                            if finished.contains_key(&v.identity()) {
                                continue;
                            }

                            if let Some(key) = v.data_key() {
                                self.handle.associate_arc_with_data_key(&*v, key);
                                finished.insert(v.identity(), key);
                                continue;
                            }

                            let mut serializer = SerializerForPaging::new(self.storage_context());
                            if let ArcSerializeOutcome::ReuseDataKey(key) =
                                v.serialize(&mut serializer).unwrap()
                            {
                                self.handle.associate_arc_with_data_key(&*v, key);
                                finished.insert(v.identity(), key);
                                continue;
                            }
                            let (data, arcs) = serializer.finish();

                            let mut subtasks = vec![];
                            for arc in arcs.iter() {
                                if finished.contains_key(&arc.identity()) {
                                    continue;
                                }
                                subtasks.push(Task::Start(arc.clone_dyn()));
                            }

                            tasks.push(Task::Finish((v, data, arcs)));
                            tasks.extend(subtasks);
                        }
                        Task::Finish((arc, data, serialized_arcs)) => {
                            let mut arcs: Vec<DataKey> = Vec::with_capacity(serialized_arcs.len());

                            for arc in serialized_arcs {
                                let key = *finished
                                    .get(&arc.identity())
                                    .expect("nested arc should have been serialized first");
                                arcs.push(key);
                            }

                            let data = PagableData { data, arcs };
                            let key = data.compute_key();
                            self.handle.cache.insert_data(key, Arc::new(data));
                            finished.insert(arc.identity(), key);

                            self.handle.associate_arc_with_data_key(&*arc, key);
                        }
                    }
                }
            } else {
                break;
            }
        }
    }
}

impl Drop for InMemoryPagableStorage {
    fn drop(&mut self) {
        self.handle.cache.dropped()
    }
}

impl Default for InMemoryPagableStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl PagableStorage for InMemoryPagableStorageHandle {
    fn commit_frontier(&self) -> CommitFrontier {
        CommitFrontier::everything()
    }

    fn discard_unwritten(&self) {}

    fn arc_cache(&self) -> &DeserializedArcCache {
        &self.cache.arcs
    }

    fn fetch_data_blocking(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        self.cache
            .get_data(key)
            .ok_or_else(|| anyhow::anyhow!("no data for {:?}", key))
    }

    async fn fetch_data(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        self.cache
            .get_data(key)
            .ok_or_else(|| anyhow::anyhow!("no data for key {:?}", key))
    }

    fn schedule_for_paging(&self, arc: Box<dyn ArcEraseDyn>) {
        // If the receiver has been dropped (which shouldn't happen in normal usage),
        // we just discard the arc. This is acceptable because it means the storage
        // is being shut down and paging is no longer needed.
        drop(self.sender.send(arc));
    }

    fn storage_context(&self) -> &StorageContext {
        &self.storage_context
    }

    fn store_data_ticketed(&self, data: PagableData) -> anyhow::Result<(DataKey, WriteTicket)> {
        let key = data.compute_key();
        self.cache.insert_data(key, Arc::new(data));
        Ok((key, WriteTicket::DURABLE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PagableSerialize;
    use crate::PagableSerializer;
    use crate::PartialPagableArc;
    use crate::arc_erase::ArcErase;

    struct ParentWithStoredChild(PartialPagableArc<u8>);

    impl PagableSerialize for ParentWithStoredChild {
        fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
            self.0.pagable_serialize(serializer)
        }
    }

    #[test]
    fn pending_page_out_reuses_nested_stored_arc() -> crate::Result<()> {
        let mut storage = InMemoryPagableStorage::new();
        let handle = storage.handle();
        let child_key = handle.store_data(PagableData {
            data: vec![42],
            arcs: Vec::new(),
        })?;
        let child = PartialPagableArc::new(42u8);
        ArcErase::set_data_key(&child, child_key);
        let parent = PartialPagableArc::new(ParentWithStoredChild(child));

        handle.schedule_for_paging(Box::new(parent));
        assert_eq!(storage.pending_paging_count(), 1);
        storage.page_out_pending();
        assert_eq!(storage.pending_paging_count(), 0);
        Ok(())
    }
}
