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
use std::sync::Arc;

use anyhow::Context;
use dupe::Dupe;
use postcard::ser_flavors::Flavor;

use crate::PagableDeserializer;
use crate::PagableDeserializerRecipe;
use crate::PagableSerializer;
use crate::arc_erase::ArcEraseDyn;
use crate::storage::data::DataKey;
use crate::storage::data::PagableData;
use crate::storage::handle::PagableStorageHandle;
use crate::traits::PagableCursor;
use crate::traits::StorageContext;

/// Concrete implementation of [`PagableSerializer`] backed by postcard.
///
/// Serializes data using the postcard binary format while tracking nested Arc
/// references separately for deduplication and lazy loading support.
pub struct PagableSerializerImpl {
    pub(crate) inner: postcard::Serializer<crate::flavors::PagableVecFlavor>,
    arcs: Vec<Box<dyn ArcEraseDyn>>,
    storage_context: StorageContext,
}

/// Result of serialization containing the raw bytes and nested arc references.
///
/// The `arcs` field contains type-erased arcs that were encountered during
/// serialization. These will be serialized separately and replaced with
/// [`DataKey`] references in the final storage format.
pub struct SerializedData {
    pub data: Vec<u8>,
    pub arcs: Vec<Box<dyn ArcEraseDyn>>,
}

impl PagableSerializerImpl {
    pub fn testing_new() -> Self {
        Self {
            inner: postcard::Serializer {
                output: crate::flavors::PagableVecFlavor::new(),
            },
            arcs: Vec::new(),
            storage_context: StorageContext::new(),
        }
    }

    pub fn finish(self) -> anyhow::Result<SerializedData> {
        Ok(SerializedData {
            data: self.inner.output.finalize()?,
            arcs: self.arcs,
        })
    }
}

impl PagableSerializer for PagableSerializerImpl {
    fn serde(&mut self) -> &mut postcard::Serializer<crate::flavors::PagableVecFlavor> {
        &mut self.inner
    }

    fn serialize_arc(&mut self, arc: &dyn ArcEraseDyn) -> anyhow::Result<()> {
        let arc = arc.clone_dyn();
        self.arcs.push(arc as _);
        Ok(())
    }

    fn position(&mut self) -> PagableCursor {
        PagableCursor {
            byte_pos: self.inner.output.position(),
            arc_index: self.arcs.len(),
        }
    }

    fn storage_context(&self) -> &StorageContext {
        &self.storage_context
    }
}

/// Concrete implementation of [`PagableDeserializer`] backed by postcard.
///
/// Deserializes data from the postcard binary format while resolving nested Arc
/// references through the storage backend. Supports both cached arc retrieval
/// (fast path) and lazy deserialization from raw data.
pub struct PagableDeserializerImpl<'de, 's> {
    // Position of the deserializer in the data buffer
    pos: crate::flavors::SharedPosition,
    // Index of the next arc to be deserialized
    arc_index: usize,

    inner: postcard::Deserializer<'de, crate::flavors::PagableSlice<'de>>,
    arcs: &'de [DataKey],
    storage: &'s PagableStorageHandle,
}

impl<'de, 's> PagableDeserializerImpl<'de, 's> {
    pub(crate) fn new(data: &'de PagableData, storage: &'s PagableStorageHandle) -> Self {
        let pos = crate::flavors::SharedPosition::new();
        Self {
            pos: pos.clone(),
            inner: postcard::Deserializer::from_flavor(crate::flavors::PagableSlice::new(
                &data.data, pos,
            )),
            arcs: &data.arcs,
            arc_index: 0,
            storage,
        }
    }

    fn take_stored_arc_key(&mut self) -> crate::Result<DataKey> {
        let key = self.arcs.get(self.arc_index).copied().ok_or_else(|| {
            anyhow::anyhow!(
                "Arc slot index {} out of bounds ({} slots)",
                self.arc_index,
                self.arcs.len(),
            )
        })?;
        self.arc_index += 1;
        Ok(key)
    }
}

impl<'de, 's> PagableDeserializer<'de> for PagableDeserializerImpl<'de, 's> {
    fn serde(&mut self) -> &mut postcard::Deserializer<'de, crate::flavors::PagableSlice<'de>> {
        &mut self.inner
    }

    fn deserialize_arc(
        &mut self,
        type_id: TypeId,
        deserialize_fn: for<'a> fn(
            &mut dyn PagableDeserializer<'a>,
            Arc<dyn PagableDeserializerRecipe>,
        ) -> crate::Result<Box<dyn ArcEraseDyn>>,
    ) -> crate::Result<Box<dyn ArcEraseDyn>> {
        let key = self
            .take_stored_arc_key()
            .with_context(|| format!("Deserializing arc with type {type_id:?}"))?;
        self.storage
            .deserialize_arc_by_key(key, type_id, deserialize_fn)
    }

    fn take_arc_key(&mut self) -> crate::Result<Option<DataKey>> {
        Ok(Some(self.take_stored_arc_key()?))
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

    fn storage(&self) -> PagableStorageHandle {
        self.storage.dupe()
    }

    fn as_dyn(&mut self) -> &mut dyn PagableDeserializer<'de> {
        self
    }

    fn storage_context(&self) -> &StorageContext {
        self.storage.backing_storage().storage_context()
    }
}
