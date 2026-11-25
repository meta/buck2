/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Pagable storage backing for DICE node values.
//!
//! `DiceStorage` wraps any [`pagable::storage::traits::PagableStorage`] backend and
//! exposes APIs for serializing a [`DiceValidValue`] to disk and deserializing it back.
//! Serialization is performed via the bridging methods on [`DiceKeyDyn`] /
//! [`DiceProjectionDyn`], which delegate to each concrete `Key`'s `value_serialize()`.
//!
//! See `Dice::page_out` for the user-facing entry point.

use std::fmt;
use std::fmt::Display;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use dice_error::storage::PagableStorageBackendParseError;
use dupe::Dupe;
use pagable::DataKey;
use pagable::StorageContext;
use pagable::arc_erase::ArcEraseDyn;
use pagable::storage::data::PagableData;
use pagable::storage::handle::PagableStorageHandle;
use pagable::storage::noop::NoopPagableStorage;
use pagable::storage::support::SerializerForPaging;
use pagable::storage::traits::ArcSerCache;
use pagable::storage::traits::CommitFrontier;
use pagable::storage::traits::DeserializedArcCache;
use pagable::storage::traits::EvictExpiredStats;
use pagable::storage::traits::PagableStorage;
use pagable::storage::traits::PageOutError;
use pagable::storage::traits::WriteTicket;
use pagable_storage::storage::sled::SledBackedPagableStorage;
use pagable_storage::storage::sqlite::SqliteBackedPagableStorage;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::watch;

use crate::HashMap;
use crate::core::graph::BackedSnapshot;
use crate::core::state::CoreStateHandle;
use crate::dice::PageOutCancel;
use crate::dice::StorageIoSnapshot;
use crate::key::DiceKey;
use crate::key::DiceKeyErased;
use crate::key_index::AccessCutoff;
use crate::key_index::DiceKeyIndex;
use crate::metrics::AllocWindow;
use crate::metrics::EvictionSnapshot;
use crate::metrics::PageInKeyTypeMetrics;
use crate::metrics::PagingMemoryMetrics;
use crate::metrics::PagingMemorySnapshot;
use crate::value::DiceValidValue;
use crate::value::HydratedValue;
use crate::value::PageOutResult;

/// On-disk backend for pagable DICE storage, from `hydration.pagable_storage_backend`.
#[derive(
    Allocative,
    Clone,
    Copy,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq
)]
#[serde(rename_all = "snake_case")]
pub enum PagableStorageBackend {
    /// SQLite database files.
    #[default]
    Sqlite,
    /// Sled embedded key-value DB.
    Sled,
    /// Serializes but discards data (no I/O).
    Noop,
}

/// Which values a page-out writes, from `hydration.page_out_policy`.
#[derive(
    Allocative,
    Clone,
    Copy,
    Debug,
    Default,
    Dupe,
    Serialize,
    Deserialize,
    PartialEq,
    Eq
)]
#[serde(rename_all = "snake_case")]
pub enum PageOutPolicy {
    /// A key's value is paged out at most once: a value recomputed or read back
    /// after a page-out stays resident, with no row to fall back on.
    OncePerKey,
    /// Each value is written at most once: a recomputed value is written by the
    /// next page-out and stays resident, backed by its row, as does a value read
    /// back from its row. A backed value can be evicted again without a write.
    #[default]
    OncePerValue,
}

/// How an eviction trigger selects resident values with committed backing.
#[derive(Allocative, Clone, Copy, Debug, Default, Dupe, PartialEq, Eq)]
pub enum EvictionStrategy {
    /// Use the trigger's built-in rules: eviction on the first write for idle,
    /// or all backed values in the snapshot for memory pressure.
    #[default]
    Default,
    /// Prefer older transaction demand batches and protect recent demand. Recency is
    /// per key, so demand on any branch protects every retained value of that key.
    Lru,
}

/// When to release resident values and how to select them.
#[derive(
    Allocative,
    Clone,
    Copy,
    Debug,
    Dupe,
    Serialize,
    Deserialize,
    PartialEq,
    Eq
)]
#[serde(from = "EvictionPolicySetting", into = "EvictionPolicySetting")]
pub enum EvictionPolicy {
    /// The default strategy evicts values on their first committed write. LRU keeps
    /// writes resident until a separate idle reclaim selects cold backed values.
    OnIdle(EvictionStrategy),
    /// Writes stay resident until memory pressure invokes the selected strategy.
    OnMemoryPressure(EvictionStrategy),
}

impl Default for EvictionPolicy {
    fn default() -> Self {
        Self::OnMemoryPressure(EvictionStrategy::Default)
    }
}

impl EvictionPolicy {
    pub fn strategy(self) -> EvictionStrategy {
        match self {
            Self::OnIdle(strategy) | Self::OnMemoryPressure(strategy) => strategy,
        }
    }

    pub fn evicts_on_memory_pressure(self) -> bool {
        matches!(self, Self::OnMemoryPressure(_))
    }
}

// Keep daemon settings as flat strings so existing on_idle and on_memory_pressure
// configurations retain their meaning when the runtime enum carries a strategy.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EvictionPolicySetting {
    OnIdle,
    OnMemoryPressure,
    OnMemoryPressureLru,
    OnIdleLru,
}

/// Limits a cold-value reclaim independently of what scheduled it (pressure, idle, or a
/// manual request). Only values with committed backing can be reclaimed.
#[derive(Clone, Copy, Debug)]
pub struct ReclaimOptions {
    /// Completed demand batches to protect in addition to the active batch. Defaults to one
    /// to preserve the latest completed working set. Zero allows the newest batch to be
    /// reclaimed once its transactions and spawned computations close.
    /// Idle grace periods belong to the scheduler and do not advance these batches.
    pub keep_recent_batches: u32,
    /// Maximum keys to inspect from the snapshot. Successive calls rotate their starting
    /// point so a small budget cannot get stuck repeatedly inspecting the same hot keys.
    pub max_candidates: usize,
    /// Soft time budget, checked during selection and between eviction batches. Snapshot
    /// creation and queued core batches are not interruptible.
    pub max_duration: Duration,
}

impl Default for ReclaimOptions {
    fn default() -> Self {
        Self {
            keep_recent_batches: 1,
            max_candidates: usize::MAX,
            max_duration: Duration::MAX,
        }
    }
}

#[derive(Clone, Copy)]
struct ReclaimStop {
    started: Instant,
    max_duration: Duration,
    cancelled: PageOutCancel,
}

impl ReclaimStop {
    fn should_stop(self) -> bool {
        (self.cancelled)() || Instant::now() - self.started >= self.max_duration
    }
}

