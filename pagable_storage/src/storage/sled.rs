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
use std::sync::Mutex;
use std::sync::mpsc;

use pagable::arc_erase::ArcEraseDyn;
use pagable::storage::data::DataKey;
use pagable::storage::data::PagableData;
use pagable::storage::support::SerializerForPaging;
use pagable::storage::traits::CommitFrontier;
use pagable::storage::traits::DeserializedArcCache;
use pagable::storage::traits::PagableStorage;
use pagable::storage::traits::WriteTicket;
use pagable::traits::StorageContext;

/// Sled-backed storage backend for pagable data.
///
/// This storage implementation persists paged-out data to a sled embedded
/// key-value database. It provides:
/// - Serialized data storage indexed by content-addressable `DataKey`
/// - Arc caching to avoid redundant deserialization
/// - Background serialization queue via mpsc channel
///
/// This is primarily useful for testing the pagable framework.
pub struct SledBackedPagableStorage {
    sender: mpsc::Sender<Box<dyn ArcEraseDyn>>,
    db: sled::Db,
    arcs: DeserializedArcCache,
    pending: Mutex<SledPendingPageOut>,
    storage_context: StorageContext,
}

/// Internal state for tracking pending paging operations.
struct SledPendingPageOut {
    pending_messages: mpsc::Receiver<Box<dyn ArcEraseDyn>>,
    pending: Vec<Box<dyn ArcEraseDyn>>,
}

impl SledBackedPagableStorage {
    pub fn try_new(path: &std::path::Path) -> anyhow::Result<Self> {
        let db = sled::Config::new()
            .cache_capacity(1024 * 1024 * 2) // 2mb
            .path(path)
            .open()?;
        let (sender, receiver) = mpsc::channel();
        Ok(Self {
            sender,
            db,
            arcs: DeserializedArcCache::new(),
            pending: Mutex::new(SledPendingPageOut {
                pending_messages: receiver,
                pending: Vec::new(),
            }),
            storage_context: StorageContext::new(),
        })
    }

    /// Returns the number of arcs queued for paging but not yet serialized.
    ///
    /// This drains the channel and returns the total count of pending arcs.
    /// Note that arcs may still be in-flight in the channel when this returns.
    pub fn pending_paging_count(&self) -> usize {
        let mut lock = self.pending.lock().expect("lock poisoned");
        while let Ok(v) = lock.pending_messages.try_recv() {
            lock.pending.push(v);
        }
        lock.pending.len()
    }

    /// Recursively serialize a set of arcs and their dependencies, storing each
    /// into sled. Updates `finished` with the identity -> DataKey mapping for
    /// each processed arc.
    fn serialize_arcs(
        &self,
        roots: Vec<Box<dyn ArcEraseDyn>>,
        finished: &mut HashMap<usize, DataKey>,
        storage_context: &StorageContext,
    ) -> anyhow::Result<()> {
        enum Task {
            Start(Box<dyn ArcEraseDyn>),
            Finish((Box<dyn ArcEraseDyn>, Vec<u8>, Vec<Box<dyn ArcEraseDyn>>)),
        }

        let mut tasks: Vec<Task> = roots.into_iter().map(Task::Start).collect();

        while let Some(task) = tasks.pop() {
            match task {
                Task::Start(v) => {
                    if finished.contains_key(&v.identity()) {
                        continue;
                    }

                    if let Some(key) = v.data_key() {
                        self.associate_arc_with_data_key(&*v, key);
                        finished.insert(v.identity(), key);
                        continue;
                    }

                    let mut serializer = SerializerForPaging::new(storage_context);
                    v.serialize(&mut serializer)?;
                    let (data, arcs) = serializer.finish();

                    let subtasks: Vec<_> = arcs
                        .iter()
                        .filter(|arc| !finished.contains_key(&arc.identity()))
                        .map(|arc| Task::Start(arc.clone_dyn()))
                        .collect();

                    tasks.push(Task::Finish((v, data, arcs)));
                    tasks.extend(subtasks);
                }
                Task::Finish((arc, data, serialized_arcs)) => {
                    let arcs = serialized_arcs
                        .iter()
                        .map(|arc| {
                            *finished
                                .get(&arc.identity())
                                .expect("nested arc should have been serialized first")
                        })
                        .collect();

                    let key = self.store_data(PagableData { data, arcs })?;
                    finished.insert(arc.identity(), key);
                    self.associate_arc_with_data_key(&*arc, key);
                }
            }
        }
        Ok(())
    }

