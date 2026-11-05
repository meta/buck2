/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! DICE paging telemetry and the page-out mechanism.
//!
//! [`PagingManager`] collects per-command paging telemetry. The rest of this
//! module implements page-out itself (also driven manually by `buck2 debug
//! hydration`, see [`crate::hydration`]) and automatic page-out when the daemon
//! goes idle.
//!
//! Automatic page-out is enabled with `hydration.page_out_on_idle = true`. When
//! enabled, a finishing command schedules a background task (see
//! [`spawn_page_out_on_idle`]) that waits for DICE to go idle and then pages out
//! to reclaim memory — but only when there is something to page out and there is
//! disk headroom (see [`should_page_out_decision`], configurable via
//! `hydration.page_out_min_free_disk_gb` / [`PageOutThresholds`]).
//!
//! Concurrency: automatic page-out deliberately does *not* take the DICE
//! exclusivity lock the explicit command uses, so it never blocks an incoming
//! command. Instead it only starts when no command is active and the daemon is
//! idle, and it is cancelled (see [`Dice::page_out_cancellable`]) the moment a
//! command that contends for the graph appears, so it yields CPU, I/O, and the
//! DICE state thread back to real work. The read-only `status` command does not
//! cancel it — with `--wait` it instead blocks until the page-out finishes. A
//! partially paged-out graph is valid — paged-out values hydrate back on demand.

use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;
use std::time::Instant;

use buck2_common::memory;
use buck2_core::buck2_env;
use buck2_core::soft_error_or_panic;
use buck2_error::ErrorTag;
use buck2_error::conversion::from_any_with_tag;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::with_dispatcher_async;
use buck2_events::metadata;
use buck2_hash::IntentionallyStdHashMap;
use buck2_server_ctx::concurrency::ConcurrencyHandler;
use buck2_util::process_stats::process_stats;
use dice::Dice;
use dice::EvictionSnapshot;
use dice::PagableNodeCounts;
use dice::PageOutCancel;
use dice::StorageIoSnapshot;
use dupe::Dupe;
use parking_lot::Mutex;
use starlark::pagable::starlark_partial_deser_stats;
use starlark::pagable::starlark_serialization_state_retained_bytes;
use tokio::sync::Notify;
use tokio::sync::watch;

use crate::active_commands::is_only_active_command;
use crate::daemon::state::TenantState;
use crate::jemalloc_stats::get_allocator_stats;

/// Command-scoped collector for DICE paging telemetry. Captures a baseline of the
/// cumulative page-in and DataKey I/O counters at construction (command start) and
/// produces the per-command delta on demand (command end).
///
/// Page-out settings are copied into each instance so the manager does not need to retain the
/// command context.
pub(crate) struct PagingManager {
    tenant: Arc<TenantState>,
    /// Resource-pressure thresholds for automatic idle page-out, `Some` iff enabled.
    page_out_on_idle: Option<PageOutThresholds>,
    total_disk_space_bytes: Option<u64>,
    page_in_baseline: IntentionallyStdHashMap<String, buck2_data::DicePageInKeyTypeStats>,
    /// `None` when pagable storage is not configured.
    data_key_io_baseline: Option<StorageIoSnapshot>,
    /// `None` when pagable storage is not configured.
    eviction_baseline: Option<EvictionSnapshot>,
    _eviction_gate: EvictionGateCommand,
}

impl PagingManager {
    pub(crate) fn new(
        tenant: Arc<TenantState>,
        page_out_on_idle: Option<PageOutThresholds>,
        total_disk_space_bytes: Option<u64>,
    ) -> PagingManager {
        let page_in_baseline = page_in_proto_map(&tenant);
        let dice = tenant.dice_manager.unsafe_dice();
        let data_key_io_baseline = dice.storage_io_metrics();
        let eviction_baseline = dice.eviction_metrics();
        let _eviction_gate = tenant.eviction_gate.command_started();
        PagingManager {
            tenant,
            page_out_on_idle,
            total_disk_space_bytes,
            page_in_baseline,
            data_key_io_baseline,
            eviction_baseline,
            _eviction_gate,
        }
    }

