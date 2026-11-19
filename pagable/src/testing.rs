/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Testing utilities for pagable serialization.
//!
//! This module provides simple serializer and deserializer implementations
//! for testing pagable types. The testing implementations store stashed
//! pointers in memory along with their type IDs for runtime type checking.
//! Arcs serialized via `serialize_arc` are serialized inline into the byte stream,
//! and arc identity is preserved across serialization (duplicate arcs are
//! only serialized once).
//!
//! # Example
//!
//! ```ignore
//! use pagable::testing::{TestingSerializer, TestingDeserializer};
//! use pagable::{PagableSerialize, PagableDeserialize};
//!
//! // Serialize
//! let mut ser = TestingSerializer::new();
//! value.pagable_serialize(&mut ser)?;
//! let (bytes, ptrs) = ser.finish();
//!
//! // Deserialize
//! let mut de = TestingDeserializer::new(&bytes, ptrs);
//! let restored = MyType::pagable_deserialize(&mut de)?;
//! ```

use std::any::TypeId;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use allocative::Allocative;
use dupe::Dupe;
use postcard::ser_flavors::Flavor as _;
use serde::Deserialize;
use serde::Serialize;

use crate::PagableDeserializerRecipe;
use crate::arc_erase::ArcEraseDyn;
use crate::arc_erase::ArcSerializeOutcome;
use crate::flavors::PagableSlice;
use crate::flavors::PagableVecFlavor;
use crate::flavors::SharedPosition;
use crate::storage::data::DataKey;
use crate::storage::data::PagableData;
use crate::storage::handle::PagableStorageHandle;
use crate::storage::traits::CommitFrontier;
use crate::storage::traits::DeserializedArcCache;
use crate::storage::traits::PagableStorage;
use crate::storage::traits::WriteTicket;
use crate::traits::PagableCursor;
use crate::traits::PagableDeserializer;
use crate::traits::PagableSerializer;
use crate::traits::StorageContext;

/// A simple in-memory serializer for testing pagable types.
///
/// This serializer uses postcard for serde serialization and stores stashed
/// pointers in a vector along with their type IDs. Arcs serialized via `serialize_arc`
/// are serialized inline into the byte stream, with arc identity preserved
/// (duplicate arcs are only serialized once). After serialization,
/// call [`finish`](Self::finish) to retrieve the serialized bytes and pointers.
pub struct TestingSerializer {
    serde: postcard::Serializer<PagableVecFlavor>,
    seen_arcs: HashSet<(TypeId, usize)>,
    /// Only used to populate `PagableCursor::arc_index`. Not meaningful for
    /// testing because arcs are serialized inline in the byte stream.
    arc_count: usize,
    storage_context: StorageContext,
}

impl TestingSerializer {
    /// Create a new testing serializer.
    pub fn new() -> Self {
        Self {
            serde: postcard::Serializer {
                output: PagableVecFlavor::new(),
            },
            seen_arcs: HashSet::new(),
            arc_count: 0,
            storage_context: StorageContext::new(),
        }
    }

    /// Finish serialization and return the serialized bytes.
    pub fn finish(self) -> Vec<u8> {
        self.serde.output.finalize().unwrap()
    }
}

impl Default for TestingSerializer {
    fn default() -> Self {
        Self::new()
    }
}

impl PagableSerializer for TestingSerializer {
    fn serde(&mut self) -> &mut postcard::Serializer<PagableVecFlavor> {
        &mut self.serde
    }

    fn serialize_arc(&mut self, arc: &dyn ArcEraseDyn) -> crate::Result<()> {
        let identity = arc.identity();
        // Always write identity first
        identity.serialize(self.serde())?;

        if self
            .seen_arcs
            .insert((arc.as_arc_any().type_id(), identity))
        {
            // First time seeing this arc, serialize its contents
            match arc.serialize(self)? {
                ArcSerializeOutcome::Serialized => {}
                ArcSerializeOutcome::ReuseDataKey(key) => {
                    // This format stores Arc payloads inline and has no way to
                    // encode a reference to an external storage key.
                    return Err(anyhow::anyhow!(
                        "TestingSerializer cannot reuse stored Arc representation {key:?}"
                    ));
                }
            }
        }
        self.arc_count += 1;
        // If already seen, nothing more to write - identity is enough
        Ok(())
    }

    fn position(&mut self) -> PagableCursor {
        PagableCursor {
            byte_pos: self.serde.output.position(),
            arc_index: self.arc_count,
        }
    }

    fn storage_context(&self) -> &StorageContext {
        &self.storage_context
    }
}

/// A simple in-memory deserializer for testing pagable types.
///
/// This deserializer uses postcard for serde deserialization and retrieves
/// stashed pointers from a vector. Arcs are deserialized inline from the byte
/// stream, with arc identity preserved (duplicate arcs point to the same
/// allocation). Type IDs are checked during unstashing to catch type mismatches.
pub struct TestingDeserializer<'de> {
    /// Owned copy of the input. Recipes clone this `Arc` instead of copying.
    bytes_arc: Arc<[u8]>,
    pos: SharedPosition,
    serde: postcard::Deserializer<'de, PagableSlice<'de>>,
    seen_arcs: HashMap<(TypeId, usize), Box<dyn ArcEraseDyn>>,
    /// Only used to populate `PagableCursor::arc_index`. Not meaningful for
    /// testing because arcs are deserialized inline from the byte stream.
    arc_index: usize,
    storage: PagableStorageHandle,
}