/// Freeze age classes before rearranging keys. A one-byte class per candidate plus the
/// existing key vector replaces timestamp/key tuples and a comparison sort. The fixed
/// buckets cover logarithmic ages of a u32 batch history, with oldest classes first.
fn order_by_generation(
    keys: &mut [DiceKey],
    classify: impl Fn(DiceKey) -> usize,
    stop: ReclaimStop,
) -> Option<usize> {
    let started = Instant::now();
    let mut classes = Vec::with_capacity(keys.len());
    let mut ends = [0usize; AccessCutoff::AGE_BUCKETS];
    for (i, &key) in keys.iter().enumerate() {
        if i % 1000 == 0 && stop.should_stop() {
            return None;
        }
        let class = classify(key);
        classes.push(class as u8);
        ends[class] += 1;
    }
    let classified = Instant::now();
    for class in 1..ends.len() {
        ends[class] += ends[class - 1];
    }
    let mut next = [0usize; AccessCutoff::AGE_BUCKETS];
    next[1..].copy_from_slice(&ends[..ends.len() - 1]);
    let mut moved = 0;
    for class in 0..ends.len() - 1 {
        while next[class] < ends[class] {
            if moved % 1000 == 0 && stop.should_stop() {
                return None;
            }
            moved += 1;
            let i = next[class];
            let actual = classes[i] as usize;
            if actual == class {
                next[class] += 1;
            } else {
                keys.swap(i, next[actual]);
                classes.swap(i, next[actual]);
                next[actual] += 1;
            }
        }
    }
    tracing::info!(
        inspected_keys = keys.len(),
        eligible_keys = ends[AccessCutoff::AGE_BUCKETS - 2],
        classification_us = (classified - started).as_micros() as u64,
        partition_us = classified.elapsed().as_micros() as u64,
        "Ordered LRU eviction candidates",
    );
    // The final class has age zero: it is already protected by the access cutoff. Avoid
    // allocating batches and waking the core for keys that cannot be evicted.
    (!stop.should_stop()).then_some(ends[AccessCutoff::AGE_BUCKETS - 2])
}

impl From<EvictionPolicySetting> for EvictionPolicy {
    fn from(setting: EvictionPolicySetting) -> Self {
        match setting {
            EvictionPolicySetting::OnIdle => Self::OnIdle(EvictionStrategy::Default),
            EvictionPolicySetting::OnMemoryPressure => {
                Self::OnMemoryPressure(EvictionStrategy::Default)
            }
            EvictionPolicySetting::OnIdleLru => Self::OnIdle(EvictionStrategy::Lru),
            EvictionPolicySetting::OnMemoryPressureLru => {
                Self::OnMemoryPressure(EvictionStrategy::Lru)
            }
        }
    }
}

impl From<EvictionPolicy> for EvictionPolicySetting {
    fn from(policy: EvictionPolicy) -> Self {
        match policy {
            EvictionPolicy::OnIdle(EvictionStrategy::Default) => Self::OnIdle,
            EvictionPolicy::OnMemoryPressure(EvictionStrategy::Default) => Self::OnMemoryPressure,
            EvictionPolicy::OnIdle(EvictionStrategy::Lru) => Self::OnIdleLru,
            EvictionPolicy::OnMemoryPressure(EvictionStrategy::Lru) => Self::OnMemoryPressureLru,
        }
    }
}

impl PagableStorageBackend {
    /// This backend, unless the `PAGABLE_STORAGE_BACKEND` env var is set, in which
    /// case that overrides it. Lets benchmarks pick a backend without a buckconfig.
    pub fn with_env_override(self) -> anyhow::Result<Self> {
        match std::env::var("PAGABLE_STORAGE_BACKEND") {
            Ok(s) => Ok(s.parse()?),
            Err(_) => Ok(self),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            PagableStorageBackend::Sqlite => "sqlite",
            PagableStorageBackend::Sled => "sled",
            PagableStorageBackend::Noop => "noop",
        }
    }
}

impl Display for PagableStorageBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PagableStorageBackend {
    type Err = PagableStorageBackendParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Empty (the buckconfig default) selects SQLite.
        if s.is_empty() || s.eq_ignore_ascii_case("sqlite") {
            Ok(PagableStorageBackend::Sqlite)
        } else if s.eq_ignore_ascii_case("sled") {
            Ok(PagableStorageBackend::Sled)
        } else if s.eq_ignore_ascii_case("noop") {
            Ok(PagableStorageBackend::Noop)
        } else {
            Err(PagableStorageBackendParseError {
                value: s.to_owned(),
            })
        }
    }
}

/// Pagable storage backing for DICE node values.
///
/// Cheaply cloneable: the underlying storage is shared via `Arc<dyn PagableStorage>`.
#[derive(Allocative, Clone, Dupe)]
pub struct DiceStorage {
    #[allocative(skip)]
    storage: Arc<dyn PagableStorage>,
    // `Arc` so the per-worker `dupe()`s in bulk `page_in` share one counter set
    // instead of each cloning a separate map.
    #[allocative(skip)]
    page_in_metrics: Arc<PageInMetrics>,
    /// Shared with the `MeteredPagableStorage` wrapping `storage`, so every
    /// `store_data` / `fetch_data` is tallied.
    #[allocative(skip)]
    io_metrics: Arc<StorageIoMetrics>,
    /// Shared with the core state thread, which measures the page-out side.
    #[allocative(skip)]
    paging_memory: Arc<PagingMemoryMetrics>,
    /// On-disk root of the store, for measuring its size. `None` when constructed
    /// without a path (`new`, e.g. tests), which never has an on-disk footprint to
    /// measure. `Arc<Path>` keeps `DiceStorage` cheap to `dupe`.
    #[allocative(skip)]
    path: Option<Arc<Path>>,
    /// Cached on-disk store size (bytes), refreshed at page-out so the command-end read
    /// needs no filesystem walk. Holds a `Result` so a failed walk surfaces (as a
    /// `soft_error`) instead of reporting 0. `Arc` so `dupe`d handles share one cache.
    #[allocative(skip)]
    cached_db_size_bytes: Arc<ArcSwap<Result<u64, Arc<std::io::Error>>>>,
    /// Which values a page-out writes; see [`PageOutPolicy`].
    page_out_policy: PageOutPolicy,
    /// When the values a page-out writes leave memory; see [`EvictionPolicy`].
    eviction_policy: EvictionPolicy,
}

/// What the core state is given about paging: where to record what paging moves, and which
/// values a page-out writes. A `Dice` without storage never pages and has none.
#[derive(Clone, Dupe)]
pub(crate) struct PagingSetup {
    pub(crate) memory: Arc<PagingMemoryMetrics>,
    pub(crate) page_out_policy: PageOutPolicy,
}

impl DiceStorage {
    /// Construct a `DiceStorage` from any [`PagableStorage`] backend.
    pub fn new(storage: Arc<dyn PagableStorage>) -> Self {
        let io_metrics = Arc::new(StorageIoMetrics::default());
        Self {
            storage: Arc::new(MeteredPagableStorage::new(storage, io_metrics.dupe())),
            page_in_metrics: Arc::new(PageInMetrics::default()),
            io_metrics,
            paging_memory: Arc::new(PagingMemoryMetrics::default()),
            path: None,
            cached_db_size_bytes: Arc::new(ArcSwap::new(Arc::new(Ok(0)))),
            page_out_policy: PageOutPolicy::default(),
            eviction_policy: EvictionPolicy::default(),
        }
    }

    /// Sets which values a page-out writes; see [`PageOutPolicy`]. Only before the storage is
    /// given to a `Dice`, whose core state reads the policy once.
    pub fn set_page_out_policy(&mut self, policy: PageOutPolicy) {
        self.page_out_policy = policy;
    }