    fn summary(
        &self,
        counts: &PagableNodeCounts,
        page_out_started: buck2_data::PageOutStarted,
    ) -> buck2_data::PagingSummary {
        let dice = self.tenant.dice_manager.unsafe_dice();
        // The delta is this command's work, matching the `page_in_*` fields beside
        // it; the cumulative totals are a daemon-wide gauge.
        let cumulative = dice.storage_io_metrics();
        let memory = dice.paging_memory_metrics();
        let delta = cumulative.map(|c| c.since(self.data_key_io_baseline.unwrap_or(c)));
        let evictions = self.command_evictions();
        buck2_data::PagingSummary {
            dice_page_in_by_key_type: compute_page_in_delta(
                &self.page_in_baseline,
                &page_in_proto_map(&self.tenant),
            ),
            paging_db_size_bytes: measured_db_size_bytes(dice.paging_db_size_bytes()),
            resident_node_count: Some(counts.resident as u64),
            paged_out_node_count: Some(counts.paged_out as u64),
            candidate_node_count: Some(counts.candidates as u64),
            page_out_started: Some(page_out_started as i32),
            paging_data_keys_out: delta.map(|d| d.data_keys_out),
            paging_data_key_bytes_out: delta.map(|d| d.bytes_out),
            paging_data_keys_in: delta.map(|d| d.data_keys_in),
            paging_data_key_bytes_in: delta.map(|d| d.bytes_in),
            paging_daemon_data_keys_out: cumulative.map(|c| c.data_keys_out),
            paging_daemon_data_key_bytes_out: cumulative.map(|c| c.bytes_out),
            paging_daemon_data_keys_in: cumulative.map(|c| c.data_keys_in),
            paging_daemon_data_key_bytes_in: cumulative.map(|c| c.bytes_in),
            paging_memory_offloaded_bytes: memory.map(|m| m.bytes_offloaded),
            paging_memory_restored_bytes: memory.map(|m| m.bytes_restored),
            paging_arc_cache_entries: dice
                .pagable_storage_handle()
                .map(|h| h.arc_cache_entries() as u64),
            starlark_partial_deser: starlark_partial_deser_proto(),
            paging_evictions: evictions.map(|e| e.evictions),
            paging_evicted_count: evictions.map(|e| e.values_evicted),
            paging_evicted_bytes: evictions.and_then(|e| e.bytes_evicted),
            paging_evictable_bytes: Some(counts.backed_bytes),
        }
    }

    /// Memory-pressure evictions since this command started, or `None` if pagable storage is not
    /// configured.
    fn command_evictions(&self) -> Option<EvictionSnapshot> {
        self.tenant
            .dice_manager
            .unsafe_dice()
            .eviction_metrics()
            .map(|e| e.since(self.eviction_baseline.unwrap_or(e)))
    }

    /// Called at command end: schedule a background idle page-out (if
    /// `triggers_idle_page_out`) and emit this command's paging telemetry. A command that evicted
    /// always gets a `buck2_page_outs` row: the idle page-out's if one started, else its own.
    pub(crate) async fn maybe_trigger_page_out_on_idle(
        &self,
        dispatcher: &EventDispatcher,
        command_end_snapshot: &buck2_data::Snapshot,
        triggers_idle_page_out: bool,
    ) {
        // Read the node tally once and share it: the paging telemetry and the page-out
        // candidates gate both need it.
        let counts = self
            .tenant
            .dice_manager
            .unsafe_dice()
            .pagable_node_counts()
            .await;
        let free_disk_bytes = free_disk_space_bytes(
            command_end_snapshot.used_disk_space_bytes,
            self.total_disk_space_bytes,
        );
        let command_evictions = self.command_evictions();
        let page_out_started = if triggers_idle_page_out {
            spawn_page_out_on_idle(
                self.page_out_on_idle,
                self.tenant.dupe(),
                dispatcher.dupe(),
                free_disk_bytes,
                counts.candidates,
                command_evictions,
            )
            .await
        } else {
            buck2_data::PageOutStarted::CommandOptedOut
        };
        if page_out_started != buck2_data::PageOutStarted::Started
            && let Some(evictions) = command_evictions
            && evictions.evictions > 0
        {
            dispatcher.instant_event(eviction_only_page_out_summary(
                self.tenant.dice_manager.unsafe_dice(),
                dispatcher,
                page_out_started,
                evictions,
                counts.backed_bytes,
            ));
        }
        dispatcher.instant_event(self.summary(&counts, page_out_started));
    }
}

/// Turn the cached DB-size measurement into a reportable value, emitting a
/// `soft_error` (and reporting nothing) when the last measurement walk failed,
/// rather than silently dropping the failure.
fn measured_db_size_bytes(size: Option<Result<u64, Arc<std::io::Error>>>) -> Option<u64> {
    match size {
        None => None,
        Some(Ok(bytes)) => Some(bytes),
        Some(Err(e)) => {
            soft_error_or_panic!(
                "paging_db_size_measurement_failed",
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "Failed to measure pagable DB size: {e}"
                )
            );
            None
        }
    }
}

