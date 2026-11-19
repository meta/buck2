/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use crate::arc_erase::ArcEraseDyn;
use crate::storage::data::DataKey;
use crate::storage::data::PagableData;
use crate::storage::traits::CommitFrontier;
use crate::storage::traits::DeserializedArcCache;
use crate::storage::traits::PagableStorage;
use crate::storage::traits::WriteTicket;
use crate::traits::StorageContext;

/// No-op storage backend that serializes data but discards it immediately.
///
/// Useful for benchmarking serialization cost in isolation, without any
/// storage I/O overhead.
pub struct NoopPagableStorage {
    arc_cache: DeserializedArcCache,
    storage_context: StorageContext,
}

impl NoopPagableStorage {
    pub fn new() -> Self {
        Self {
            arc_cache: DeserializedArcCache::new(),
            storage_context: StorageContext::new(),
        }
    }
}

#[async_trait::async_trait]
impl PagableStorage for NoopPagableStorage {
    fn commit_frontier(&self) -> CommitFrontier {
        CommitFrontier::everything()
    }

    fn discard_unwritten(&self) {}

    fn arc_cache(&self) -> &DeserializedArcCache {
        &self.arc_cache
    }

    fn fetch_data_blocking(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        Err(anyhow::anyhow!(
            "NoopPagableStorage does not store data (key {:?})",
            key,
        ))
    }

    async fn fetch_data(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        Err(anyhow::anyhow!(
            "NoopPagableStorage does not store data (key {:?})",
            key,
        ))
    }

    fn schedule_for_paging(&self, _arc: Box<dyn ArcEraseDyn>) {}

    fn storage_context(&self) -> &StorageContext {
        &self.storage_context
    }

    fn store_data_ticketed(&self, data: PagableData) -> anyhow::Result<(DataKey, WriteTicket)> {
        Ok((data.compute_key(), WriteTicket::DURABLE))
    }
}