    /// Sets when the values a page-out writes leave memory; see [`EvictionPolicy`].
    pub fn set_eviction_policy(&mut self, policy: EvictionPolicy) {
        self.eviction_policy = policy;
    }

    pub(crate) fn eviction_policy(&self) -> EvictionPolicy {
        self.eviction_policy
    }

    pub(crate) fn tracks_accesses(&self) -> bool {
        self.eviction_policy.strategy() == EvictionStrategy::Lru
    }

    pub(crate) fn paging_setup(&self) -> PagingSetup {
        PagingSetup {
            memory: self.paging_memory.dupe(),
            page_out_policy: self.page_out_policy,
        }
    }

    /// Cumulative page-in counters per key type since this `DiceStorage` was
    /// created.
    pub(crate) fn page_in_metrics_snapshot(&self) -> HashMap<&'static str, PageInKeyTypeMetrics> {
        self.page_in_metrics.snapshot()
    }

    /// Storage-scoped configuration and state for pagable value implementations.
    pub fn storage_context(&self) -> &StorageContext {
        self.storage.storage_context()
    }

    pub(crate) fn storage_handle(&self) -> PagableStorageHandle {
        PagableStorageHandle::new(self.storage.dupe())
    }

    /// Cumulative DataKey-level page-out / page-in totals since this `DiceStorage`
    /// was created.
    pub(crate) fn storage_io_snapshot(&self) -> StorageIoSnapshot {
        self.io_metrics.snapshot()
    }

    /// Cumulative memory paging has moved, measured from the allocator.
    pub(crate) fn paging_memory_snapshot(&self) -> Option<PagingMemorySnapshot> {
        self.paging_memory.snapshot()
    }

    /// The last measured on-disk store size in bytes, or `None` if this storage has
    /// no on-disk path (constructed via `new`). `Some(Err)` if the last measurement
    /// walk failed. Cached at page-out, so it is cheap on the command-end path (no
    /// filesystem walk).
    pub(crate) fn on_disk_size_bytes(&self) -> Option<Result<u64, Arc<std::io::Error>>> {
        self.path
            .as_ref()
            .map(|_| (**self.cached_db_size_bytes.load()).clone())
    }

    /// Walk the on-disk store and cache the resulting size (or the walk error), off
    /// the executor via `spawn_blocking`. Called at the end of page-out, the only point
    /// the append-only store changes. No-op without an on-disk path.
    async fn refresh_db_size_bytes(&self) {
        if let Some(path) = self.path.dupe() {
            let result = tokio::task::spawn_blocking(move || dir_size_bytes(&path))
                .await
                .unwrap_or_else(|e| Err(std::io::Error::other(e)))
                .map_err(Arc::new);
            self.cached_db_size_bytes.store(Arc::new(result));
        }
    }

    /// Open (or create) a `DiceStorage` rooted at the given directory, using the
    /// given on-disk `backend`.
    pub fn open(path: &Path, backend: PagableStorageBackend) -> anyhow::Result<Self> {
        Self::open_with_key_read_ahead(path, backend, false)
    }

    /// As [`Self::open`]. `key_read_ahead` is `hydration.key_read_ahead`, which the SQLite
    /// backend acts on; the other backends have no key files to read ahead.
    pub fn open_with_key_read_ahead(
        path: &Path,
        backend: PagableStorageBackend,
        key_read_ahead: bool,
    ) -> anyhow::Result<Self> {
        let storage: Arc<dyn PagableStorage> = match backend {
            PagableStorageBackend::Sled => Arc::new(SledBackedPagableStorage::try_new(path)?),
            PagableStorageBackend::Noop => Arc::new(NoopPagableStorage::new()),
            PagableStorageBackend::Sqlite => Arc::new(
                SqliteBackedPagableStorage::try_new_with_key_read_ahead(path, key_read_ahead)?,
            ),
        };
        // The store is empty until the first page-out, so the size cache starts at 0
        // and is populated there rather than walked here on the async startup path.
        Ok(Self {
            path: Some(Arc::from(path)),
            ..Self::new(storage)
        })
    }

    /// Serialize and page out all paged-in values, then mark them as paged
    /// out on the core state thread.
    ///
    /// `keys` comes from `CoreState::paged_in_keys()`. For entries with an
    /// existing `DataKey` (on-disk copy still valid), no serialization is needed.
    /// For entries without, the value is serialized via the key's `ValueSerialize`.
    ///
    /// Stops after the current chunk if `cancelled` is set, leaving the remaining
    /// keys paged in (a valid state). Pass an always-`false` flag for an
    /// uninterruptible page-out.
    pub(crate) async fn page_out(
        &self,
        keys: Vec<(DiceKey, DiceValidValue)>,
        key_index: &DiceKeyIndex,
        state_handle: &CoreStateHandle,
        cancelled: PageOutCancel,
    ) -> anyhow::Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        // Process this many keys in parallel at a time, limit peak RSS
        const CHUNK_SIZE: usize = 32768;
        let finished = Arc::new(ArcSerCache::new());
        let rows_before = self.io_metrics.data_keys_out.load(Ordering::Relaxed);
        let num_workers = env_concurrency("BUCK2_DICE_PAGE_OUT_WORKERS");
        let pending = Arc::new(PendingEvictions::default());
        let awaiting_commit = AwaitingCommit {
            storage: self.dupe(),
            finished: finished.dupe(),
            pending: pending.dupe(),
            state_handle: state_handle.dupe(),
        };
        let (stop_evictor, stopping) = watch::channel(false);
        let mut evictor = AbortOnDrop(tokio::spawn(
            PageOutEvictor {
                awaiting_commit: awaiting_commit.dupe(),
                stopping,
            }
            .run(),
        ));

        let mut remaining = keys;
        let mut worker_error = None;
        while !remaining.is_empty() {
            if cancelled() {
                break;
            }
            let split_at = remaining.len().min(CHUNK_SIZE);
            let mut chunk = remaining.split_off(split_at);
            std::mem::swap(&mut chunk, &mut remaining);

            let worker_size = chunk.len().div_ceil(num_workers);
            let mut chunk_iter = chunk.into_iter();
            let mut handles = Vec::with_capacity(num_workers);

            for _ in 0..num_workers {
                // Stop spawning workers that would immediately cancel themselves if
                // a command arrived mid-chunk.
                if cancelled() {
                    break;
                }
                let items: Vec<_> = (&mut chunk_iter)
                    .take(worker_size)
                    .map(|(k, v)| (k, key_index.get(k).dupe(), v))
                    .collect();
                if items.is_empty() {
                    break;
                }
                let storage = self.dupe();
                let finished = finished.clone();
                let state_handle = state_handle.dupe();
                let pending = pending.dupe();
                handles.push(tokio::spawn(async move {
                    storage.page_out_chunk(items, &finished, &state_handle, &pending, cancelled)
                }));
            }

            // Join every worker even after a failure - abandoning one mid-chunk
            // would leave it writing and evicting concurrently with the flush
            // below - and keep every error: they may have different root causes.
            let mut chunk_errors: Vec<anyhow::Error> = Vec::new();
            for handle in handles {
                let result = match handle.await {
                    Ok(result) => result,
                    Err(join_error) => Err(join_error.into()),
                };
                if let Err(e) = result {
                    chunk_errors.push(e);
                }
            }
            if !chunk_errors.is_empty() {
                worker_error = Some(if chunk_errors.len() == 1 {
                    chunk_errors.pop().unwrap()
                } else {
                    anyhow::anyhow!(
                        "{} page-out workers failed: {}",
                        chunk_errors.len(),
                        chunk_errors
                            .iter()
                            .map(|e| format!("{e:#}"))
                            .collect::<Vec<_>>()
                            .join("; ")
                    )
                });
                break;
            }
        }

        stop_evictor.send_replace(true);
        if let Err(error) = (&mut evictor.0).await {
            worker_error = Some(match worker_error {
                Some(original) => {
                    original.context(format!("page-out binding worker failed: {error}"))
                }
                None => error.into(),
            });
        }
        if cancelled() {
            // Rows committed by now may already back evicted values, so the
            // arcs those rows hold are bound, and the values they cover
            // committed, before anything is dropped; an arc can reach the
            // unbound list after the tick that evicted a value referencing it.
            // Nothing refers to the rows still queued, so dropping them hands
            // the shards back to the command that cancelled us, instead of
            // draining a queue it would have to read past. The frontier is
            // taken before the discard, which makes the dropped rows look
            // committed.
            let committed = self.storage.commit_frontier();
            self.storage.discard_unwritten();
            awaiting_commit.commit_covered(&committed).await;
            pending.clear();
            finished.discard_unbound();
        }
        let flushed = self.storage.flush();
        // Only rows the flush committed may be bound or evicted: after a failed
        // flush the frontier still names the rows that never landed, and their
        // values stay resident.
        awaiting_commit
            .commit_covered(&self.storage.commit_frontier())
            .await;
        // Every row was written for some value, and a value is paged out by now or will
        // not be by this page-out.
        let rows = self.io_metrics.data_keys_out.load(Ordering::Relaxed) - rows_before;
        self.io_metrics
            .record_discarded(rows.saturating_sub(pending.rows_paged_out()));
        // Nothing looks up a slot from here on. The slots go before the sweep: an arc that a
        // slot still holds is not freed by it.
        drop(awaiting_commit);
        drop_in_parallel(finished).await;
        // Evictions are fire-and-forget. The sweep must run after the core has
        // processed every one, or the arcs of late evictions stay pinned until
        // the next sweep. State requests are FIFO, so any round trip is the barrier.
        let _ = state_handle.pagable_node_counts().await;
        let _ = self.release_after_evictions();
        // The append-only store only changes here; refresh the cached size so the
        // command-end path reports it without a filesystem walk.
        self.refresh_db_size_bytes().await;
        if let Some(e) = worker_error {
            // The worker error is the root cause; the flush failure still decides
            // whether the evicted keys' data committed, so it rides along.
            return Err(match flushed {
                Ok(()) => e,
                Err(flush_error) => e.context(format!(
                    "flush after page-out worker failure also failed: {flush_error:#}"
                )),
            });
        }
        flushed?;
        Ok(())
    }

    /// Drops the resident values a row already backs, one read back or written by an earlier
    /// page-out, without writing anything: those backed when the call starts, not ones that
    /// become backed while it runs. Returns how many values were evicted. Runs alongside
    /// computations: a lookup of an evicted value reads it back.
    pub(crate) async fn evict_backed(&self, state_handle: &CoreStateHandle) -> u64 {
        let snapshot = state_handle.backed_snapshot().await;
        self.evict_candidates(state_handle, snapshot, None, None)
            .await
    }

    async fn evict_candidates(
        &self,
        state_handle: &CoreStateHandle,
        snapshot: BackedSnapshot,
        access_cutoff: Option<AccessCutoff>,
        stop: Option<ReclaimStop>,
    ) -> u64 {
        // In batches of keys, so the core thread stays responsive between them.
        const EVICT_BATCH_KEYS: usize = 1000;
        // Bounded by what was backed at the start. Computations run meanwhile, and values that
        // become backed in the meantime, read back by a computation or written by a page-out, are
        // likely in use. They are left for the next eviction.
        let start = Instant::now();
        self.paging_memory.record_eviction();
        let mut evicted_value_count = 0;
        for keys in snapshot.keys.chunks(EVICT_BATCH_KEYS) {
            if stop.is_some_and(ReclaimStop::should_stop) {
                break;
            }
            evicted_value_count += state_handle
                .evict_backed_batch(keys.to_vec(), snapshot.cutoff, access_cutoff.dupe())
                .await;
        }
        if stop.is_none() || evicted_value_count != 0 {
            let graph_done = Instant::now();
            let (freed_bytes, arcs) = self.release_after_evictions();
            self.paging_memory.record_evicted(0, freed_bytes);
            tracing::info!(
                values_evicted = evicted_value_count,
                graph_us = (graph_done - start).as_micros() as u64,
                cleanup_us = graph_done.elapsed().as_micros() as u64,
                arc_cache_passes = arcs.passes,
                arc_cache_removed = arcs.removed,
                cleanup_freed_bytes = freed_bytes,
                "Evicted backed DICE values",
            );
        }
        evicted_value_count
    }

    pub(crate) async fn evict_backed_on_memory_pressure(
        &self,
        state_handle: &CoreStateHandle,
        key_index: &DiceKeyIndex,
    ) -> u64 {
        match self.eviction_policy {
            EvictionPolicy::OnIdle(_) => return 0,
            EvictionPolicy::OnMemoryPressure(EvictionStrategy::Default) => {
                return self.evict_backed(state_handle).await;
            }
            EvictionPolicy::OnMemoryPressure(EvictionStrategy::Lru) => {}
        }
        self.reclaim(state_handle, key_index, ReclaimOptions::default(), || false)
            .await
    }

    pub(crate) async fn reclaim(
        &self,
        state_handle: &CoreStateHandle,
        key_index: &DiceKeyIndex,
        options: ReclaimOptions,
        cancelled: PageOutCancel,
    ) -> u64 {
        let stop = ReclaimStop {
            started: Instant::now(),
            max_duration: options.max_duration,
            cancelled,
        };
        if !self.tracks_accesses() || options.max_candidates == 0 || stop.should_stop() {
            return 0;
        }
        // Selection and every queued core batch must use the same retention boundary.
        let access_cutoff = key_index
            .access_cutoff()
            .keep_recent_batches(options.keep_recent_batches);
        let mut snapshot = state_handle.backed_snapshot().await;
        if snapshot.keys.is_empty() || stop.should_stop() {
            return 0;
        }
        let selection_cutoff = access_cutoff.dupe();
        let submitted = Instant::now();
        snapshot.keys = tokio::task::spawn_blocking(move || {
            let selection_queue_us = submitted.elapsed().as_micros() as u64;
            let mut keys = snapshot.keys;
            let inspect = options.max_candidates.min(keys.len());
            if inspect < keys.len() {
                let offset = selection_cutoff.candidate_offset(inspect, keys.len());
                keys = keys[offset..]
                    .iter()
                    .chain(&keys[..offset])
                    .take(inspect)
                    .copied()
                    .collect();
            }
            let eligible =
                order_by_generation(&mut keys, |key| selection_cutoff.age_bucket(key), stop)
                    .unwrap_or(0);
            keys.truncate(eligible);
            tracing::info!(
                selection_queue_us,
                selected_keys = keys.len(),
                "Selected LRU eviction candidates",
            );
            keys
        })
        .await
        .expect("LRU candidate selection panicked");
        if snapshot.keys.is_empty() || stop.should_stop() {
            return 0;
        }
        self.evict_candidates(state_handle, snapshot, Some(access_cutoff), Some(stop))
            .await
    }

    /// Frees what evictions the core has applied left behind: the backend's caches and the
    /// arc-cache bindings whose values the evictions dropped. Records the memory this frees as
    /// offloaded and returns it, with what the arc-cache sweep did.
    fn release_after_evictions(&self) -> (i64, EvictExpiredStats) {
        // Runs on this thread alone, so the window sees only these frees.
        let window = AllocWindow::open();
        self.storage.release_memory();
        let arcs = self.storage.arc_cache().evict_expired();
        let freed_bytes = window.net_freed();
        self.paging_memory.record_offloaded(freed_bytes);
        (freed_bytes, arcs)
    }

    fn page_out_chunk(
        &self,
        items: Vec<(DiceKey, DiceKeyErased, DiceValidValue)>,
        finished: &ArcSerCache,
        state_handle: &CoreStateHandle,
        pending: &PendingEvictions,
        cancelled: PageOutCancel,
    ) -> anyhow::Result<()> {
        // Candidates whose value could not be serialized; marked so they aren't
        // offered as page-out candidates again (until recomputed).
        let mut non_pageable = Vec::new();
        for (dice_key, key_dyn, value) in items {
            // Stop promptly on cancellation; keys not yet processed stay paged
            // in, which is a valid state.
            if cancelled() {
                break;
            }
            if let Some((data_key, serialized_bytes, ticket, rows)) =
                self.page_out_value(&key_dyn, &value, finished)?
            {
                pending.push(
                    dice_key,
                    PageOutResult {
                        serialized_value: value,
                        data_key,
                        serialized_bytes,
                    },
                    ticket,
                    rows,
                );
            } else {
                non_pageable.push((dice_key, value));
            }
        }
        // No flush here: flushing per worker slice makes the effective
        // transaction a few thousand rows, and each commit rewrites every
        // index page it touched - the dominant page-out cost at scale
        // (measurements in the diff summary). Buffers rotate and commit at
        // their byte bound, and `page_out` flushes once at the end.
        if !non_pageable.is_empty() {
            state_handle.mark_non_pageable(non_pageable);
        }
        Ok(())
    }

    /// Writes `value` and everything it references, and returns its key, the size of its
    /// own row, the ticket that covers its rows, and how many rows this wrote. `None` if
    /// the value was not written.
    fn page_out_value(
        &self,
        key_dyn: &DiceKeyErased,
        value: &DiceValidValue,
        finished: &ArcSerCache,
    ) -> anyhow::Result<Option<(DataKey, u64, WriteTicket, usize)>> {
        let storage_context = self.storage.storage_context();
        let mut serializer = SerializerForPaging::new(storage_context);
        let serialize_result = match key_dyn {
            DiceKeyErased::Key(k) => k.pagable_serialize_value(value.as_dyn(), &mut serializer),
            DiceKeyErased::Projection(p) => p
                .proj()
                .pagable_serialize_value(value.as_dyn(), &mut serializer),
        };
        match serialize_result {
            None => {
                tracing::debug!(
                    "Skipping page-out of `{}`: no value serializer",
                    key_dyn.key_type_name()
                );
                Ok(None)
            }
            Some(Err(e)) => Err(e),
            Some(Ok(())) => {
                let (data, arcs) = serializer.finish();
                let serialized_bytes = data.len() as u64;
                match self
                    .storage
                    .page_out_item_ticketed(data, arcs, finished, storage_context)
                {
                    Ok((data_key, ticket, rows)) => {
                        Ok(Some((data_key, serialized_bytes, ticket, rows)))
                    }
                    Err(PageOutError::Failed(e)) => Err(e),
                    Err(PageOutError::AlreadyFailed) => Ok(None),
                }
            }
        }
    }

    /// Rehydrate all paged-out values in parallel, sending rehydrate messages
    /// back to the core state thread.
    pub(crate) async fn page_in(
        &self,
        keys: Vec<(DiceKey, DataKey)>,
        key_index: &DiceKeyIndex,
        state_handle: &CoreStateHandle,
    ) -> anyhow::Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        let num_workers = env_concurrency("BUCK2_DICE_PAGE_IN_WORKERS");
        let worker_size = keys.len().div_ceil(num_workers);

        let handles: Vec<_> = keys
            .chunks(worker_size)
            .map(|worker_chunk| {
                let storage = self.dupe();
                let state_handle = state_handle.dupe();
                let items: Vec<_> = worker_chunk
                    .iter()
                    .map(|(k, dk)| (*k, key_index.get(*k).dupe(), *dk))
                    .collect();
                tokio::spawn(async move {
                    for (dice_key, key_dyn, data_key) in &items {
                        let hydrated = storage.hydrate(key_dyn, *data_key).await?;
                        state_handle.rehydrate(*dice_key, *data_key, hydrated);
                    }
                    Ok::<_, anyhow::Error>(())
                })
            })
            .collect();

        for handle in handles {
            handle.await??;
        }
        Ok(())
    }

    /// Cumulative graph nodes evicted by page-out since daemon start.
    pub(crate) fn paged_out_node_total(&self) -> u64 {
        self.paging_memory.nodes_paged_out()
    }

    pub(crate) fn eviction_snapshot(&self) -> EvictionSnapshot {
        self.paging_memory.eviction_snapshot()
    }

    /// Deserialize the value at `data_key` back into a `DiceValidValue` via `key_dyn`'s
    /// `ValueSerialize`.
    pub(crate) async fn hydrate(
        &self,
        key_dyn: &DiceKeyErased,
        data_key: DataKey,
    ) -> anyhow::Result<HydratedValue> {
        let fetch_start = Instant::now();
        let data = self.storage.fetch_data(&data_key).await?;
        let fetch_us = fetch_start.elapsed().as_micros() as u64;
        let bytes = data.data.len() as u64;

        // pagable_deserialize_value lazily fetches nested `PagableArc` sub-values,
        // so deser_us also covers that nested I/O, not just CPU.
        let deser_start = Instant::now();
        // Rebuilding the value is where page-in adds memory, and it happens on this
        // thread. No await inside the window, so tokio cannot move us to another
        // thread mid-measurement and read a different thread's counters. An arc
        // reused from the cache allocates nothing and so costs nothing here.
        //
        // The handle and deserializer are scoped so they are dropped before the
        // reading: they are freed right after, but only what is still live at the
        // measurement counts as restored, so leaving them alive would charge every
        // page-in for them.
        //
        // A `?` below skips the recording, which is correct: a page-in that fails
        // hands back no value, so whatever it allocated is dropped rather than
        // retained, and charging it would count memory paging is not holding.
        let window = AllocWindow::open();
        let arc = {
            let handle = self.storage_handle();
            let mut deserializer = handle.root_deserializer(data_key, &data);
            match key_dyn {
                DiceKeyErased::Key(k) => k.pagable_deserialize_value(&mut deserializer)?,
                DiceKeyErased::Projection(p) => {
                    p.proj().pagable_deserialize_value(&mut deserializer)?
                }
            }
        };
        let restored_bytes = window.net_allocated();
        self.paging_memory.record_restored(restored_bytes);
        let deser_us = deser_start.elapsed().as_micros() as u64;

        self.page_in_metrics.record(
            key_dyn.key_type_name(),
            PageInSample {
                fetch_us,
                deser_us,
                bytes,
                restored_bytes,
            },
        );

        Ok(HydratedValue {
            value: DiceValidValue::from_arc(arc),
            serialized_bytes: bytes,
        })
    }
}