/// Daemon-lifetime Starlark partial-deserialization counters, as proto stats.
fn starlark_partial_deser_proto() -> Option<buck2_data::StarlarkPartialDeserStats> {
    let s = starlark_partial_deser_stats()?;
    Some(buck2_data::StarlarkPartialDeserStats {
        heaps_loaded: s.heaps_loaded,
        heap_retained_blob_bytes: s.heap_retained_blob_bytes,
        heaps_with_metadata: s.heaps_with_metadata,
        used_heap_retained_blob_bytes: s.used_heap_retained_blob_bytes,
        heap_value_serialized_bytes: s.heap_value_serialized_bytes,
        heap_values: s.heap_values,
        heap_value_alloc_bytes: s.heap_value_alloc_bytes,
        claimed_values: s.claimed_values,
        claimed_alloc_bytes: s.claimed_alloc_bytes,
    })
}

/// Cumulative per-key-type page-in counters, as proto stats.
fn page_in_proto_map(
    tenant: &TenantState,
) -> IntentionallyStdHashMap<String, buck2_data::DicePageInKeyTypeStats> {
    tenant
        .dice_manager
        .unsafe_dice()
        .page_in_metrics()
        .iter()
        .map(|(&key_type, stats)| {
            (
                String::from(key_type),
                buck2_data::DicePageInKeyTypeStats {
                    count: stats.count,
                    fetch_us: stats.fetch_us,
                    deser_us: stats.deser_us,
                    bytes: stats.bytes,
                    restored_bytes: stats.restored_bytes,
                },
            )
        })
        .collect()
}

/// Per-key-type delta of the cumulative page-in counters between the command's
/// start (`baseline`) and now (`current`).
fn compute_page_in_delta(
    baseline: &IntentionallyStdHashMap<String, buck2_data::DicePageInKeyTypeStats>,
    current: &IntentionallyStdHashMap<String, buck2_data::DicePageInKeyTypeStats>,
) -> IntentionallyStdHashMap<String, buck2_data::DicePageInKeyTypeStats> {
    current
        .iter()
        .filter_map(|(key_type, c)| {
            let base = baseline.get(key_type);
            // saturating_sub guards against an (unexpected) counter regression,
            // e.g. a daemon restart mid-command resetting the cumulatives.
            let delta = buck2_data::DicePageInKeyTypeStats {
                count: c.count.saturating_sub(base.map_or(0, |b| b.count)),
                fetch_us: c.fetch_us.saturating_sub(base.map_or(0, |b| b.fetch_us)),
                deser_us: c.deser_us.saturating_sub(base.map_or(0, |b| b.deser_us)),
                bytes: c.bytes.saturating_sub(base.map_or(0, |b| b.bytes)),
                restored_bytes: c
                    .restored_bytes
                    .saturating_sub(base.map_or(0, |b| b.restored_bytes)),
            };
            (delta.count > 0).then(|| (key_type.clone(), delta))
        })
        .collect()
}

/// Page DICE node values out to the configured on-disk storage, then return the
/// freed memory to the OS. `cancelled` lets automatic idle page-out stop promptly
/// when a command arrives; pass `|| false` for an uninterruptible page-out.
pub(crate) async fn page_out(
    dice: &Arc<Dice>,
    cancelled: PageOutCancel,
) -> buck2_error::Result<()> {
    if buck2_env!("BUCK2_TEST_FAIL_PAGE_OUT", bool, applicability = testing).unwrap() {
        return Err(buck2_error::buck2_error!(
            ErrorTag::TestOnly,
            "Injected page-out failure (BUCK2_TEST_FAIL_PAGE_OUT)"
        ));
    }
    dice.page_out_cancellable(cancelled)
        .await
        .map_err(|e| from_any_with_tag(e, ErrorTag::Environment))?;

    // Waiting for metrics drains the DICE state queue, ensuring evictions have
    // processed before we purge.
    let _ = dice.metrics();
    memory::purge_jemalloc()?;
    Ok(())
}

/// Lock-free state of the background idle page-out, doubling as the single-flight
/// guard and the cancel flag. `IDLE -> RUNNING` when a page-out starts (only `IDLE`
/// may start one, so at most one runs), `RUNNING -> CANCELLED` when a command asks
/// it to stop, back to `IDLE` when it finishes. The detached page-out task polls
/// [`page_out_cancelled`] lock-free while [`cancel_active_page_out`] transitions it,
/// so one atomic replaces a mutex-guarded slot plus a separate `Arc<AtomicBool>`.
static PAGE_OUT: AtomicU8 = AtomicU8::new(IDLE);
const IDLE: u8 = 0;
const RUNNING: u8 = 1;
const CANCELLED: u8 = 2;

