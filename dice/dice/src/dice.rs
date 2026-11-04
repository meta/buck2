/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::future::Future;
use std::sync::Arc as StdArc;
use std::time::Duration;

use allocative::Allocative;
use dice_core::BranchId;
use dice_core::Version;
use dupe::Dupe;
use pagable::StorageContext;
use pagable::storage::handle::PagableStorageHandle;
use parking_lot::RwLock;

use crate::DiceTransactionUpdater;
use crate::HashMap;
use crate::api::cycles::DetectCycles;
use crate::api::data::DiceData;
use crate::api::user_data::UserComputationData;
use crate::arc::Arc;
use crate::core::state::CoreStateHandle;
use crate::core::state::init_state;
use crate::introspection::graph::GraphIntrospectable;
use crate::key::DiceKey;
use crate::key_index::DiceKeyIndex;
use crate::metrics::EvictionSnapshot;
use crate::metrics::Metrics;
use crate::metrics::PageInKeyTypeMetrics;
use crate::metrics::PagingMemorySnapshot;
use crate::storage::DiceStorage;
use crate::updater::TransactionUpdater;

/// Cancellation check for [`Dice::page_out_cancellable`]: returns `true` when the
/// in-progress page-out should stop promptly. Polled per key, so it must be cheap
/// and lock-free. A capture-free `fn`, so it needs no allocation and is `Copy`ed
/// into each page-out worker.
pub type PageOutCancel = fn() -> bool;

/// How soon after a version's computations stop making progress they are first checked for
/// dependency cycles, unless configured otherwise.
pub(crate) const DEFAULT_CYCLE_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// An incremental computation engine that executes arbitrary computations that
/// maps `Key`s to values.
///
/// A `Dice` begins with no branch: [`Dice::new_root`] makes one, and every updater and
/// transaction runs on a branch its caller was handed.
#[derive(Allocative)]
pub struct Dice {
    pub(crate) key_index: DiceKeyIndex,
    pub(crate) state_handle: CoreStateHandle,
    pub(crate) global_data: StdArc<DiceData>,
    branch_data: StdArc<RwLock<HashMap<BranchId, StdArc<DiceData>>>>,
    pub(crate) pagable_storage: Option<DiceStorage>,
    /// How soon after a version's computations stop making progress they are first checked for
    /// dependency cycles, or `None` to never check. See `epoch::cycles`.
    pub(crate) cycle_check_interval: Option<Duration>,
}

// SAFETY: as for `DiceComputations`.
unsafe impl Send for Dice {}
unsafe impl Sync for Dice {}

fn _assert_send_sync(
    Dice {
        key_index,
        state_handle,
        global_data,
        branch_data,
        pagable_storage,
        cycle_check_interval,
    }: &Dice,
) {
    fn assert<T: Send + Sync>(_: &T) {}
    assert(key_index);
    assert(state_handle);
    assert(global_data);
    assert(branch_data);
    assert(pagable_storage);
    assert(cycle_check_interval);
}

impl Debug for Dice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dice").finish_non_exhaustive()
    }
}

pub struct DiceDataBuilder {
    data: DiceData,
    pagable_storage: Option<DiceStorage>,
    cycle_check_interval: Duration,
}

impl From<DiceData> for DiceDataBuilder {
    fn from(data: DiceData) -> Self {
        Self {
            data,
            ..Self::new()
        }
    }
}

impl DiceDataBuilder {
    pub(crate) fn new() -> Self {
        Self {
            data: DiceData::new(),
            pagable_storage: None,
            cycle_check_interval: DEFAULT_CYCLE_CHECK_INTERVAL,
        }
    }

    pub fn set<K: Send + Sync + 'static>(&mut self, val: K) {
        self.data.set(val);
    }

    /// Configures pagable storage for this DICE instance, enabling
    /// [`Dice::page_out`].
    pub fn set_pagable_storage(&mut self, storage: DiceStorage) {
        self.pagable_storage = Some(storage);
    }

    /// Sets how soon after a version's computations stop making progress they are first checked
    /// for dependency cycles, when [`DetectCycles::Enabled`] is passed to `build`; while they
    /// stay stalled without a cycle, further checks come less and less often, up to 16 times
    /// this. See [`DetectCycles`] for the tradeoff. Defaults to one second.
    pub fn set_cycle_check_interval(&mut self, interval: Duration) {
        self.cycle_check_interval = interval;
    }

    /// Starts the core state thread. Call this from within the tokio runtime that will run the
    /// computations: the thread answers requests through that runtime's workers, which is a good
    /// deal cheaper than answering them itself, as it does when started outside a runtime, so the
    /// runtime has to keep running for as long as the `Dice` is in use.
    pub fn build(self, detect_cycles: DetectCycles) -> StdArc<Dice> {
        let cycle_check_interval = match detect_cycles {
            DetectCycles::Enabled => Some(self.cycle_check_interval),
            DetectCycles::Disabled => None,
        };
        Dice::new(self.data, self.pagable_storage, cycle_check_interval)
    }
}