/// Values page-out workers have serialized, held until every row each one
/// references has committed. Only then is the value evicted, so a row the
/// storage never writes, after a failure, leaves its value resident rather
/// than unreadable.
#[derive(Default)]
struct PendingEvictions {
    /// Where workers push. The evictor moves entries to `waiting` before it
    /// scans them, so a scan of everything waiting never holds up a worker.
    incoming: parking_lot::Mutex<Vec<PendingEviction>>,
    waiting: parking_lot::Mutex<Vec<PendingEviction>>,
    /// The rows written for the values taken so far.
    rows_paged_out: AtomicU64,
}

/// A value, the ticket that covers its rows, and how many rows were written for it.
type PendingEviction = (DiceKey, PageOutResult, WriteTicket, usize);

impl PendingEvictions {
    fn push(&self, key: DiceKey, result: PageOutResult, ticket: WriteTicket, rows: usize) {
        self.incoming.lock().push((key, result, ticket, rows));
    }

    fn rows_paged_out(&self) -> u64 {
        self.rows_paged_out.load(Ordering::Relaxed)
    }

    /// Drops every pending value, leaving them resident.
    fn clear(&self) {
        self.incoming.lock().clear();
        self.waiting.lock().clear();
    }

    /// Removes and returns the values whose rows `frontier` says have committed.
    fn take_covered(&self, frontier: &CommitFrontier) -> Vec<(DiceKey, PageOutResult)> {
        let mut waiting = self.waiting.lock();
        waiting.append(&mut self.incoming.lock());
        waiting
            .extract_if(.., |(_, _, ticket, _)| frontier.covers(ticket))
            .map(|(key, result, _, rows)| {
                self.rows_paged_out
                    .fetch_add(rows as u64, Ordering::Relaxed);
                (key, result)
            })
            .collect()
    }
}