/// Notified when an idle page-out finishes, so `status --wait` can await it
/// instead of polling.
static PAGE_OUT_DONE: LazyLock<Notify> = LazyLock::new(Notify::new);

/// Set when an idle page-out returns an error. Once set, no further idle
/// page-outs are scheduled for the rest of this daemon's lifetime.
/// Paging may be in a bad state, this avoids wasting work if we hit the same error
/// and/or avoids page out values that will later cause page in failures.
/// Cleared only on daemon restart.
static PAGE_OUT_FAILED: AtomicBool = AtomicBool::new(false);

/// Whether a background idle page-out is running, for `buck2 debug hydration
/// status`. A manual `page-out` isn't tracked: it holds the exclusive command
/// lock, so a concurrent `status` blocks behind it and never observes it mid-run.
pub(crate) fn page_out_in_progress() -> bool {
    PAGE_OUT.load(Ordering::Relaxed) != IDLE
}

/// Whether the running idle page-out has been asked to cancel. Passed to
/// [`Dice::page_out_cancellable`] as its cancel check (polled per key).
fn page_out_cancelled() -> bool {
    PAGE_OUT.load(Ordering::Relaxed) == CANCELLED
}

/// Block until no idle page-out is in progress, for `status --wait`.
pub(crate) async fn wait_for_idle_page_out() {
    loop {
        let notified = PAGE_OUT_DONE.notified();
        tokio::pin!(notified);
        // Register as a waiter before the check so a page-out that finishes in the
        // gap still wakes us.
        notified.as_mut().enable();
        if !page_out_in_progress() {
            return;
        }
        notified.await;
    }
}

/// Cancel the in-progress idle page-out, if any. Called when a command that
/// contends for the graph starts (from `run_streaming`, gated by
/// `triggers_idle_page_out`), and by the manual page-out / page-in subcommands.
pub(crate) fn cancel_active_page_out() {
    let _ = PAGE_OUT.compare_exchange(RUNNING, CANCELLED, Ordering::Relaxed, Ordering::Relaxed);
}

/// Cancel the in-progress idle page-out, if any, and wait for it to finish.
pub(crate) async fn stop_idle_page_out() {
    cancel_active_page_out();
    wait_for_idle_page_out().await;
}

/// RAII single-flight guard; while held, [`PAGE_OUT`] is `RUNNING`/`CANCELLED`, so
/// no second page-out can start.
struct PageOutGuard;