impl Dice {
    pub(crate) fn new(
        global_data: DiceData,
        pagable_storage: Option<DiceStorage>,
        cycle_check_interval: Option<Duration>,
    ) -> StdArc<Self> {
        let state_handle = init_state(pagable_storage.as_ref().map(DiceStorage::paging_setup));

        let global_data = StdArc::new(global_data);

        StdArc::new(Dice {
            key_index: Default::default(),
            state_handle,
            global_data,
            branch_data: StdArc::new(RwLock::new(HashMap::default())),
            pagable_storage,
            cycle_check_interval,
        })
    }

    /// A `Dice` without pagable storage that detects cycles at the default interval.
    #[cfg(test)]
    pub(crate) fn testing_new(global_data: DiceData) -> StdArc<Self> {
        Self::new(global_data, None, Some(DEFAULT_CYCLE_CHECK_INTERVAL))
    }

    pub fn builder() -> DiceDataBuilder {
        DiceDataBuilder::new()
    }

    /// Starts a branch whose state is `from`'s: its first version sees exactly what `from` sees,
    /// and the commits made to it diverge from there. `from` may be any version of a live
    /// branch, its head included. Dropping the future before it yields the branch does not leak
    /// one.
    pub fn fork(&self, from: Version) -> impl Future<Output = BranchId> + use<> {
        let data = self.data_for_branch(from.branch());
        let branch_data = self.branch_data.dupe();
        let branch = self.state_handle.fork(from);
        async move {
            let branch = branch.await;
            branch_data.write().insert(branch, data);
            branch
        }
    }

    /// Starts a branch with no parent. It starts out with no injected values, and it shares with
    /// the other branches exactly what a certificate written elsewhere lets it: a value whose
    /// premises hold at it, once the same inputs have been injected. Dropping the future before
    /// it yields the branch does not leak one.
    pub fn new_root(&self) -> impl Future<Output = BranchId> + use<> {
        let data = self.global_data.dupe();
        let branch_data = self.branch_data.dupe();
        let branch = self.state_handle.new_root();
        async move {
            let branch = branch.await;
            branch_data.write().insert(branch, data);
            branch
        }
    }

    /// Starts a root whose computations use `data`. Certificates are shared with other roots
    /// under the same rules as [`Self::new_root`]. Any differences in this untracked data that
    /// affect computed values must be represented by keys or tracked dependencies. Dropping
    /// the future before it yields the branch does not leak one.
    pub fn new_root_with_data(&self, data: DiceData) -> impl Future<Output = BranchId> + use<> {
        self.new_root_with_shared_data(StdArc::new(data))
    }

    /// Start an empty root with the same immutable computation data as `from`.
    /// Cached results are shared wherever their tracked dependencies agree.
    pub fn new_root_from(&self, from: BranchId) -> impl Future<Output = BranchId> + use<> {
        self.new_root_with_shared_data(self.data_for_branch(from))
    }

    fn new_root_with_shared_data(
        &self,
        data: StdArc<DiceData>,
    ) -> impl Future<Output = BranchId> + use<> {
        let branch_data = self.branch_data.dupe();
        let branch = self.state_handle.new_root();
        async move {
            let branch = branch.await;
            branch_data.write().insert(branch, data);
            branch
        }
    }

    /// The branch's newest version.
    pub fn head(&self, branch: BranchId) -> impl Future<Output = Version> + use<> {
        self.state_handle.current_version(branch)
    }

    /// Deletes `branch`, releasing every value that only it retained. The branches forked from
    /// it move to its parent, or become roots, and keep seeing exactly what they saw.
    ///
    /// Transactions still running on the branch keep working, but without reuse: nothing
    /// resolves for them any more, injected keys included, and nothing they compute is kept. One
    /// that requests an injected key it has not seen before fails as if the key had never been
    /// injected; callers that want running transactions to finish should wait for the branch to
    /// be idle first ([`Self::wait_for_idle_on`]). Starting a transaction on the branch,
    /// committing to it or forking from it afterwards is an error that panics.
    pub fn delete_branch(&self, branch: BranchId) {
        self.state_handle.delete_branch(branch);
        self.branch_data.write().remove(&branch);
    }