/// Aborts the task it holds when dropped, so the evictor does not outlive a
/// `page_out` that unwinds before stopping it.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Every 250 ms, commits what the storage has committed so far, so memory is released during the
/// page-out rather than at its end. Shutdown waits for the current tick, binding included; it
/// never detaches binding jobs partway through.
struct PageOutEvictor {
    awaiting_commit: AwaitingCommit,
    stopping: watch::Receiver<bool>,
}

impl PageOutEvictor {
    async fn run(mut self) {
        loop {
            tokio::select! {
                biased;
                _ = self.stopping.wait_for(|stopping| *stopping) => return,
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
            }
            self.awaiting_commit
                .commit_covered(&self.awaiting_commit.storage.storage.commit_frontier())
                .await;
        }
    }
}

/// What a page-out has written whose rows the storage has yet to commit: the arcs to bind to
/// their rows, and the values for the core state to back or evict.
#[derive(Clone, Dupe)]
struct AwaitingCommit {
    storage: DiceStorage,
    finished: Arc<ArcSerCache>,
    pending: Arc<PendingEvictions>,
    state_handle: CoreStateHandle,
}

impl AwaitingCommit {
    /// Binds the arcs, then commits the values to the core state, whose rows `frontier` covers.
    async fn commit_covered(&self, frontier: &CommitFrontier) {
        bind_in_parallel(&self.storage.storage, self.finished.take_covered(frontier)).await;
        commit_in_batches(
            &self.state_handle,
            self.pending.take_covered(frontier),
            self.storage.eviction_policy,
        );
    }
}