    /// Processes all pending arcs, recursively serializing them and their dependencies.
    ///
    /// This method:
    /// 1. Drains the pending queue
    /// 2. For each arc, recursively serializes it and all nested arcs
    /// 3. Computes content-addressable `DataKey`s via blake3 hashing
    /// 4. Stores the serialized data and updates arcs with their keys
    ///
    /// This is typically called explicitly in tests or by a background thread in production.
    pub fn page_out_pending(&self) -> anyhow::Result<()> {
        loop {
            // Drain the channel and pop one item while holding the pending lock,
            // then drop it before accessing storage_context to avoid deadlock.
            let item = {
                let mut lock = self.pending.lock().expect("lock poisoned");
                while let Ok(v) = lock.pending_messages.try_recv() {
                    lock.pending.push(v);
                }
                lock.pending.pop()
            };

            match item {
                Some(v) if v.needs_paging_out() => {
                    let mut finished: HashMap<usize, DataKey> = HashMap::new();
                    self.serialize_arcs(vec![v], &mut finished, &self.storage_context)?;
                }
                Some(_) => continue,
                None => break,
            }
        }
        Ok(())
    }

    pub fn write_bytes<T: bytemuck::Pod>(&self, key: &str, data: T) {
        self.db
            .insert(key.as_bytes(), sled::IVec::from(bytemuck::bytes_of(&data)))
            .expect("sled insert failed");
    }

    pub fn flush(&self) {
        self.db.flush().expect("sled flush failed");
    }

    pub fn fetch_bytes_blocking<T: bytemuck::Pod>(&self, key: &str) -> anyhow::Result<T> {
        let bytes = self
            .db
            .get(key.as_bytes())?
            .ok_or_else(|| anyhow::anyhow!("no data for key {:?}", key))?;
        Ok(bytemuck::pod_read_unaligned(&bytes))
    }

    pub fn fetch_data_blocking(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        let bytes = self
            .db
            .get(bytemuck::bytes_of(key))?
            .ok_or_else(|| anyhow::anyhow!("no data for key {:?}", key))?;
        Self::decode_pagable_data(&bytes, key)
    }

    fn decode_pagable_data(bytes: &[u8], key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        if bytes.len() < 16 {
            return Err(anyhow::anyhow!(
                "corrupt sled entry for key {:?}: too short for header",
                key
            ));
        }
        let data_len = u64::from_le_bytes(bytes[..8].try_into()?) as usize;
        let arcs_len = u64::from_le_bytes(bytes[8..16].try_into()?) as usize;
        let expected_len = arcs_len
            .checked_mul(16)
            .and_then(|v| v.checked_add(data_len))
            .and_then(|v| v.checked_add(16))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "corrupt sled entry for key {:?}: length overflow (data_len={}, arcs_len={})",
                    key,
                    data_len,
                    arcs_len
                )
            })?;
        if bytes.len() < expected_len {
            return Err(anyhow::anyhow!(
                "corrupt sled entry for key {:?}: expected {} bytes, got {}",
                key,
                expected_len,
                bytes.len()
            ));
        }
        let data = bytes[16..16 + data_len].to_vec();
        let arcs = (0..arcs_len)
            .map(|i| {
                let offset = 16 + data_len + i * 16;
                DataKey::from_stored_bytes(bytes[offset..offset + 16].try_into()?)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Arc::new(PagableData { data, arcs }))
    }
}

#[async_trait::async_trait]
impl PagableStorage for SledBackedPagableStorage {
    fn commit_frontier(&self) -> CommitFrontier {
        CommitFrontier::everything()
    }

    fn discard_unwritten(&self) {}

    fn arc_cache(&self) -> &DeserializedArcCache {
        &self.arcs
    }

    fn fetch_data_blocking(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        SledBackedPagableStorage::fetch_data_blocking(self, key)
    }

    #[cfg(feature = "tokio")]
    async fn fetch_data(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        let db = self.db.clone();
        let key = *key;
        tokio::task::spawn_blocking(move || {
            let bytes = db
                .get(bytemuck::bytes_of(&key))?
                .ok_or_else(|| anyhow::anyhow!("no data for key {:?}", key))?;
            Self::decode_pagable_data(&bytes, &key)
        })
        .await?
    }

    #[cfg(not(feature = "tokio"))]
    async fn fetch_data(&self, _key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        Err(anyhow::anyhow!("sled backend requires tokio feature"))
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

    /// Serialize `PagableData` into the on-disk byte format and insert into sled.
    /// Returns the content-addressable `DataKey`.
    fn store_data_ticketed(&self, data: PagableData) -> anyhow::Result<(DataKey, WriteTicket)> {
        let key = data.compute_key();
        let db_key = bytemuck::bytes_of(&key);
        if self.db.contains_key(db_key)? {
            return Ok((key, WriteTicket::DURABLE));
        }

        let bytes_size = 8 + 8 + data.data.len() + data.arcs.len() * 16;
        let mut bytes = Vec::with_capacity(bytes_size);
        bytes.extend_from_slice(&(data.data.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(data.arcs.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&data.data);
        bytes.extend_from_slice(bytemuck::cast_slice(&data.arcs));
        assert_eq!(bytes.len(), bytes_size);

        self.db.insert(db_key, sled::IVec::from(bytes))?;
        Ok((key, WriteTicket::DURABLE))
    }
}