    /// An updater for `branch`: its commits extend that branch, and its transactions run at that
    /// branch's versions.
    pub fn updater(self: &StdArc<Self>, branch: BranchId) -> DiceTransactionUpdater {
        self.updater_with_data(branch, UserComputationData::new())
    }

    /// An updater for `branch` whose transactions carry `extra`.
    pub fn updater_with_data(
        self: &StdArc<Self>,
        branch: BranchId,
        extra: UserComputationData,
    ) -> DiceTransactionUpdater {
        DiceTransactionUpdater(TransactionUpdater::new(
            self.dupe(),
            branch,
            Arc::new(extra),
        ))
    }

    pub(crate) fn data_for_branch(&self, branch: BranchId) -> StdArc<DiceData> {
        self.branch_data
            .read()
            .get(&branch)
            .map(Dupe::dupe)
            // Constructing an updater for a deleted branch remains cheap; the core rejects any
            // attempt to start a transaction on it before this fallback data can be observed.
            .unwrap_or_else(|| self.global_data.dupe())
    }

    pub fn metrics(&self) -> Metrics {
        self.state_handle.metrics()
    }

    /// Cumulative per-key-type page-in counters. Kept separate from
    /// [`Dice::metrics`] as it is heavier; collect only at command boundaries,
    /// not on the per-snapshot path.
    pub fn page_in_metrics(&self) -> HashMap<&'static str, PageInKeyTypeMetrics> {
        self.pagable_storage
            .as_ref()
            .map(|storage| storage.page_in_metrics_snapshot())
            .unwrap_or_default()
    }

    /// Cumulative DataKey page-out / page-in totals since daemon start, or `None` if
    /// pagable storage is not configured — these get logged, so "paging is off" must
    /// not look like "paging moved nothing".
    pub fn storage_io_metrics(&self) -> Option<StorageIoSnapshot> {
        self.pagable_storage
            .as_ref()
            .map(|storage| storage.storage_io_snapshot())
    }

    /// Cumulative memory paging has moved since daemon start, measured from the
    /// allocator rather than inferred from serialized sizes.
    ///
    /// `None` if pagable storage is not configured, or if the allocator counters
    /// cannot be read — without them the totals would sit at zero, which reads as
    /// "paging moved nothing" rather than "nothing was measured".
    pub fn paging_memory_metrics(&self) -> Option<PagingMemorySnapshot> {
        self.pagable_storage
            .as_ref()
            .and_then(|storage| storage.paging_memory_snapshot())
    }

    /// Cumulative graph nodes evicted by page-out since daemon start, or `None`
    /// if pagable storage is not configured.
    pub fn paged_out_node_total(&self) -> Option<u64> {
        self.pagable_storage
            .as_ref()
            .map(|storage| storage.paged_out_node_total())
    }

    /// Cumulative evictions by [`Dice::evict_backed`] since daemon start, or `None` if pagable
    /// storage is not configured.
    pub fn eviction_metrics(&self) -> Option<EvictionSnapshot> {
        self.pagable_storage
            .as_ref()
            .map(|storage| storage.eviction_snapshot())
    }

    /// Last measured on-disk size in bytes of the pagable store, or `None` if
    /// pagable storage is not configured. `Some(Err)` if the measurement walk failed.
    /// Cached at page-out (the store is append-only), so this is cheap.
    pub fn paging_db_size_bytes(&self) -> Option<Result<u64, StdArc<std::io::Error>>> {
        self.pagable_storage
            .as_ref()
            .and_then(|storage| storage.on_disk_size_bytes())
    }

    /// Storage-owned context used by serializer-specific paging telemetry.
    #[doc(hidden)]
    pub fn pagable_storage_context(&self) -> Option<&StorageContext> {
        self.pagable_storage
            .as_ref()
            .map(DiceStorage::storage_context)
    }

    /// Pagable storage handle used by serializer-specific debug telemetry.
    #[doc(hidden)]
    pub fn pagable_storage_handle(&self) -> Option<PagableStorageHandle> {
        self.pagable_storage
            .as_ref()
            .map(DiceStorage::storage_handle)
    }

    /// Current depth of the request queue feeding the dice core-state thread.
    /// Sampled synchronously without enqueueing a request, so this can be
    /// called even when the queue is fully backed up.
    pub fn core_state_queue_depth(&self) -> usize {
        self.state_handle.queue_depth()
    }

    /// Total number of requests the dice core-state thread has processed since
    /// this DICE instance was created. Monotonic; sampling it at intervals
    /// yields the core-state throughput. Like `core_state_queue_depth`, does
    /// not itself enqueue a request.
    pub fn core_state_processed_requests(&self) -> usize {
        self.state_handle.processed_requests()
    }

    pub fn to_introspectable(&self) -> GraphIntrospectable {
        let (graph_introspectable, version_introspectable) = self.state_handle.introspection();
        // a bit subtle, but make sure we introspect the key_index after we get the graphs as
        // there may still be new keys added and running. A snapshot of `key_index` prior to
        // snapshotting the graphs will result in missing keys
        let key_index = self.key_index.introspect();

        GraphIntrospectable {
            graph: graph_introspectable,
            version_data: version_introspectable,
            key_map: key_index,
        }
    }

    /// Waits until dice is idle. See [`Dice::is_idle`] for what that means.
    pub fn wait_for_idle(&self) -> impl Future<Output = ()> + 'static + use<> {
        self.wait_for_idle_in(None)
    }

    /// Waits until `branch` is idle. See [`Dice::is_idle_on`].
    pub fn wait_for_idle_on(&self, branch: BranchId) -> impl Future<Output = ()> + 'static + use<> {
        self.wait_for_idle_in(Some(branch))
    }

    fn wait_for_idle_in(
        &self,
        branch: Option<BranchId>,
    ) -> impl Future<Output = ()> + 'static + use<> {
        let state_handle = self.state_handle.dupe();
        async move {
            loop {
                let status = state_handle.idle_status(branch).await;
                if status.is_idle() {
                    return;
                }
                // Whatever stood in the way is awaited and the question asked again: a draining
                // task may start others before it terminates, and the release of one version
                // says nothing about the others.
                let drained = dice_futures::join::join_all(
                    status
                        .draining
                        .iter()
                        .map(|t| t.as_ref().await_termination()),
                );
                match status.released {
                    Some(released) => {
                        drop(futures::future::join(drained, released).await);
                    }
                    None => {
                        drained.await;
                    }
                }
            }
        }
    }

    /// Whether nothing is happening: no transaction is alive, and no computation started by one
    /// that has since been dropped is still running.
    ///
    /// This is a snapshot: a transaction can be started right after it is taken.
    ///
    /// The state query is enqueued before this method returns, so callers may preserve its ordering
    /// while awaiting the result later.
    pub fn is_idle(&self) -> impl Future<Output = bool> + use<> {
        let status = self.state_handle.idle_status(None);
        async move { status.await.is_idle() }
    }

    /// [`Dice::is_idle`], for the transactions on `branch` alone.
    pub fn is_idle_on(&self, branch: BranchId) -> impl Future<Output = bool> + use<> {
        let status = self.state_handle.idle_status(Some(branch));
        async move { status.await.is_idle() }
    }

    /// Whether a computation started by a transaction that has since been dropped is still
    /// running: the part of not being idle that no live transaction accounts for.
    ///
    /// Like [`Dice::is_idle`], the state query is enqueued before this method returns.
    pub fn is_draining(&self) -> impl Future<Output = bool> + use<> {
        let status = self.state_handle.idle_status(None);
        async move { !status.await.draining.is_empty() }
    }

    /// [`Dice::is_draining`], for the transactions on `branch` alone.
    pub fn is_draining_on(&self, branch: BranchId) -> impl Future<Output = bool> + use<> {
        let status = self.state_handle.idle_status(Some(branch));
        async move { !status.await.draining.is_empty() }
    }

    /// Whether no computation is running anywhere, whatever transactions are alive: what paging
    /// out and in require. Weaker than idle, since a transaction that is alive but computing
    /// nothing does not count, and it will page anything it goes on to read back in.
    async fn no_computation_running(&self) -> bool {
        self.state_handle.running_tasks(None).await.is_empty()
    }

    /// Page out every resident computed value to the configured `DiceStorage`.
    ///
    /// **Caller must ensure DICE is idle** before calling this — typically by awaiting
    /// `wait_for_idle()` first. What is refused is only the stronger case of a computation
    /// running: a transaction that is alive but computing nothing is tolerated, and pages
    /// anything it goes on to read back in.
    ///
    /// No-op if `DiceStorage` was not configured on the builder.
    pub async fn page_out(self: &StdArc<Self>) -> anyhow::Result<()> {
        // Never cancelled.
        self.page_out_cancellable(|| false).await
    }

    /// Like [`Dice::page_out`], but stops promptly once `cancelled` returns true
    /// (checked per key), leaving a partially paged-out graph (a valid state —
    /// paged-out values hydrate back on demand). Used by automatic idle page-out
    /// so it can yield promptly when a new command starts.
    ///
    /// Returns no count. Evictions are applied asynchronously on the state
    /// thread and are not all done when this returns, so read
    /// [`Dice::paged_out_node_total`] once the queue has drained instead.
    pub async fn page_out_cancellable(
        self: &StdArc<Self>,
        cancelled: PageOutCancel,
    ) -> anyhow::Result<()> {
        if !self.no_computation_running().await {
            // A command can race in and start computing even after the caller
            // waited for idle — `wait_for_idle` is not a lasting guarantee. On the
            // idle page-out path that same command also cancels us, so a set
            // `cancelled` flag means this is that benign race: bail quietly. If we
            // aren't cancelled, something called this while a computation runs, which
            // risks paging out a value that's being recomputed — surface it.
            if cancelled() {
                return Ok(());
            }
            return Err(anyhow::anyhow!(
                "Dice::page_out called while a computation is running"
            ));
        }
        let Some(storage) = self.pagable_storage.as_ref() else {
            return Ok(());
        };
        let keys = self.state_handle.keys_to_page_out().await;
        storage
            .page_out(keys, &self.key_index, &self.state_handle, cancelled)
            .await
    }

    /// Drops the resident values whose rows already hold them, one read back or written by an
    /// earlier page-out, without writing anything; for reclaiming memory under pressure. Bounded
    /// by the values backed when the call starts: ones a concurrent computation reads back
    /// meanwhile are left for the next eviction. Returns how many values were evicted.
    ///
    /// No-op if `DiceStorage` was not configured on the builder.
    pub async fn evict_backed(&self) -> u64 {
        match self.pagable_storage.as_ref() {
            Some(storage) => storage.evict_backed(&self.state_handle).await,
            None => 0,
        }
    }

    /// Page in (rehydrate) all paged-out values from the
    /// configured `DiceStorage`, used for debugging.
    ///
    /// **Caller must ensure DICE is idle** before calling this.
    pub async fn page_in(self: &StdArc<Self>) -> anyhow::Result<()> {
        if !self.no_computation_running().await {
            return Err(anyhow::anyhow!(
                "Dice::page_in called while a computation is running; call `wait_for_idle()` first"
            ));
        }
        let Some(storage) = self.pagable_storage.as_ref() else {
            return Err(anyhow::anyhow!("No storage available for page-in"));
        };
        let keys = self.state_handle.paged_out_keys().await?;
        storage
            .page_in(keys, &self.key_index, &self.state_handle)
            .await?;
        Ok(())
    }

    /// Summarize pagable status: counts of resident vs paged-out node values
    /// with a per-key-type breakdown.
    ///
    /// Read-only and does not require DICE to be idle — it reports a consistent
    /// snapshot taken on the core-state thread.
    pub async fn pagable_status(&self) -> PagableStatus {
        let raw = self.state_handle.pagable_status().await;

        // `key_index.get` can't panic here: keys are interned before their node
        // enters the graph, and we snapshot the graph before resolving keys (the
        // dual of `to_introspectable`'s ordering). Don't reorder those two steps.
        let mut counts: HashMap<&'static str, PagableTypeStat> = HashMap::default();
        let mut tally = |keys: &[DiceKey], count: fn(&mut PagableTypeStat) -> &mut usize| {
            for key in keys {
                let key_type = self.key_index.get(*key).key_type_name();
                let stat = counts.entry(key_type).or_insert(PagableTypeStat {
                    key_type,
                    resident: 0,
                    paged_out: 0,
                    backed: 0,
                });
                *count(stat) += 1;
            }
        };
        tally(&raw.resident, |t| &mut t.resident);
        tally(&raw.paged_out, |t| &mut t.paged_out);
        tally(&raw.backed, |t| &mut t.backed);
        let mut by_type: Vec<PagableTypeStat> = counts.into_values().collect();
        // Biggest contributors first, with a name tie-break so output is stable
        // across runs (the underlying HashMap iteration order is not).
        by_type.sort_by(|a, b| {
            (b.resident + b.paged_out)
                .cmp(&(a.resident + a.paged_out))
                .then_with(|| a.key_type.cmp(b.key_type))
        });

        PagableStatus {
            total_nodes: raw.total_nodes,
            resident_count: raw.counts.resident,
            paged_out_count: raw.counts.paged_out,
            candidate_count: raw.counts.candidates,
            backed_count: raw.counts.backed,
            by_type,
        }
    }

    /// Resident, paged-out, and page-out-candidate DICE node counts, read O(1) from
    /// incrementally-maintained core-state tallies (no graph scan). Counts every
    /// occupied node regardless of whether pagable storage is configured.
    pub async fn pagable_node_counts(&self) -> PagableNodeCounts {
        self.state_handle.pagable_node_counts().await
    }
}