impl<'de> TestingDeserializer<'de> {
    /// Create a new testing deserializer.
    ///
    /// The `bytes` should come from a previous call to
    /// [`TestingSerializer::finish`].
    pub fn new(bytes: &'de [u8]) -> Self {
        let pos = SharedPosition::new();
        let storage = PagableStorageHandle::new(Arc::new(EmptyPagableStorage::new()));
        Self {
            bytes_arc: Arc::from(bytes.to_vec().into_boxed_slice()),
            pos: pos.clone(),
            serde: postcard::Deserializer::from_flavor(PagableSlice::new(bytes, pos)),
            seen_arcs: HashMap::new(),
            arc_index: 0,
            storage,
        }
    }

    /// Construct a deserializer sharing an existing `Arc<[u8]>`.
    pub(crate) fn from_bytes_arc(bytes: &'de Arc<[u8]>, storage: PagableStorageHandle) -> Self {
        let pos = SharedPosition::new();
        Self {
            bytes_arc: bytes.dupe(),
            pos: pos.clone(),
            serde: postcard::Deserializer::from_flavor(PagableSlice::new(bytes, pos)),
            seen_arcs: HashMap::new(),
            arc_index: 0,
            storage,
        }
    }
}

impl<'de> PagableDeserializer<'de> for TestingDeserializer<'de> {
    fn serde(&mut self) -> &mut postcard::Deserializer<'de, crate::flavors::PagableSlice<'de>> {
        &mut self.serde
    }

    fn position(&self) -> PagableCursor {
        PagableCursor {
            byte_pos: self.pos.get(),
            arc_index: self.arc_index,
        }
    }

    unsafe fn seek(&mut self, cursor: PagableCursor) {
        self.pos.set(cursor.byte_pos);
        self.arc_index = cursor.arc_index;
    }

    fn deserialize_arc(
        &mut self,
        type_id: TypeId,
        deserialize_fn: for<'a> fn(
            &mut dyn PagableDeserializer<'a>,
            Arc<dyn PagableDeserializerRecipe>,
        ) -> crate::Result<Box<dyn ArcEraseDyn>>,
    ) -> crate::Result<Box<dyn ArcEraseDyn>> {
        // Read identity first
        let identity: usize = Deserialize::deserialize(&mut self.serde)?;

        self.arc_index += 1;
        if let Some(arc_dyn) = self.seen_arcs.get(&(type_id, identity)) {
            // Already seen - return a clone
            Ok(arc_dyn.clone_dyn())
        } else {
            // First time - deserialize, store in map, return
            let recipe: Arc<dyn PagableDeserializerRecipe> = Arc::new(TestingRecipe {
                bytes: self.bytes_arc.dupe(),
            });
            let arc = deserialize_fn(self, recipe)?;
            self.seen_arcs.insert((type_id, identity), arc.clone_dyn());
            Ok(arc)
        }
    }

    /// Arcs are identified inline in the byte stream here rather than by
    /// `DataKey`, so there is no key to hand out.
    fn take_arc_key(&mut self) -> crate::Result<Option<DataKey>> {
        Ok(None)
    }

    fn storage(&self) -> PagableStorageHandle {
        self.storage.dupe()
    }

    fn as_dyn(&mut self) -> &mut dyn crate::traits::PagableDeserializer<'de> {
        self
    }

    fn storage_context(&self) -> &StorageContext {
        self.storage.backing_storage().storage_context()
    }
}

pub(crate) struct EmptyPagableStorage {
    arc_cache: DeserializedArcCache,
    storage_context: StorageContext,
}

impl EmptyPagableStorage {
    pub(crate) fn new() -> Self {
        Self {
            arc_cache: DeserializedArcCache::new(),
            storage_context: StorageContext::new(),
        }
    }
}

#[derive(Allocative)]
pub(crate) struct TestingRecipe {
    bytes: Arc<[u8]>,
}

impl PagableDeserializerRecipe for TestingRecipe {
    fn open<'a>(
        &'a self,
        storage: &'a PagableStorageHandle,
    ) -> Box<dyn PagableDeserializer<'a> + 'a> {
        Box::new(TestingDeserializer::from_bytes_arc(
            &self.bytes,
            storage.dupe(),
        ))
    }

    fn retained_data_len(&self) -> usize {
        self.bytes.len()
    }
}

#[async_trait::async_trait]
impl PagableStorage for EmptyPagableStorage {
    fn commit_frontier(&self) -> CommitFrontier {
        CommitFrontier::everything()
    }

    fn discard_unwritten(&self) {}

    fn arc_cache(&self) -> &DeserializedArcCache {
        &self.arc_cache
    }

    fn fetch_data_blocking(&self, _key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        Err(anyhow::anyhow!(
            "No storage available for testing deserializer"
        ))
    }

    async fn fetch_data(&self, _key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        Err(anyhow::anyhow!(
            "No storage available for testing deserializer"
        ))
    }

    fn schedule_for_paging(&self, _arc: Box<dyn ArcEraseDyn>) {
        // no-op
    }

    fn storage_context(&self) -> &StorageContext {
        &self.storage_context
    }

    fn store_data_ticketed(&self, data: PagableData) -> anyhow::Result<(DataKey, WriteTicket)> {
        Ok((data.compute_key(), WriteTicket::DURABLE))
    }
}