/// Binds arcs on the blocking pool, many at a time. A page-out writes tens of
/// millions of arc rows; bound one task at a time they took longer to bind
/// than to write.
async fn bind_in_parallel(
    storage: &Arc<dyn PagableStorage>,
    mut covered: Vec<(Box<dyn ArcEraseDyn>, DataKey)>,
) {
    const BIND_CHUNK: usize = 1 << 16;
    let mut tasks = Vec::with_capacity(covered.len().div_ceil(BIND_CHUNK));
    while !covered.is_empty() {
        let chunk = covered.split_off(covered.len().saturating_sub(BIND_CHUNK));
        let storage = storage.dupe();
        tasks.push(tokio::task::spawn_blocking(move || {
            for (arc, key) in chunk {
                storage.associate_arc_with_data_key(&*arc, key);
            }
        }));
    }
    for task in tasks {
        // A chunk only fails by panicking, and a panic in the binding of
        // paged-out arcs is a bug in the page-out, not something to continue from.
        if let Err(e) = task.await
            && e.is_panic()
        {
            std::panic::resume_unwind(e.into_panic());
        }
    }
}

/// Drops the slots of `finished` on a few threads of the blocking pool. Each slot releases its
/// arc's allocation, memory that is cold by then: on one thread the 17 million slots of a
/// `fbcode//buck2:buck2` page-out took 6 s to drop.
async fn drop_in_parallel(finished: Arc<ArcSerCache>) {
    // Few tasks: the allocator caches the last blocks that a thread frees, and a cached block
    // keeps its page resident until the thread exits. With a task per 65536 slots, the daemon
    // had about 0.5 GB more resident memory for the seconds after the page-out.
    const DROP_TASKS: usize = 16;
    // Another holder, if one is left, drops the cache when it lets go.
    let Some(finished) = Arc::into_inner(finished) else {
        return;
    };
    let chunk_size = finished.len().div_ceil(DROP_TASKS).max(1);
    let tasks: Vec<_> = finished
        .into_slot_chunks(chunk_size)
        .into_iter()
        .map(|chunk| tokio::task::spawn_blocking(move || drop(chunk)))
        .collect();
    for task in tasks {
        // A chunk only fails by panicking, in the drop of an arc's contents.
        if let Err(e) = task.await
            && e.is_panic()
        {
            std::panic::resume_unwind(e.into_panic());
        }
    }
}

/// Sends committed rows in batches small enough to keep the core thread responsive.
fn commit_in_batches(
    state_handle: &CoreStateHandle,
    mut keys: Vec<(DiceKey, PageOutResult)>,
    eviction_policy: EvictionPolicy,
) {
    const COMMIT_BATCH_SIZE: usize = 1000;
    while !keys.is_empty() {
        let rest = keys.split_off(keys.len().min(COMMIT_BATCH_SIZE));
        state_handle.commit_page_out(std::mem::replace(&mut keys, rest), eviction_policy);
    }
}