impl PageOutGuard {
    /// Returns `None` if a page-out is already running (state isn't `IDLE`). This is
    /// expected, not an error: a cancelled page-out keeps running until it observes
    /// the flag, so a new one must not start and race it on the same graph.
    fn acquire() -> Option<Self> {
        // Dropping a guard releases PAGE_OUT, so only construct one on success.
        PAGE_OUT
            .compare_exchange(IDLE, RUNNING, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
            .then(|| PageOutGuard)
    }
}

impl Drop for PageOutGuard {
    fn drop(&mut self) {
        PAGE_OUT.store(IDLE, Ordering::Release);
        PAGE_OUT_DONE.notify_waiters();
    }
}

/// Resource thresholds gating idle page-out. GiB (not bytes) so
/// `DaemonStartupOptions` stays `Eq` (no floats).
#[derive(Clone, Copy, allocative::Allocative)]
pub(crate) struct PageOutThresholds {
    /// Page out only when at least this many GiB of disk are free to write to.
    pub(crate) min_free_disk_gb: u64,
}

/// Spawn a background idle page-out if it should run, returning the outcome:
/// [`PageOutStarted::Started`] if one was scheduled (not whether it succeeds), else the
/// reason it wasn't — disabled, not enough disk headroom, another command active, one
/// already running, or nothing to page out. When commands overlap, only the last to
/// finish still sees itself as the sole active command, so only it starts one.
pub(crate) async fn spawn_page_out_on_idle(
    thresholds: Option<PageOutThresholds>,
    tenant: Arc<TenantState>,
    dispatcher: EventDispatcher,
    free_disk_bytes: Option<u64>,
    pagable_candidates: usize,
    command_evictions: Option<EvictionSnapshot>,
) -> buck2_data::PageOutStarted {
    use buck2_data::PageOutStarted;

    let Some(thresholds) = thresholds else {
        return PageOutStarted::Disabled;
    };

    if PAGE_OUT_FAILED.load(Ordering::Relaxed) {
        return PageOutStarted::DisabledAfterError;
    }

    if !is_only_active_command(dispatcher.trace_id()) {
        return PageOutStarted::OtherCommandsActive;
    }

    if let Some(reason) = should_page_out_decision(free_disk_bytes, thresholds) {
        return reason;
    }

    let Some(guard) = PageOutGuard::acquire() else {
        return PageOutStarted::AlreadyRunning;
    };

    // Re-check under the guard: a command that started in the window above (which
    // wouldn't have cancelled us) mustn't slip through. Also bail if there is nothing
    // to page out.
    if !is_only_active_command(dispatcher.trace_id()) {
        return PageOutStarted::OtherCommandsActive;
    }
    if pagable_candidates == 0 {
        return PageOutStarted::NothingToPageOut;
    }

    let context_dispatcher = dispatcher.clone();
    tokio::spawn(with_dispatcher_async(context_dispatcher, async move {
        let _tenant = tenant;
        if let Err(e) = page_out_on_idle(
            guard,
            _tenant.dice_manager.dupe(),
            dispatcher,
            command_evictions,
        )
        .await
        {
            soft_error_or_panic!(
                "page_out_on_idle_failed",
                e.context("Automatic page-out on idle failed")
            );
        }
    }));
    PageOutStarted::Started
}

/// Free disk space (bytes) on `buck-out` (where paged-out values are written),
/// derived from telemetry the command already collected rather than a fresh stat:
/// `total` disk (captured from `SystemInfo` at command start) minus `used` disk (from
/// the command-end `Snapshot`). `None` when either input is unavailable.
fn free_disk_space_bytes(used: Option<u64>, total: Option<u64>) -> Option<u64> {
    Some(total?.saturating_sub(used?))
}

/// The idle page-out disk gate: `None` when there is headroom (`free_disk_bytes` at or
/// above `min_free_disk_gb`), else the reason there isn't —
/// [`PageOutStarted::InsufficientDiskHeadroom`], or [`PageOutStarted::FreeDiskUnknown`]
/// when `free_disk_bytes` is `None` (total or used disk unavailable).
fn should_page_out_decision(
    free_disk_bytes: Option<u64>,
    thresholds: PageOutThresholds,
) -> Option<buck2_data::PageOutStarted> {
    let min_free_disk_bytes = thresholds
        .min_free_disk_gb
        .saturating_mul(1024 * 1024 * 1024);
    match free_disk_bytes {
        Some(free) if free >= min_free_disk_bytes => None,
        Some(free) => {
            tracing::debug!("Skipping page-out on idle: only {free} bytes of disk free");
            Some(buck2_data::PageOutStarted::InsufficientDiskHeadroom)
        }
        None => Some(buck2_data::PageOutStarted::FreeDiskUnknown),
    }
}

async fn page_out_on_idle(
    _guard: PageOutGuard,
    dice_manager: Arc<ConcurrencyHandler>,
    dispatcher: EventDispatcher,
    command_evictions: Option<EvictionSnapshot>,
) -> buck2_error::Result<()> {
    let dice = dice_manager.unsafe_dice().dupe();

    // Let the residual DICE tasks from the command that triggered this page-out
    // drain first.
    dice.wait_for_idle().await;

    // A command may have arrived (and cancelled us) while we waited for idle. That's
    // rare, so don't check here — `page_out` observes the flag and stops promptly.
    tracing::info!("Daemon is idle; paging DICE out to reclaim memory");
    let (result, mut summary) = page_out_measured(&dice, page_out_cancelled, &dispatcher).await;
    summary.page_out_started = Some(buck2_data::PageOutStarted::Started as i32);
    set_command_evictions(&mut summary, command_evictions);
    if result.is_err() {
        // Set before this function returns (dropping the single-flight guard) so a
        // `status --wait` that unblocks on guard release already sees the flag.
        PAGE_OUT_FAILED.store(true, Ordering::Relaxed);
    }
    // A "late" event on the finalized triggering command's trace: its
    // per-command channel is gone, but the dispatcher tees to the daemon
    // Scribe sink, so this still reaches `buck2_page_outs` with the command's
    // trace id. The manual command deliberately leaves that table alone.
    dispatcher.instant_event(summary);
    result.map(|_| ())
}

/// Page out, measuring memory and DataKey I/O around it.
///
/// Shared with `buck2 debug hydration page-out` so both paths report the same
/// measurements. `cancelled` both drives the page-out and fills the summary
/// field. The `Result` is for control flow; the failure is also in the
/// summary's `error`.
pub(crate) async fn page_out_measured(
    dice: &Arc<Dice>,
    cancelled: PageOutCancel,
    dispatcher: &EventDispatcher,
) -> (buck2_error::Result<()>, buck2_data::PageOutSummary) {
    let (resident_bytes_before, allocated_bytes_before, db_size_bytes_before) =
        page_out_memory_snapshot(dice);
    let io_before = dice.storage_io_metrics();
    let memory_before = dice.paging_memory_metrics();
    let nodes_before = dice.paged_out_node_total();
    let evictions_before = dice.eviction_metrics();
    let start = Instant::now();
    let result = page_out(dice, cancelled).await;
    let duration_ms = (Instant::now() - start).as_millis() as u64;
    let (resident_bytes_after, allocated_bytes_after, db_size_bytes_after) =
        page_out_memory_snapshot(dice);
    let starlark_serialization_state_bytes_after =
        dice.pagable_storage_context().and_then(|storage| {
            u64::try_from(starlark_serialization_state_retained_bytes(storage)).ok()
        });
    let daemon_io = dice.storage_io_metrics();
    let memory = dice.paging_memory_metrics();
    let io_delta = daemon_io
        .zip(io_before)
        .map(|(after, before)| after.since(before));
    let memory_delta = memory
        .zip(memory_before)
        .map(|(after, before)| after.since(before));
    let evictable_bytes = dice
        .pagable_storage_handle()
        .is_some()
        .then_some(dice.pagable_node_counts().await.backed_bytes);
    let evictions = dice.eviction_metrics();
    let evictions_delta = evictions
        .zip(evictions_before)
        .map(|(after, before)| after.since(before));
    let error = match &result {
        Ok(()) => None,
        Err(e) => Some(e.into()),
    };
    // Read after `page_out` has drained the state queue, so every eviction it
    // queued has been applied. A daemon-lifetime total, so differenced like the
    // memory counters beside it.
    let paged_out_count = dice
        .paged_out_node_total()
        .zip(nodes_before)
        .map_or(0, |(after, before)| after.saturating_sub(before));
    let summary = buck2_data::PageOutSummary {
        metadata: metadata::collect(dispatcher.daemon_id()),
        command_uuid: Some(dispatcher.trace_id().to_string()),
        paged_out_count,
        duration_ms,
        cancelled: cancelled(),
        error,
        resident_bytes_before,
        resident_bytes_after,
        allocated_bytes_before,
        allocated_bytes_after,
        db_size_bytes_before,
        db_size_bytes_after,
        starlark_serialization_state_bytes_after,
        daemon_data_keys_out: daemon_io.map(|c| c.data_keys_out),
        daemon_data_key_bytes_out: daemon_io.map(|c| c.bytes_out),
        daemon_data_keys_in: daemon_io.map(|c| c.data_keys_in),
        daemon_data_key_bytes_in: daemon_io.map(|c| c.bytes_in),
        data_keys_out: io_delta.map(|d| d.data_keys_out),
        data_key_bytes_out: io_delta.map(|d| d.bytes_out),
        data_keys_in: io_delta.map(|d| d.data_keys_in),
        data_key_bytes_in: io_delta.map(|d| d.bytes_in),
        daemon_memory_offloaded_bytes: memory.map(|m| m.bytes_offloaded),
        daemon_memory_restored_bytes: memory.map(|m| m.bytes_restored),
        memory_offloaded_bytes: memory_delta.map(|d| d.bytes_offloaded),
        memory_restored_bytes: memory_delta.map(|d| d.bytes_restored),
        daemon_arc_cache_entries: dice
            .pagable_storage_handle()
            .map(|h| h.arc_cache_entries() as u64),
        evicted_count: evictions_delta.map(|d| d.values_evicted),
        evicted_bytes: evictions_delta.and_then(|d| d.bytes_evicted),
        daemon_evictions: evictions.map(|e| e.evictions),
        daemon_evicted_count: evictions.map(|e| e.values_evicted),
        daemon_evicted_bytes: evictions.and_then(|e| e.bytes_evicted),
        evictable_bytes,
        page_out_started: None,
        command_evictions: None,
        command_evicted_count: None,
        command_evicted_bytes: None,
    };
    (result, summary)
}

/// The `buck2_page_outs` row for a command that evicted backed values but started no idle
/// page-out, so that every command's evictions reach the table. `page_out_started` says why no
/// page-out ran; the page-out fields are left unset.
fn eviction_only_page_out_summary(
    dice: &Arc<Dice>,
    dispatcher: &EventDispatcher,
    page_out_started: buck2_data::PageOutStarted,
    command_evictions: EvictionSnapshot,
    evictable_bytes: u64,
) -> buck2_data::PageOutSummary {
    let daemon = dice.eviction_metrics();
    let mut summary = buck2_data::PageOutSummary {
        metadata: metadata::collect(dispatcher.daemon_id()),
        command_uuid: Some(dispatcher.trace_id().to_string()),
        daemon_evictions: daemon.map(|e| e.evictions),
        daemon_evicted_count: daemon.map(|e| e.values_evicted),
        daemon_evicted_bytes: daemon.and_then(|e| e.bytes_evicted),
        evictable_bytes: Some(evictable_bytes),
        page_out_started: Some(page_out_started as i32),
        ..Default::default()
    };
    set_command_evictions(&mut summary, Some(command_evictions));
    summary
}

fn set_command_evictions(
    summary: &mut buck2_data::PageOutSummary,
    command_evictions: Option<EvictionSnapshot>,
) {
    summary.command_evictions = command_evictions.map(|e| e.evictions);
    summary.command_evicted_count = command_evictions.map(|e| e.values_evicted);
    summary.command_evicted_bytes = command_evictions.and_then(|e| e.bytes_evicted);
}

/// Limits a tenant's memory-pressure evictions to one per command. While a command runs, an onset
/// evicts only if no eviction has run since the most recent command started. With no command
/// running, every onset evicts.
#[derive(Default)]
pub(crate) struct EvictionGate {
    state: Mutex<EvictionGateState>,
}

#[derive(Default)]
struct EvictionGateState {
    active_commands: usize,
    evicted_since_command_start: bool,
}

impl EvictionGate {
    /// Registers a running command until the returned guard is dropped, and allows one more
    /// eviction.
    pub(crate) fn command_started(self: &Arc<Self>) -> EvictionGateCommand {
        let mut state = self.state.lock();
        state.active_commands += 1;
        state.evicted_since_command_start = false;
        EvictionGateCommand(self.dupe())
    }