/// Cumulative DataKey page-out / page-in totals. `bytes_*` sum each DataKey's
/// serialized value payload, a proxy for allocated in-memory bytes moved (not RSS),
/// so `bytes_out - bytes_in` approximates the bytes currently offloaded.
///
/// This is I/O volume, not freed memory: an arc shared with a still-resident value
/// is serialized without being dropped, so `bytes_out` over-counts under partial
/// page-out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageIoSnapshot {
    pub data_keys_out: u64,
    pub bytes_out: u64,
    pub data_keys_in: u64,
    pub bytes_in: u64,
}

impl StorageIoSnapshot {
    /// The I/O between `earlier` and this snapshot. Saturating: counters only grow
    /// within one `DiceStorage`, so a regression means mismatched snapshots.
    pub fn since(self, earlier: StorageIoSnapshot) -> StorageIoSnapshot {
        StorageIoSnapshot {
            data_keys_out: self.data_keys_out.saturating_sub(earlier.data_keys_out),
            bytes_out: self.bytes_out.saturating_sub(earlier.bytes_out),
            data_keys_in: self.data_keys_in.saturating_sub(earlier.data_keys_in),
            bytes_in: self.bytes_in.saturating_sub(earlier.bytes_in),
        }
    }
}

/// Resident, paged-out, page-out-candidate, and backed DICE node counts. `candidates`, the
/// subset of `resident` the next page-out serializes, and `backed`, the subset a row already
/// holds, are disjoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagableNodeCounts {
    pub resident: usize,
    pub paged_out: usize,
    pub candidates: usize,
    pub backed: usize,
    /// Serialized size of the rows holding the `backed` values: what [`Dice::evict_backed`]
    /// would drop, in serialized rather than in-memory bytes, excluding nested `PagableArc`
    /// sub-values. A lower bound: a value that became backed by being proven equal to a
    /// paged-out one, rather than read back, counts as zero.
    pub backed_bytes: u64,
}