/// Cumulative page-in counters for a `DiceStorage`, broken down by key type.
#[derive(Default)]
struct PageInMetrics {
    // DashMap keyed by `&'static str`: bulk `page_in` records from parallel
    // workers (no global lock), and static keys avoid per-record allocation.
    by_key_type: DashMap<&'static str, PageInKeyTypeMetrics>,
}

/// One page-in's costs, accumulated into [`PageInMetrics`] under its key type.
struct PageInSample {
    fetch_us: u64,
    deser_us: u64,
    bytes: u64,
    restored_bytes: i64,
}

impl PageInMetrics {
    fn record(&self, key_type: &'static str, sample: PageInSample) {
        let mut entry = self.by_key_type.entry(key_type).or_default();
        entry.count += 1;
        entry.fetch_us += sample.fetch_us;
        entry.deser_us += sample.deser_us;
        entry.bytes += sample.bytes;
        entry.restored_bytes += sample.restored_bytes;
    }

    fn snapshot(&self) -> HashMap<&'static str, PageInKeyTypeMetrics> {
        self.by_key_type
            .iter()
            .map(|entry| (*entry.key(), *entry.value()))
            .collect()
    }
}

/// Cumulative DataKey tallies maintained by [`MeteredPagableStorage`]. `bytes_*` sum
/// each DataKey's serialized value payload (`PagableData::data`).
#[derive(Default)]
struct StorageIoMetrics {
    data_keys_out: AtomicU64,
    bytes_out: AtomicU64,
    data_keys_in: AtomicU64,
    bytes_in: AtomicU64,
    data_keys_discarded: AtomicU64,
}

impl StorageIoMetrics {
    fn record_out(&self, bytes: u64) {
        self.data_keys_out.fetch_add(1, Ordering::Relaxed);
        self.bytes_out.fetch_add(bytes, Ordering::Relaxed);
    }

    fn record_in(&self, bytes: u64) {
        self.data_keys_in.fetch_add(1, Ordering::Relaxed);
        self.bytes_in.fetch_add(bytes, Ordering::Relaxed);
    }

    fn record_discarded(&self, data_keys: u64) {
        self.data_keys_discarded
            .fetch_add(data_keys, Ordering::Relaxed);
    }

    fn snapshot(&self) -> StorageIoSnapshot {
        StorageIoSnapshot {
            data_keys_out: self.data_keys_out.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
            data_keys_in: self.data_keys_in.load(Ordering::Relaxed),
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
            data_keys_discarded: self.data_keys_discarded.load(Ordering::Relaxed),
        }
    }
}

/// [`PagableStorage`] decorator tallying every DataKey blob written and read into
/// shared [`StorageIoMetrics`].
///
/// Counting at the trait boundary rather than in `page_out_value` / `hydrate` is what
/// captures nested `PagableArc` sub-values, which route back through here. Arc-cache
/// hits never reach `fetch_data`, so resident values aren't counted as paged in.
struct MeteredPagableStorage {
    inner: Arc<dyn PagableStorage>,
    metrics: Arc<StorageIoMetrics>,
}

impl MeteredPagableStorage {
    fn new(inner: Arc<dyn PagableStorage>, metrics: Arc<StorageIoMetrics>) -> Self {
        Self { inner, metrics }
    }
}

#[async_trait::async_trait]
impl PagableStorage for MeteredPagableStorage {
    fn arc_cache(&self) -> &DeserializedArcCache {
        self.inner.arc_cache()
    }

    fn fetch_data_blocking(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        let data = self.inner.fetch_data_blocking(key)?;
        self.metrics.record_in(data.data.len() as u64);
        Ok(data)
    }

    async fn fetch_data(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        let data = self.inner.fetch_data(key).await?;
        self.metrics.record_in(data.data.len() as u64);
        Ok(data)
    }

    fn schedule_for_paging(&self, arc: Box<dyn ArcEraseDyn>) {
        self.inner.schedule_for_paging(arc)
    }

    fn storage_context(&self) -> &StorageContext {
        self.inner.storage_context()
    }

    fn store_data_ticketed(&self, data: PagableData) -> anyhow::Result<(DataKey, WriteTicket)> {
        let bytes = data.data.len() as u64;
        let stored = self.inner.store_data_ticketed(data)?;
        self.metrics.record_out(bytes);
        Ok(stored)
    }

    fn commit_frontier(&self) -> CommitFrontier {
        self.inner.commit_frontier()
    }

    fn discard_unwritten(&self) {
        self.inner.discard_unwritten()
    }

    fn flush(&self) -> anyhow::Result<()> {
        self.inner.flush()
    }

    fn release_memory(&self) {
        self.inner.release_memory()
    }
}

fn env_concurrency(var: &str) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        })
}