    /// Whether an onset of memory pressure should evict now. Claims the running commands'
    /// eviction when it says yes.
    fn try_evict(&self) -> bool {
        let mut state = self.state.lock();
        if state.active_commands > 0 && state.evicted_since_command_start {
            return false;
        }
        state.evicted_since_command_start = true;
        true
    }
}

/// A running command, registered with an [`EvictionGate`].
pub(crate) struct EvictionGateCommand(Arc<EvictionGate>);

impl Drop for EvictionGateCommand {
    fn drop(&mut self) {
        self.0.state.lock().active_commands -= 1;
    }
}

/// Evicts an engine's backed DICE values, those a row already holds, when resource control starts
/// seeing memory pressure (the default `hydration.eviction_policy`, `on_memory_pressure`, under
/// which a page-out evicts nothing and these are the only evictions), at most once per command
/// (see [`EvictionGate`]). Only the onset triggers it. Ends with the engine or the tracker.
pub(crate) fn spawn_evict_on_memory_pressure(
    dice: Weak<Dice>,
    gate: Arc<EvictionGate>,
    mut pressure: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        while pressure.changed().await.is_ok() {
            if !*pressure.borrow_and_update() {
                continue;
            }
            let Some(dice) = dice.upgrade() else {
                return;
            };
            if !gate.try_evict() {
                tracing::debug!("Skipping eviction: this command has already evicted once");
                continue;
            }
            evict_backed_measured(&dice).await;
        }
    });
}