/// Summary of how many DICE node values are resident in memory vs paged out to
/// storage, returned by [`Dice::pagable_status`].
pub struct PagableStatus {
    /// All graph nodes, including vacant / in-progress ones. Hence
    /// `total_nodes >= resident_count + paged_out_count`.
    pub total_nodes: usize,
    pub resident_count: usize,
    pub paged_out_count: usize,
    /// Resident nodes the next page-out serializes; a subset of `resident_count`.
    pub candidate_count: usize,
    /// Resident nodes a row already holds; a subset of `resident_count`, disjoint from the
    /// candidates.
    pub backed_count: usize,
    /// Per-key-type counts, sorted by total (resident + paged-out) descending.
    pub by_type: Vec<PagableTypeStat>,
}

/// Resident vs paged-out node counts for a single key type. Part of
/// [`PagableStatus`].
pub struct PagableTypeStat {
    pub key_type: &'static str,
    pub resident: usize,
    pub paged_out: usize,
    /// Resident values a row already holds; a subset of `resident`.
    pub backed: usize,
}

#[cfg(test)]
pub(crate) mod testing {
    use dupe::Dupe;

    use crate::dice::Dice;
    use crate::epoch::evaluator::VersionState;
    use crate::updater::ActiveTransactionGuard;
    use crate::versions::VersionNumber;

    impl Dice {
        pub(crate) async fn testing_shared_ctx(
            &self,
            v: VersionNumber,
        ) -> (VersionState, ActiveTransactionGuard) {
            let guard = ActiveTransactionGuard::new(v, self.state_handle.dupe());
            self.state_handle.ctx_at_version(v, guard).await
        }
    }
}