/// Sum the sizes of all files under `path`, recursing into subdirectories. Any
/// read or stat error aborts the walk and is returned to the caller.
fn dir_size_bytes(path: &Path) -> std::io::Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        total += if metadata.is_dir() {
            dir_size_bytes(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(total)
}

#[cfg(test)]
mod cleanup_tests;

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::Arc;
    use std::time::Duration;
    use std::time::Instant;

    use dupe::Dupe;
    use pagable::PagableDeserialize;
    use pagable::PagableSerialize;
    use pagable::storage::data::PagableData;
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;
    use pagable::storage::support::SerializerForPaging;
    use pagable::storage::traits::ArcSerCache;
    use pagable::storage::traits::PagableStorage;
    use pagable::storage::traits::WriteTicket;

    use crate::key::DiceKey;
    use crate::key_index::AccessCutoff;
    use crate::storage::DiceStorage;
    use crate::storage::MeteredPagableStorage;
    use crate::storage::PagableStorageBackend;
    use crate::storage::PageInMetrics;
    use crate::storage::PageInSample;
    use crate::storage::ReclaimStop;
    use crate::storage::StorageIoMetrics;
    use crate::storage::order_by_generation;

    #[test]
    fn generation_partition_preserves_keys_and_excludes_protected_class() {
        for len in [0u32, 1, AccessCutoff::AGE_BUCKETS as u32, 2053] {
            for mode in 0..3 {
                let class = |key: DiceKey| match mode {
                    0 => 0,
                    1 => AccessCutoff::AGE_BUCKETS - 1,
                    _ => (key.index as usize * 17 + 5) % AccessCutoff::AGE_BUCKETS,
                };
                let mut keys: Vec<_> = (0..len).rev().map(|index| DiceKey { index }).collect();
                let reads = Cell::new(0);
                let eligible = order_by_generation(
                    &mut keys,
                    |key| {
                        reads.set(reads.get() + 1);
                        class(key)
                    },
                    ReclaimStop {
                        started: Instant::now(),
                        max_duration: Duration::MAX,
                        cancelled: || false,
                    },
                )
                .unwrap();
                assert_eq!(reads.get(), len, "freeze each access class exactly once");
                assert!(keys.windows(2).all(|pair| class(pair[0]) <= class(pair[1])));
                assert!(
                    keys[..eligible]
                        .iter()
                        .all(|&key| class(key) < AccessCutoff::AGE_BUCKETS - 1)
                );
                assert!(
                    keys[eligible..]
                        .iter()
                        .all(|&key| class(key) == AccessCutoff::AGE_BUCKETS - 1)
                );
                keys.sort_unstable_by_key(|key| key.index);
                assert_eq!(
                    keys.iter().map(|key| key.index).collect::<Vec<_>>(),
                    (0..len).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn page_in_metrics_breakdown() {
        let metrics = PageInMetrics::default();
        metrics.record(
            "A",
            PageInSample {
                fetch_us: 10,
                deser_us: 20,
                bytes: 100,
                restored_bytes: 1000,
            },
        );
        metrics.record(
            "A",
            PageInSample {
                fetch_us: 5,
                deser_us: 5,
                bytes: 50,
                restored_bytes: 500,
            },
        );
        metrics.record(
            "B",
            PageInSample {
                fetch_us: 1,
                deser_us: 2,
                bytes: 3,
                restored_bytes: 30,
            },
        );

        // Snapshot is per-key-type only; "A"'s two records collapse into one
        // entry, and summing across types is the caller's job.
        let snap = metrics.snapshot();
        let a = snap.get("A").expect("A was recorded");
        assert_eq!(
            (a.count, a.fetch_us, a.deser_us, a.bytes, a.restored_bytes),
            (2, 15, 25, 150, 1500)
        );
        let b = snap.get("B").expect("B was recorded");
        assert_eq!(
            (b.count, b.fetch_us, b.deser_us, b.bytes, b.restored_bytes),
            (1, 1, 2, 3, 30)
        );
    }

    #[test]
    fn page_in_metrics_restored_bytes_can_net_negative() {
        // A cached-arc page-in allocates only scratch and can net negative;
        // accumulating as-is lets it cancel instead of biasing totals upward.
        let metrics = PageInMetrics::default();
        metrics.record(
            "A",
            PageInSample {
                fetch_us: 10,
                deser_us: 20,
                bytes: 100,
                restored_bytes: 1000,
            },
        );
        metrics.record(
            "A",
            PageInSample {
                fetch_us: 10,
                deser_us: 20,
                bytes: 100,
                restored_bytes: -1500,
            },
        );

        let snap = metrics.snapshot();
        let a = snap.get("A").expect("A was recorded");
        assert_eq!(
            a.restored_bytes, -500,
            "a negative record must cancel against a positive one"
        );
    }

    /// `DiceStorage` sees its backend only through the metering wrapper, so the
    /// wrapper must pass commit tracking through or every row looks durable at once.
    #[test]
    fn metered_storage_forwards_commit_tracking() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let storage = DiceStorage::open(dir.path(), PagableStorageBackend::Sqlite)?.storage;
        let (_key, ticket) = storage.store_data_ticketed(PagableData {
            data: vec![0u8; 10],
            arcs: vec![],
        })?;
        assert_ne!(ticket, WriteTicket::DURABLE);
        assert!(!storage.commit_frontier().covers(&ticket));
        storage.flush()?;
        assert!(storage.commit_frontier().covers(&ticket));

        let (key, _ticket) = storage.store_data_ticketed(PagableData {
            data: vec![1u8; 10],
            arcs: vec![],
        })?;
        storage.discard_unwritten();
        storage.flush()?;
        assert!(
            storage.fetch_data_blocking(&key).is_err(),
            "the discard must reach the backend"
        );
        Ok(())
    }

    #[test]
    fn metered_storage_counts_datakey_io() {
        let metrics = Arc::new(StorageIoMetrics::default());
        // Kept in scope: dropping it invalidates the backing cache.
        let backing = InMemoryPagableStorage::new();
        let storage = MeteredPagableStorage::new(backing.handle(), metrics.dupe());

        let a = storage
            .store_data(PagableData {
                data: vec![0u8; 10],
                arcs: vec![],
            })
            .unwrap();
        let b = storage
            .store_data(PagableData {
                data: vec![1u8; 20],
                arcs: vec![],
            })
            .unwrap();
        // Only the value payload counts — not the `DataKey` (a 16-byte hash) nor the
        // on-disk arc-key list: this store adds 5, not 5 + 2*16.
        storage
            .store_data(PagableData {
                data: vec![2u8; 5],
                arcs: vec![a, b],
            })
            .unwrap();

        let snap = metrics.snapshot();
        assert_eq!(
            (
                snap.data_keys_out,
                snap.bytes_out,
                snap.data_keys_in,
                snap.bytes_in
            ),
            (3, 35, 0, 0),
            "each store_data counts one DataKey and its value-payload bytes"
        );

        // Reads count the same value-payload bytes, so a value read back nets to
        // zero (`bytes_out - bytes_in`) for that DataKey.
        storage.fetch_data_blocking(&a).unwrap();
        storage.fetch_data_blocking(&b).unwrap();
        let snap = metrics.snapshot();
        assert_eq!(
            (snap.data_keys_in, snap.bytes_in),
            (2, 30),
            "each fetch counts one DataKey and its value-payload bytes"
        );
    }

    /// Nested `PagableArc` sub-values get their own `DataKey` and are counted too —
    /// the property that justifies decorating the trait rather than counting in
    /// `page_out_value` / `hydrate`.
    #[test]
    fn metered_storage_counts_nested_arc_sub_values() -> anyhow::Result<()> {
        const NESTED_LEN: usize = 1000;

        let metrics = Arc::new(StorageIoMetrics::default());
        // Kept in scope: dropping it invalidates the backing cache.
        let backing = InMemoryPagableStorage::new();
        let storage: Arc<dyn PagableStorage> =
            Arc::new(MeteredPagableStorage::new(backing.handle(), metrics.dupe()));

        // Arc-heavy shape: tiny parent blob, payload in the sub-value.
        let nested: Arc<Vec<u8>> = Arc::new(vec![0xAB; NESTED_LEN]);
        let storage_context = storage.storage_context();
        let mut serializer = SerializerForPaging::new(storage_context);
        7u8.pagable_serialize(&mut serializer)?;
        nested.pagable_serialize(&mut serializer)?;
        let (data, arcs) = serializer.finish();
        let parent_key = storage
            .page_out_item(data, arcs, &ArcSerCache::new(), storage_context)
            .map_err(anyhow::Error::from)?;
        storage.flush()?;

        let snap = metrics.snapshot();
        assert_eq!(
            snap.data_keys_out, 2,
            "the parent and its nested arc are separate DataKeys",
        );
        assert!(
            snap.bytes_out > NESTED_LEN as u64,
            "bytes_out {} must cover the nested arc's {NESTED_LEN}-byte payload, \
             which the parent blob does not contain",
            snap.bytes_out,
        );

        // While it is alive, page-in reuses the allocation via the arc cache and
        // never reads the sub-value back.
        drop(nested);

        let root = storage.fetch_data_blocking(&parent_key)?;
        let handle = PagableStorageHandle::new(storage.dupe());
        let mut deserializer = handle.root_deserializer(parent_key, &root);
        assert_eq!(u8::pagable_deserialize(&mut deserializer)?, 7);
        let restored = Arc::<Vec<u8>>::pagable_deserialize(&mut deserializer)?;
        assert_eq!(restored.len(), NESTED_LEN);

        let snap = metrics.snapshot();
        assert_eq!(
            (snap.data_keys_in, snap.bytes_in),
            (snap.data_keys_out, snap.bytes_out),
            "reading the value back nets every DataKey and byte written",
        );
        Ok(())
    }
}