/// Evicts backed values and returns the freed memory to the OS, logging what it moved. Returns
/// how many values were evicted.
pub(crate) async fn evict_backed_measured(dice: &Arc<Dice>) -> u64 {
    let (resident_before, allocated_before, _) = page_out_memory_snapshot(dice);
    let start = Instant::now();
    let evicted = dice.evict_backed().await;
    if let Err(e) = memory::purge_jemalloc() {
        soft_error_or_panic!("jemalloc_purge_failed", e);
    }
    let (resident_after, allocated_after, _) = page_out_memory_snapshot(dice);
    tracing::info!(
        "Evicted {evicted} backed DICE values in {:?}; \
         RSS {resident_before:?} -> {resident_after:?} bytes, \
         allocated {allocated_before:?} -> {allocated_after:?} bytes",
        start.elapsed(),
    );
    evicted
}

/// `(resident RSS, jemalloc allocated, pagable DB on-disk size)` in bytes, each
/// `None` when unavailable: no RSS on this platform, not built with jemalloc, or no
/// pagable storage configured. Reads `/proc` and the store directory, so it is only
/// called around a page-out, never on the hot path.
fn page_out_memory_snapshot(dice: &Arc<Dice>) -> (Option<u64>, Option<u64>, Option<u64>) {
    let resident = process_stats().rss_bytes;
    let allocated = get_allocator_stats().ok().and_then(|s| s.bytes_allocated);
    // A measurement error here is surfaced via the command-end `PagingSummary`; for
    // these before/after metrics just use the size when available.
    let db_size = dice.paging_db_size_bytes().and_then(|r| r.ok());
    (resident, allocated, db_size)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use buck2_hash::IntentionallyStdHashMap;

    use super::EvictionGate;
    use super::PageOutGuard;
    use super::PageOutThresholds;
    use super::cancel_active_page_out;
    use super::compute_page_in_delta;
    use super::page_out_cancelled;
    use super::page_out_in_progress;
    use super::should_page_out_decision;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn failed_page_out_acquire_preserves_active_guard() {
        let guard = PageOutGuard::acquire().expect("initial page-out should acquire the guard");
        for cancelled in [false, true] {
            if cancelled {
                cancel_active_page_out();
            }
            for _ in 0..2 {
                assert!(PageOutGuard::acquire().is_none());
                assert!(page_out_in_progress());
                assert_eq!(page_out_cancelled(), cancelled);
            }
        }

        drop(guard);
        assert!(!page_out_in_progress());

        let next = PageOutGuard::acquire().expect("guard should be released after page-out");
        assert!(!page_out_cancelled());
        drop(next);
        assert!(!page_out_in_progress());
    }

    #[test]
    fn eviction_gate_allows_one_eviction_per_command() {
        let gate = Arc::new(EvictionGate::default());
        assert!(
            gate.try_evict(),
            "with no command running, every onset evicts"
        );
        assert!(
            gate.try_evict(),
            "with no command running, every onset evicts"
        );

        let first = gate.command_started();
        assert!(gate.try_evict(), "a running command gets one eviction");
        assert!(!gate.try_evict(), "and only one");

        let second = gate.command_started();
        assert!(gate.try_evict(), "a command starting allows one more");
        assert!(!gate.try_evict());

        drop(second);
        assert!(
            !gate.try_evict(),
            "a command finishing does not reset the running one's allowance"
        );
        drop(first);
        assert!(
            gate.try_evict(),
            "with no command running, every onset evicts"
        );
    }

    #[test]
    fn delta_subtracts_baseline_and_drops_unchanged() {
        let stat =
            |count, fetch_us, deser_us, bytes, restored_bytes| buck2_data::DicePageInKeyTypeStats {
                count,
                fetch_us,
                deser_us,
                bytes,
                restored_bytes,
            };

        // Baseline cumulatives include page-ins from earlier commands on this
        // daemon, so they must be subtracted out.
        let mut baseline = IntentionallyStdHashMap::default();
        baseline.insert("A".to_owned(), stat(10, 100, 200, 1000, 10000));
        baseline.insert("C".to_owned(), stat(5, 50, 50, 500, 5000));

        let mut current = IntentionallyStdHashMap::default();
        current.insert("A".to_owned(), stat(12, 130, 260, 1300, 13000)); // +2 this command
        current.insert("B".to_owned(), stat(3, 30, 60, 300, 3000)); // new key type, baseline 0
        current.insert("C".to_owned(), stat(5, 50, 50, 500, 5000)); // unchanged -> omitted

        let delta = compute_page_in_delta(&baseline, &current);

        assert_eq!(
            delta.len(),
            2,
            "only key types with page-ins during the command are kept"
        );
        let a = delta.get("A").expect("A had new page-ins");
        assert_eq!(
            (a.count, a.fetch_us, a.deser_us, a.bytes, a.restored_bytes),
            (2, 30, 60, 300, 3000)
        );
        let b = delta.get("B").expect("B is new this command (baseline 0)");
        assert_eq!(
            (b.count, b.fetch_us, b.deser_us, b.bytes, b.restored_bytes),
            (3, 30, 60, 300, 3000)
        );
        assert!(
            !delta.contains_key("C"),
            "a key type with no new page-ins is omitted"
        );
    }

    #[test]
    fn pages_out_only_with_disk_headroom() {
        let thresholds = PageOutThresholds {
            min_free_disk_gb: 20,
        };
        assert_eq!(should_page_out_decision(Some(50 * GIB), thresholds), None);
        // threshold is inclusive
        assert_eq!(should_page_out_decision(Some(20 * GIB), thresholds), None);
        assert_eq!(
            should_page_out_decision(Some(19 * GIB), thresholds),
            Some(buck2_data::PageOutStarted::InsufficientDiskHeadroom)
        );
        // Free disk couldn't be determined.
        assert_eq!(
            should_page_out_decision(None, thresholds),
            Some(buck2_data::PageOutStarted::FreeDiskUnknown)
        );
    }

    #[test]
    fn min_free_disk_gb_saturates() {
        // A huge GiB threshold saturates rather than overflowing; nothing meets it.
        let thresholds = PageOutThresholds {
            min_free_disk_gb: u64::MAX,
        };
        assert_eq!(
            should_page_out_decision(Some(u64::MAX - 1), thresholds),
            Some(buck2_data::PageOutStarted::InsufficientDiskHeadroom)
        );
    }
}
