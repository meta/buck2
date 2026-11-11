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
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::PoisonError;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use buck2_build_api::actions::impls::write_action_cache_stats::WRITE_ACTION_CACHE_STATS;
use buck2_core::io_counters::IoCounterKey;
use buck2_error::BuckErrorContext;
use buck2_events::EventSinkStats;
use buck2_execute::dep_file_state::DepFileDbSize;
use buck2_execute::dep_file_state::DepFileStore;
use buck2_execute::re::manager::ReConnectionManager;
use buck2_fs::fs_util::DiskSpaceStats;
use buck2_fs::fs_util::disk_space_stats;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_hash::IntentionallyStdHashMap;
use buck2_util::process_stats::process_stats;
use buck2_util::system_stats::UnixSystemStats;
use dupe::Dupe;

use crate::cpu_usage_collector::CpuUsageCollector;
use crate::daemon::state::DaemonStateData;
use crate::daemon::state::TenantState;
use crate::jemalloc_stats::get_allocator_stats;
use crate::net_io::NetworkKind;
use crate::net_io::SystemNetworkIoCollector;

/// Per-worker cumulative counter snapshot. Tracked across snapshots so that
/// each snapshot can emit per-snapshot deltas (smaller varints than the
/// ever-growing cumulative values). Field set must mirror exactly the
/// cumulative-on-the-wire fields in the proto's
/// `TokioRuntimeStats.Worker` message that we want to delta-encode.
#[derive(Clone, Default)]
struct WorkerCounters {
    noop_count: u64,
    steal_count: u64,
    poll_count: u64,
    total_busy_duration_us: u64,
    local_schedule_count: u64,
    overflow_count: u64,
}

/// Top-level (runtime-wide) cumulative counter snapshot. Same delta-encoding
/// scheme as `WorkerCounters` — the field set must mirror the
/// cumulative-on-the-wire fields on `TokioRuntimeStats` itself (excluding
/// gauges like `num_alive_tasks` and `global_queue_depth`).
#[derive(Clone, Default)]
struct RuntimeCounters {
    budget_forced_yield_count: u64,
    remote_schedule_count: u64,
}

/// Per-collector state that backs event-log size optimizations for the tokio
/// runtime stats block. Wrapped in `Arc` because `SnapshotCollector` is
/// cloned into `HeartbeatGuard`'s background task and we need both clones to
/// share the same baseline state across snapshots.
struct TokioMetricsState {
    /// Set to `true` after the first snapshot of this collector emits the
    /// histogram bucket-upper-bound array. Subsequent snapshots emit an empty
    /// array — the bounds are constant for the daemon's lifetime, so the
    /// webconsole only needs to see them once and re-emitting them on every
    /// snapshot is just bytes on the wire.
    emitted_bucket_bounds: AtomicBool,

    /// Per-worker per-bucket cumulative poll counts as of the previous
    /// snapshot, used to compute per-snapshot deltas for emission.
    /// Outer index is worker id; inner index is histogram bucket.
    /// `None` until the first snapshot lands; first snapshot stores baseline
    /// without emitting any deltas (consumers already treat the first
    /// snapshot's per-window quantities as zero).
    prev_bucket_counts: Mutex<Option<Vec<Vec<u64>>>>,

    /// Same idea as `prev_bucket_counts` but for the per-worker scalar
    /// counters that are now delta-encoded on the wire (noop, steal, poll,
    /// busy duration, local schedule, overflow).
    prev_worker_counters: Mutex<Option<Vec<WorkerCounters>>>,

    /// Same idea but for top-level (runtime-wide) cumulative counters.
    prev_runtime_counters: Mutex<Option<RuntimeCounters>>,
}

impl TokioMetricsState {
    fn new() -> Self {
        Self {
            emitted_bucket_bounds: AtomicBool::new(false),
            prev_bucket_counts: Mutex::new(None),
            prev_worker_counters: Mutex::new(None),
            prev_runtime_counters: Mutex::new(None),
        }
    }
}

/// Stores state handles necessary to produce snapshots.
#[derive(Clone)]
pub struct SnapshotCollector {
    daemon: Arc<DaemonStateData>,
    tenant: Arc<TenantState>,
    net_io_collector: SystemNetworkIoCollector,
    buck_out_path: Arc<AbsNormPathBuf>,
    cpu_usage_collector: Option<CpuUsageCollector>,
    /// Handle to the *main* (`buck2-rt`) runtime where DICE / build work runs.
    /// Snapshot collection itself runs on the smaller `buck2-tn` Tonic
    /// runtime, so `tokio::runtime::Handle::current()` would otherwise report
    /// the gRPC runtime's pool sizes (which are hardcoded to 2). Always read
    /// metrics from this handle instead.
    runtime: tokio::runtime::Handle,
    tokio_metrics_state: Arc<TokioMetricsState>,
}

/// How long the sampler pauses after each reading of the dep-file database.
const DB_SIZE_SAMPLE_DELAY: Duration = Duration::from_millis(500);

/// Keeps the latest reading of the dep-file cache database, sampled off to the side so that
/// building a snapshot never reads the database.
pub struct DepFileDbSizeSampler {
    /// The last attempt's outcome.
    latest: Mutex<Option<DepFileDbSize>>,
    /// Set once, by `start`, after the task it refers to has been spawned.
    sampler: OnceLock<tokio::task::JoinHandle<()>>,
}

impl DepFileDbSizeSampler {
    /// Starts sampling on `rt`, which must outlive the daemon's commands. Each reading is handed
    /// to that runtime's blocking pool, and the lock is taken only to store the result, so no lock
    /// is ever held across the read.
    pub fn start(store: Arc<dyn DepFileStore>, rt: &tokio::runtime::Handle) -> Arc<Self> {
        let this = Arc::new(Self {
            latest: Mutex::new(None),
            sampler: OnceLock::new(),
        });
        let sampling = rt.spawn({
            // Weak, so the task does not keep the sampler alive: the strong count reaching zero is
            // what runs `Drop`, and `Drop` is what stops this loop.
            let this = Arc::downgrade(&this);
            let rt = rt.clone();
            async move {
                loop {
                    let store = store.dupe();
                    let measured = rt.spawn_blocking(move || store.db_size()).await;
                    // A panicking measurement leaves the previous reading in place rather than
                    // reporting nothing, since the reading itself was not disproved.
                    if let Ok(measured) = measured {
                        let Some(this) = this.upgrade() else {
                            return;
                        };
                        *this.latest.lock().unwrap_or_else(PoisonError::into_inner) = measured;
                    }
                    // Pausing after the reading rather than ticking to a schedule: a slow reading
                    // is followed by the same pause as any other, so a slow database is sampled
                    // less often instead of more. Every path through the loop reaches this.
                    tokio::time::sleep(DB_SIZE_SAMPLE_DELAY).await;
                }
            }
        });
        // Cannot already be set: `start` is the only writer and runs once per sampler.
        let _unused = this.sampler.set(sampling);
        this
    }

    /// The latest reading, or `None` if there is not one yet or the database could not be read.
    /// Cheap enough for the snapshot path: a lock and a copy, never any I/O.
    pub fn latest(&self) -> Option<DepFileDbSize> {
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for DepFileDbSizeSampler {
    fn drop(&mut self) {
        if let Some(sampling) = self.sampler.take() {
            sampling.abort();
        }
    }
}

impl SnapshotCollector {
    pub fn new(
        daemon: Arc<DaemonStateData>,
        tenant: Arc<TenantState>,
        runtime: tokio::runtime::Handle,
    ) -> SnapshotCollector {
        let buck_out_path = tenant.repo.paths.buck_out_path();
        SnapshotCollector {
            daemon,
            tenant,
            net_io_collector: SystemNetworkIoCollector::new(),
            buck_out_path: buck_out_path.into(),
            cpu_usage_collector: CpuUsageCollector::new().ok(),
            runtime,
            tokio_metrics_state: Arc::new(TokioMetricsState::new()),
        }
    }

    /// Create a new Snapshot.
    pub async fn create_snapshot(&self) -> buck2_data::Snapshot {
        let mut snapshot = buck2_data::Snapshot::default();
        self.add_system_metrics(&mut snapshot);
        self.add_daemon_metrics(&mut snapshot);
        self.add_re_metrics(&mut snapshot);
        self.add_write_action_cache_metrics(&mut snapshot);
        self.add_http_metrics(&mut snapshot);
        self.add_io_metrics(&mut snapshot);
        self.add_dice_metrics(&mut snapshot);
        self.add_digest_interner_metrics(&mut snapshot);
        self.add_standalone_ttl_refresh_metrics(&mut snapshot);
        self.add_materializer_metrics(&mut snapshot);
        self.add_sink_metrics(&mut snapshot);
        self.add_net_io_metrics(&mut snapshot);
        self.add_cpu_usage(&mut snapshot);
        self.add_memory_metrics(&mut snapshot).await;
        self.add_tokio_runtime_stats(&mut snapshot);
        self.add_dep_file_db_size(&mut snapshot);
        snapshot
    }

    /// Copies the latest database size from the repo's sampler. No I/O: snapshots feed
    /// superconsole, and sizing the database can block for as long as a cold disk takes.
    fn add_dep_file_db_size(&self, snapshot: &mut buck2_data::Snapshot) {
        let Some(size) = self
            .tenant
            .repo
            .persisted_dep_file_cache
            .as_ref()
            .and_then(|cache| cache.db_size.latest())
        else {
            return;
        };
        snapshot.dep_file_db_entries = Some(size.entries);
        snapshot.dep_file_db_bytes = Some(size.bytes);
    }

    fn add_daemon_metrics(&self, snapshot: &mut buck2_data::Snapshot) {
        snapshot.blocking_executor_io_queue_size =
            self.daemon.blocking_executor_factory.queue_size() as u64;
        let inline_io = self.daemon.blocking_executor_factory.inline_io_stats();
        snapshot.blocking_executor_io_inline_in_flight = inline_io.in_flight;
        snapshot.blocking_executor_io_inline_waiting = inline_io.waiting;
        if let Some(store) = self
            .tenant
            .repo
            .persisted_dep_file_cache
            .as_ref()
            .map(|cache| &cache.store)
        {
            snapshot.dep_file_db_queue_size = store.queue_size();
            let writes = store.write_stats();
            snapshot.dep_file_db_writes_applied = writes.applied;
            snapshot.dep_file_db_inserts = writes.inserts;
            snapshot.dep_file_db_insert_duration_us = writes.insert_duration_us;
            snapshot.dep_file_db_deletes = writes.deletes;
            snapshot.dep_file_db_delete_duration_us = writes.delete_duration_us;
            snapshot.dep_file_db_clears = writes.clears;
            snapshot.dep_file_db_clear_duration_us = writes.clear_duration_us;
            snapshot.dep_file_db_write_duration_us = writes.duration_us;
            snapshot.dep_file_db_write_max_us = writes.max_us;
            let reads = store.read_stats();
            snapshot.dep_file_db_probes = reads.probes;
            snapshot.dep_file_db_probe_duration_us = reads.probe_duration_us;
            snapshot.dep_file_db_fetches = reads.fetches;
            snapshot.dep_file_db_fetches_found = reads.fetches_found;
            snapshot.dep_file_db_fetch_duration_us = reads.fetch_duration_us;
            snapshot.dep_file_db_read_max_us = reads.max_us;
            snapshot.dep_file_db_read_lock_wait_us = reads.lock_wait_us;
            snapshot.dep_file_db_read_lock_wait_max_us = reads.lock_wait_max_us;
            snapshot.dep_file_db_read_connections = reads.read_connections;
            snapshot.dep_file_db_hits = reads.hits;
        }
    }

    fn add_io_metrics(&self, snapshot: &mut buck2_data::Snapshot) {
        let metrics = self.runtime.metrics();
        snapshot.tokio_blocking_queue_depth = metrics.blocking_queue_depth() as u64;
        snapshot.tokio_num_idle_blocking_threads = metrics.num_idle_blocking_threads() as u64;
        snapshot.tokio_num_blocking_threads = metrics.num_blocking_threads() as u64;

        // Using loop here to make sure no key is forgotten.
        for key in IoCounterKey::ALL {
            let pointer = match key {
                IoCounterKey::Stat => &mut snapshot.io_in_flight_stat,
                IoCounterKey::Copy => &mut snapshot.io_in_flight_copy,
                IoCounterKey::Symlink => &mut snapshot.io_in_flight_symlink,
                IoCounterKey::Hardlink => &mut snapshot.io_in_flight_hardlink,
                IoCounterKey::MkDir => &mut snapshot.io_in_flight_mk_dir,
                IoCounterKey::ReadDir => &mut snapshot.io_in_flight_read_dir,
                IoCounterKey::ReadDirEden => &mut snapshot.io_in_flight_read_dir_eden,
                IoCounterKey::RmDir => &mut snapshot.io_in_flight_rm_dir,
                IoCounterKey::RmDirAll => &mut snapshot.io_in_flight_rm_dir_all,
                IoCounterKey::StatEden => &mut snapshot.io_in_flight_stat_eden,
                IoCounterKey::Chmod => &mut snapshot.io_in_flight_chmod,
                IoCounterKey::ReadLink => &mut snapshot.io_in_flight_read_link,
                IoCounterKey::Remove => &mut snapshot.io_in_flight_remove,
                IoCounterKey::Rename => &mut snapshot.io_in_flight_rename,
                IoCounterKey::Read => &mut snapshot.io_in_flight_read,
                IoCounterKey::Write => &mut snapshot.io_in_flight_write,
                IoCounterKey::Canonicalize => &mut snapshot.io_in_flight_canonicalize,
                IoCounterKey::EdenSettle => &mut snapshot.io_in_flight_eden_settle,
            };
            *pointer = key.get();
        }

        for key in IoCounterKey::ALL {
            let pointer = match key {
                IoCounterKey::Copy => &mut snapshot.io_copy_count,
                IoCounterKey::Symlink => &mut snapshot.io_symlink_count,
                IoCounterKey::Hardlink => &mut snapshot.io_hardlink_count,
                IoCounterKey::MkDir => &mut snapshot.io_mkdir_count,
                IoCounterKey::ReadDir => &mut snapshot.io_readdir_count,
                IoCounterKey::ReadDirEden => &mut snapshot.io_readdir_eden_count,
                IoCounterKey::RmDir => &mut snapshot.io_rmdir_count,
                IoCounterKey::RmDirAll => &mut snapshot.io_rmdir_all_count,
                IoCounterKey::Stat => &mut snapshot.io_stat_count,
                IoCounterKey::StatEden => &mut snapshot.io_stat_eden_count,
                IoCounterKey::Chmod => &mut snapshot.io_chmod_count,
                IoCounterKey::ReadLink => &mut snapshot.io_readlink_count,
                IoCounterKey::Remove => &mut snapshot.io_remove_count,
                IoCounterKey::Rename => &mut snapshot.io_rename_count,
                IoCounterKey::Read => &mut snapshot.io_read_count,
                IoCounterKey::Write => &mut snapshot.io_write_count,
                IoCounterKey::Canonicalize => &mut snapshot.io_canonicalize_count,
                IoCounterKey::EdenSettle => &mut snapshot.io_eden_settle_count,
            };
            *pointer = Some(key.get_finished());
        }
    }

    fn add_write_action_cache_metrics(&self, snapshot: &mut buck2_data::Snapshot) {
        let counts = WRITE_ACTION_CACHE_STATS.counts();
        snapshot.write_action_cache_hits = counts.hits;
        snapshot.write_action_cache_misses = counts.misses;
        snapshot.write_action_cache_lookup_failures = counts.lookup_failures;
        snapshot.write_action_cache_populated = counts.populated;
        snapshot.write_action_cache_populate_failures = counts.populate_failures;
        snapshot.write_action_cache_uploaded = counts.uploaded;
        snapshot.write_action_cache_written = counts.written;
        snapshot.write_action_cache_verified = counts.verified;
        snapshot.write_action_cache_mismatches = counts.mismatches;
    }

    fn add_re_metrics(&self, snapshot: &mut buck2_data::Snapshot) {
        fn inner(
            snapshot: &mut buck2_data::Snapshot,
            re: &ReConnectionManager,
        ) -> buck2_error::Result<()> {
            let stats = re
                .get_network_stats()
                .buck_error_context("Error collecting network stats")?;

            snapshot.re_download_bytes = stats.downloaded;
            snapshot.re_upload_bytes = stats.uploaded;
            snapshot.re_uploads_started = stats.uploads.started;
            snapshot.re_uploads_finished_successfully = stats.uploads.finished_successfully;
            snapshot.re_uploads_finished_with_error = stats.uploads.finished_with_error;
            snapshot.re_downloads_started = stats.downloads.started;
            snapshot.re_downloads_finished_successfully = stats.downloads.finished_successfully;
            snapshot.re_downloads_finished_with_error = stats.downloads.finished_with_error;
            snapshot.re_action_cache_started = stats.action_cache.started;
            snapshot.re_action_cache_finished_successfully =
                stats.action_cache.finished_successfully;
            snapshot.re_action_cache_finished_with_error = stats.action_cache.finished_with_error;
            snapshot.re_executes_started = stats.executes.started;
            snapshot.re_executes_finished_successfully = stats.executes.finished_successfully;
            snapshot.re_executes_finished_with_error = stats.executes.finished_with_error;
            snapshot.re_materializes_started = stats.materializes.started;
            snapshot.re_materializes_finished_successfully =
                stats.materializes.finished_successfully;
            snapshot.re_materializes_finished_with_error = stats.materializes.finished_with_error;
            snapshot.re_write_action_results_started = stats.write_action_results.started;
            snapshot.re_write_action_results_finished_successfully =
                stats.write_action_results.finished_successfully;
            snapshot.re_write_action_results_finished_with_error =
                stats.write_action_results.finished_with_error;
            snapshot.re_get_digest_expirations_started = stats.get_digest_expirations.started;
            snapshot.re_get_digest_expirations_finished_successfully =
                stats.get_digest_expirations.finished_successfully;
            snapshot.re_get_digest_expirations_finished_with_error =
                stats.get_digest_expirations.finished_with_error;
            snapshot.re_presence_check_rpcs = stats.presence.rpcs;
            snapshot.re_presence_check_digests_queried = stats.presence.digests_queried;
            snapshot.re_presence_check_digest_answers = stats.presence.digest_answers;
            snapshot.re_presence_check_largest_batch = stats.presence.largest_batch;

            snapshot.zdb_download_queries = stats.download_stats.zdb.queries;
            snapshot.zdb_download_bytes = stats.download_stats.zdb.bytes;
            snapshot.zdb_upload_queries = stats.upload_stats.zdb.queries;
            snapshot.zdb_upload_bytes = stats.upload_stats.zdb.bytes;

            snapshot.zgateway_download_queries = stats.download_stats.zgateway.queries;
            snapshot.zgateway_download_bytes = stats.download_stats.zgateway.bytes;
            snapshot.zgateway_upload_queries = stats.upload_stats.zgateway.queries;
            snapshot.zgateway_upload_bytes = stats.upload_stats.zgateway.bytes;

            snapshot.manifold_download_queries = stats.download_stats.manifold.queries;
            snapshot.manifold_download_bytes = stats.download_stats.manifold.bytes;
            snapshot.manifold_upload_queries = stats.upload_stats.manifold.queries;
            snapshot.manifold_upload_bytes = stats.upload_stats.manifold.bytes;

            snapshot.hedwig_download_queries = stats.download_stats.hedwig.queries;
            snapshot.hedwig_download_bytes = stats.download_stats.hedwig.bytes;
            snapshot.hedwig_upload_queries = stats.upload_stats.hedwig.queries;
            snapshot.hedwig_upload_bytes = stats.upload_stats.hedwig.bytes;

            snapshot.local_cache_hits_files = stats.local_cache.hits_files;
            snapshot.local_cache_hits_bytes = stats.local_cache.hits_bytes;
            snapshot.local_cache_misses_files = stats.local_cache.misses_files;
            snapshot.local_cache_misses_bytes = stats.local_cache.misses_bytes;

            snapshot.local_cache_hits_files_from_memory_cache = stats.local_cache.hits_from_memory;
            snapshot.local_cache_hits_files_from_filesystem_cache = stats.local_cache.hits_from_fs;

            snapshot.local_cache_lookups = stats.local_cache.cache_lookups;
            snapshot.local_cache_lookup_latency_microseconds =
                stats.local_cache.cache_lookup_latency_microseconds;

            Ok(())
        }

        // Nothing we can do if we get an error, unfortunately.
        if let Err(e) = inner(snapshot, &self.tenant.repo.re_client_manager) {
            tracing::debug!("Error collecting network stats: {:#}", e);
        }
    }

    fn add_http_metrics(&self, snapshot: &mut buck2_data::Snapshot) {
        snapshot.http_download_bytes = self.daemon.http_client.stats().get_downloaded_bytes();
    }

    fn add_dice_metrics(&self, snapshot: &mut buck2_data::Snapshot) {
        let metrics = self.tenant.dice_manager.unsafe_dice().metrics();
        snapshot.dice_key_count = metrics.key_count as u64;
        snapshot.dice_active_transaction_count = metrics.active_transaction_count;
    }

    fn add_digest_interner_metrics(&self, snapshot: &mut buck2_data::Snapshot) {
        let stats = buck2_common::cas_digest::file_digest_interner_stats();
        snapshot.file_digest_interner_entries = stats.entries;
        snapshot.file_digest_interner_pinned_entries = stats.pinned;
        snapshot.file_digest_interner_paged_entries = stats.paged;
        snapshot.file_digest_interner_hits = stats.hits;
        snapshot.file_digest_interner_misses = stats.misses;
    }

    fn add_standalone_ttl_refresh_metrics(&self, snapshot: &mut buck2_data::Snapshot) {
        let stats = self.daemon.standalone_ttl_refresh.stats();
        snapshot.standalone_ttl_refresh = Some(buck2_data::StandaloneTtlRefreshStats {
            passes: stats.passes,
            errors: stats.errors,
            last_pass_visited: stats.last_pass_visited,
            last_pass_swept: stats.last_pass_swept,
            last_pass_candidates: stats.last_pass_candidates,
            last_pass_refreshed: stats.last_pass_refreshed,
            last_pass_gone: stats.last_pass_gone,
            last_pass_duration_us: stats.last_pass_duration_us,
        });
    }

    fn add_materializer_metrics(&self, snapshot: &mut buck2_data::Snapshot) {
        self.tenant.repo.materializer.add_snapshot_stats(snapshot);
    }

    fn add_sink_metrics(&self, snapshot: &mut buck2_data::Snapshot) {
        if let Some(metrics) = self.daemon.scribe_sink.as_ref().map(|sink| sink.stats()) {
            let EventSinkStats {
                successes,
                failures_invalid_request,
                failures_unauthorized,
                failures_rate_limited,
                failures_pushed_back,
                failures_enqueue_failed,
                failures_internal_error,
                failures_timed_out,
                failures_unknown,
                buffered,
                dropped,
                bytes_written,
                worker_busy_us,
                worker_convert_us,
                worker_upload_us,
                worker_send_wait_us,
                worker_transport_opens,
                worker_upload_connect_us,
                worker_upload_slot_wait_us,
            } = metrics;
            snapshot.sink_successes = Some(successes);
            snapshot.sink_failures = Some(metrics.failures());
            snapshot.sink_failures_invalid_request = Some(failures_invalid_request);
            snapshot.sink_failures_unauthorized = Some(failures_unauthorized);
            snapshot.sink_failures_rate_limited = Some(failures_rate_limited);
            snapshot.sink_failures_pushed_back = Some(failures_pushed_back);
            snapshot.sink_failures_enqueue_failed = Some(failures_enqueue_failed);
            snapshot.sink_failures_internal_error = Some(failures_internal_error);
            snapshot.sink_failures_timed_out = Some(failures_timed_out);
            snapshot.sink_failures_unknown = Some(failures_unknown);
            snapshot.sink_buffer_depth = Some(buffered);
            snapshot.sink_dropped = Some(dropped);
            snapshot.sink_bytes_written = Some(bytes_written);
            snapshot.sink_worker_busy_us = Some(worker_busy_us);
            snapshot.sink_worker_convert_us = Some(worker_convert_us);
            snapshot.sink_worker_upload_us = Some(worker_upload_us);
            snapshot.sink_worker_send_wait_us = Some(worker_send_wait_us);
            snapshot.sink_worker_transport_opens = Some(worker_transport_opens);
            snapshot.sink_worker_upload_connect_us = Some(worker_upload_connect_us);
            snapshot.sink_worker_upload_slot_wait_us = Some(worker_upload_slot_wait_us);
        }
    }

    fn add_net_io_metrics(&self, snapshot: &mut buck2_data::Snapshot) {
        if let Ok(Some(net_io_counters_per_nic)) = self.net_io_collector.collect() {
            snapshot.network_interface_stats = net_io_counters_per_nic
                .into_iter()
                .map(|(nic, counters)| {
                    (
                        nic,
                        buck2_data::NetworkInterfaceStats {
                            tx_bytes: counters.bytes_sent,
                            rx_bytes: counters.bytes_recv,
                            network_kind: match counters.network_kind {
                                NetworkKind::WiFi => buck2_data::NetworkKind::WiFi.into(),
                                NetworkKind::Ethernet => buck2_data::NetworkKind::Ethernet.into(),
                                NetworkKind::Unknown => {
                                    buck2_data::NetworkKind::UnknownNetKind.into()
                                }
                            },
                        },
                    )
                })
                .collect();
        } else {
            snapshot.network_interface_stats = IntentionallyStdHashMap::default();
        }
    }

    fn add_system_metrics(&self, snapshot: &mut buck2_data::Snapshot) {
        let process_stats = process_stats();
        if let Some(max_rss_bytes) = process_stats.max_rss_bytes {
            snapshot.buck2_max_rss = max_rss_bytes;
        }
        if let Some(user_cpu_us) = process_stats.user_cpu_us {
            snapshot.buck2_user_cpu_us = user_cpu_us;
        }
        if let Some(system_cpu_us) = process_stats.system_cpu_us {
            snapshot.buck2_system_cpu_us = system_cpu_us;
        }
        snapshot.daemon_uptime_s = (Instant::now() - self.daemon.start_time).as_secs();
        snapshot.buck2_rss = process_stats.rss_bytes;
        let allocator_stats = get_allocator_stats().ok();
        if let Some(alloc_stats) = allocator_stats {
            snapshot.malloc_bytes_active = alloc_stats.bytes_active;
            snapshot.malloc_bytes_allocated = alloc_stats.bytes_allocated;
        }

        if let Ok(DiskSpaceStats {
            total_space,
            free_space,
        }) = disk_space_stats(&*self.buck_out_path)
        {
            snapshot.used_disk_space_bytes = Some(total_space - free_space);
        }

        if let Some(UnixSystemStats {
            load1,
            load5,
            load15,
        }) = UnixSystemStats::get()
        {
            snapshot.unix_system_stats = Some(buck2_data::UnixSystemStats {
                load1,
                load5,
                load15,
            });
        }
    }

    fn add_tokio_runtime_stats(&self, snapshot: &mut buck2_data::Snapshot) {
        let metrics = self.runtime.metrics();
        let num_workers = metrics.num_workers();

        let num_buckets = if metrics.poll_time_histogram_enabled() {
            metrics.poll_time_histogram_num_buckets()
        } else {
            0
        };

        // Histogram bucket upper bounds are constant for the daemon's
        // lifetime, so emit them only on this collector's first snapshot.
        // Subsequent snapshots leave the array empty and the consumer reuses
        // the bounds it captured from the first snapshot.
        let bucket_uppers_ns: Vec<u64> = if num_buckets > 0
            && !self
                .tokio_metrics_state
                .emitted_bucket_bounds
                .swap(true, Ordering::Relaxed)
        {
            (0..num_buckets)
                .map(|b| metrics.poll_time_histogram_bucket_range(b).end.as_nanos() as u64)
                .collect()
        } else {
            Vec::new()
        };

        // Read current cumulative bucket counts, then compute per-snapshot
        // deltas vs the previous emission and rotate into `prev`. First
        // snapshot stores baseline without emitting any deltas — that matches
        // the consumer's "first snapshot has no window" semantics.
        let mut prev_guard = self
            .tokio_metrics_state
            .prev_bucket_counts
            .lock()
            .expect("tokio metrics state mutex poisoned");
        let curr_bucket_counts: Vec<Vec<u64>> = (0..num_workers)
            .map(|i| {
                (0..num_buckets)
                    .map(|b| metrics.poll_time_histogram_bucket_count(i, b))
                    .collect()
            })
            .collect();
        let bucket_deltas_per_worker: Vec<Vec<u64>> = match prev_guard.as_ref() {
            None => vec![Vec::new(); num_workers],
            Some(prev) => curr_bucket_counts
                .iter()
                .enumerate()
                .map(|(i, curr)| compute_bucket_deltas(prev.get(i).map(Vec::as_slice), curr))
                .collect(),
        };
        *prev_guard = Some(curr_bucket_counts);
        drop(prev_guard);

        // Same baseline-then-deltas pattern as for bucket counts, but for the
        // per-worker scalar counters. First snapshot stores baseline and
        // emits zero deltas; subsequent snapshots emit `curr - prev`.
        let mut prev_counters_guard = self
            .tokio_metrics_state
            .prev_worker_counters
            .lock()
            .expect("tokio metrics state mutex poisoned");
        let curr_counters: Vec<WorkerCounters> = (0..num_workers)
            .map(|i| WorkerCounters {
                noop_count: metrics.worker_noop_count(i),
                steal_count: metrics.worker_steal_count(i),
                poll_count: metrics.worker_poll_count(i),
                total_busy_duration_us: metrics.worker_total_busy_duration(i).as_micros() as u64,
                local_schedule_count: metrics.worker_local_schedule_count(i),
                overflow_count: metrics.worker_overflow_count(i),
            })
            .collect();
        let counter_deltas: Vec<WorkerCounters> = match prev_counters_guard.as_ref() {
            None => vec![WorkerCounters::default(); num_workers],
            Some(prev) => curr_counters
                .iter()
                .enumerate()
                .map(|(i, curr)| compute_worker_counter_deltas(prev.get(i), curr))
                .collect(),
        };
        *prev_counters_guard = Some(curr_counters);
        drop(prev_counters_guard);

        let workers = (0..num_workers)
            .map(|i| {
                let d = &counter_deltas[i];
                buck2_data::tokio_runtime_stats::Worker {
                    // Cumulative; parity is needed by the SPA's worker-state
                    // classifier (odd ⇒ currently parked).
                    park_unpark_count: metrics.worker_park_unpark_count(i),
                    // Per-snapshot deltas — see compute_worker_counter_deltas.
                    noop_count: d.noop_count,
                    steal_count: d.steal_count,
                    poll_count: d.poll_count,
                    total_busy_duration_us: d.total_busy_duration_us,
                    local_schedule_count: d.local_schedule_count,
                    overflow_count: d.overflow_count,
                    // Gauge — current depth at snapshot time, not cumulative.
                    local_queue_depth: metrics.worker_local_queue_depth(i) as u64,
                    poll_time_bucket_counts: bucket_deltas_per_worker[i].clone(),
                }
            })
            .collect();
        // Same baseline-then-deltas pattern for the runtime-wide cumulative
        // counters. Gauges (num_alive_tasks, global_queue_depth) pass through
        // as instantaneous readings.
        let mut prev_runtime_guard = self
            .tokio_metrics_state
            .prev_runtime_counters
            .lock()
            .expect("tokio metrics state mutex poisoned");
        let curr_runtime = RuntimeCounters {
            budget_forced_yield_count: metrics.budget_forced_yield_count(),
            remote_schedule_count: metrics.remote_schedule_count(),
        };
        let runtime_deltas = match prev_runtime_guard.as_ref() {
            None => RuntimeCounters::default(),
            Some(prev) => compute_runtime_counter_deltas(prev, &curr_runtime),
        };
        *prev_runtime_guard = Some(curr_runtime);
        drop(prev_runtime_guard);

        snapshot.tokio_runtime_stats = Some(buck2_data::TokioRuntimeStats {
            num_alive_tasks: metrics.num_alive_tasks() as u64,
            global_queue_depth: metrics.global_queue_depth() as u64,
            budget_forced_yield_count: runtime_deltas.budget_forced_yield_count,
            remote_schedule_count: runtime_deltas.remote_schedule_count,
            workers,
            poll_time_histogram_bucket_upper_bound_ns: bucket_uppers_ns,
        });
    }

    fn add_cpu_usage(&self, snapshot: &mut buck2_data::Snapshot) {
        if let Some(collector) = &self.cpu_usage_collector {
            if let Some(cpu_usage) = collector.get_usage_since_command_start() {
                snapshot.host_cpu_usage_system_ms = Some(cpu_usage.system_millis);
                snapshot.host_cpu_usage_user_ms = Some(cpu_usage.user_millis);
            }
        }
    }

    async fn add_memory_metrics(&self, snapshot: &mut buck2_data::Snapshot) {
        #[cfg(not(unix))]
        {
            let _snapshot = snapshot;
        }
        #[cfg(unix)]
        {
            use buck2_resource_control::cgroup_files::MemoryStat;

            fn convert_stats(
                stats: &MemoryStat,
                swap_bytes: u64,
                memory_pressure_10s_avg: f64,
                memory_pressure_60s_avg: f64,
            ) -> buck2_data::UnixCgroupMemoryStats {
                buck2_data::UnixCgroupMemoryStats {
                    anon: stats.anon,
                    file: stats.file,
                    kernel: stats.kernel,
                    swap_bytes,
                    memory_pressure_10s_avg,
                    memory_pressure_60s_avg,
                }
            }

            // Try to read Buck2 daemon memory information from cgroup

            if let Some(memory_tracker) = self.daemon.memory_tracker.as_ref() {
                let cgroup_tree = &memory_tracker.cgroup_tree;
                let (stat, swap, pressure) = futures::future::join3(
                    cgroup_tree.allprocs().read_memory_stat(),
                    cgroup_tree.allprocs().read_swap_current(),
                    cgroup_tree.allprocs().read_memory_pressure(),
                )
                .await;
                if let (Ok(stat), Ok(swap), Ok(pressure)) = (stat, swap, pressure) {
                    let cgroup_stats =
                        convert_stats(&stat, swap, pressure.full.avg10, pressure.full.avg60);
                    snapshot.allprocs_cgroup = Some(cgroup_stats);
                }

                let (stat, swap, pressure) = futures::future::join3(
                    cgroup_tree.forkserver_and_actions().read_memory_stat(),
                    cgroup_tree.forkserver_and_actions().read_swap_current(),
                    cgroup_tree.forkserver_and_actions().read_memory_pressure(),
                )
                .await;
                if let (Ok(stat), Ok(swap), Ok(pressure)) = (stat, swap, pressure) {
                    let cgroup_stats =
                        convert_stats(&stat, swap, pressure.full.avg10, pressure.full.avg60);
                    snapshot.forkserver_actions_cgroup = Some(cgroup_stats);
                }
            }
        }
    }
}

/// Field-wise saturating delta of the runtime-wide scalar counters that are
/// delta-encoded on the wire. `saturating_sub` guards against any
/// (unexpected) cumulative-counter regression.
fn compute_runtime_counter_deltas(
    prev: &RuntimeCounters,
    curr: &RuntimeCounters,
) -> RuntimeCounters {
    RuntimeCounters {
        budget_forced_yield_count: curr
            .budget_forced_yield_count
            .saturating_sub(prev.budget_forced_yield_count),
        remote_schedule_count: curr
            .remote_schedule_count
            .saturating_sub(prev.remote_schedule_count),
    }
}

/// Field-wise saturating delta of the per-worker scalar counters that are
/// delta-encoded on the wire. `prev` is `None` only for workers that didn't
/// exist last snapshot (extreme edge case — tokio's worker count is fixed at
/// runtime build time); in that case all previous values are treated as 0,
/// so the delta is the worker's full cumulative reading. `saturating_sub`
/// guards against any (unexpected) cumulative-counter regression.
fn compute_worker_counter_deltas(
    prev: Option<&WorkerCounters>,
    curr: &WorkerCounters,
) -> WorkerCounters {
    let p = prev.cloned().unwrap_or_default();
    WorkerCounters {
        noop_count: curr.noop_count.saturating_sub(p.noop_count),
        steal_count: curr.steal_count.saturating_sub(p.steal_count),
        poll_count: curr.poll_count.saturating_sub(p.poll_count),
        total_busy_duration_us: curr
            .total_busy_duration_us
            .saturating_sub(p.total_busy_duration_us),
        local_schedule_count: curr
            .local_schedule_count
            .saturating_sub(p.local_schedule_count),
        overflow_count: curr.overflow_count.saturating_sub(p.overflow_count),
    }
}

/// Per-bucket `curr - prev` delta with trailing zeros trimmed.
fn compute_bucket_deltas(prev: Option<&[u64]>, curr: &[u64]) -> Vec<u64> {
    let mut out: Vec<u64> = curr
        .iter()
        .enumerate()
        .map(|(b, &c)| {
            let p = prev.and_then(|p| p.get(b).copied()).unwrap_or(0);
            c.saturating_sub(p)
        })
        .collect();
    while out.last() == Some(&0) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tokio_unstable_smoke {
    //! Self-test that the tokio APIs `add_tokio_runtime_stats` depends on
    //! are available at build time. Every method called below is gated
    //! behind tokio's `cfg(tokio_unstable)` macro — if the
    //! `tokio_unstable` rustflag in `fbcode/buck2/.cargo/config.toml` is
    //! ever removed, THIS TEST is the loud, localized failure (instead of
    //! a generic "no method named X found" deep inside snapshot
    //! collection). Also exercises `enable_metrics_poll_time_histogram`
    //! so the corresponding daemon.rs runtime configuration can't silently
    //! get removed without something noticing.
    use std::time::Duration;

    use tokio::runtime::Builder;
    use tokio::runtime::HistogramConfiguration;
    use tokio::runtime::LogHistogram;

    #[test]
    fn tokio_unstable_metrics_compile_and_work() {
        let rt = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .enable_metrics_poll_time_histogram()
            .metrics_poll_time_histogram_configuration(HistogramConfiguration::log(
                LogHistogram::default(),
            ))
            .build()
            .expect("build runtime");

        rt.block_on(async {
            // Spawn one task so workers actually do something measurable.
            tokio::time::sleep(Duration::from_millis(1)).await;
        });

        let m = rt.handle().metrics();
        assert!(
            m.poll_time_histogram_enabled(),
            "histogram should be enabled after enable_metrics_poll_time_histogram"
        );
        let n_workers = m.num_workers();
        assert!(n_workers >= 1);
        // Touch every API the snapshot collector reads from. If any of these
        // methods disappear from tokio's surface (e.g. the `tokio_unstable`
        // cfg is dropped), this fails to compile and points at the cause.
        let _ = m.worker_park_unpark_count(0);
        let _ = m.worker_noop_count(0);
        let _ = m.worker_steal_count(0);
        let _ = m.worker_poll_count(0);
        let _ = m.worker_total_busy_duration(0);
        let _ = m.worker_local_schedule_count(0);
        let _ = m.worker_overflow_count(0);
        let _ = m.worker_local_queue_depth(0);
        let _ = m.poll_time_histogram_num_buckets();
        let _ = m.poll_time_histogram_bucket_range(0);
        let _ = m.poll_time_histogram_bucket_count(0, 0);
        let _ = m.budget_forced_yield_count();
        let _ = m.remote_schedule_count();
        let _ = m.global_queue_depth();
        let _ = m.num_alive_tasks();
    }
}

#[cfg(test)]
mod compute_bucket_deltas_tests {
    use super::compute_bucket_deltas;

    #[test]
    fn first_snapshot_with_no_prev_keeps_full_curr() {
        // None prev means treat previous as all zeros. With curr non-zero,
        // the delta is curr itself; trailing zeros still get trimmed.
        assert_eq!(compute_bucket_deltas(None, &[1, 2, 0, 0]), vec![1, 2]);
    }

    #[test]
    fn typical_short_poll_only_emits_short_buckets() {
        // Most workers in most snapshots only see polls in the first few
        // buckets — trailing zeros should be trimmed entirely.
        let prev = vec![100, 50, 0, 0, 0, 0];
        let curr = vec![110, 53, 0, 0, 0, 0];
        assert_eq!(compute_bucket_deltas(Some(&prev), &curr), vec![10, 3]);
    }

    #[test]
    fn no_change_at_all_emits_empty_array() {
        let prev = vec![100, 50, 0, 1];
        let curr = vec![100, 50, 0, 1];
        assert_eq!(compute_bucket_deltas(Some(&prev), &curr), Vec::<u64>::new());
    }

    #[test]
    fn delta_in_tail_bucket_is_preserved() {
        // A long poll lands in a tail bucket — trim must not strip it.
        let prev = vec![100, 50, 0, 0, 0];
        let curr = vec![110, 50, 0, 0, 1];
        assert_eq!(
            compute_bucket_deltas(Some(&prev), &curr),
            vec![10, 0, 0, 0, 1]
        );
    }

    #[test]
    fn non_monotonic_counter_clamps_to_zero() {
        // Tokio's bucket counts shouldn't decrease, but if they ever do
        // (programmer error / reset / overflow), saturating_sub keeps us
        // from emitting wrap-around garbage.
        let prev = vec![10, 20];
        let curr = vec![5, 25];
        assert_eq!(compute_bucket_deltas(Some(&prev), &curr), vec![0, 5]);
    }
}

#[cfg(test)]
mod compute_worker_counter_deltas_tests {
    use super::WorkerCounters;
    use super::compute_worker_counter_deltas;

    #[test]
    fn no_prev_treats_baseline_as_zero() {
        let curr = WorkerCounters {
            poll_count: 100,
            total_busy_duration_us: 250,
            ..Default::default()
        };
        let d = compute_worker_counter_deltas(None, &curr);
        assert_eq!(d.poll_count, 100);
        assert_eq!(d.total_busy_duration_us, 250);
        assert_eq!(d.noop_count, 0);
    }

    #[test]
    fn typical_deltas_are_field_wise_subtraction() {
        let prev = WorkerCounters {
            noop_count: 5,
            steal_count: 0,
            poll_count: 1000,
            total_busy_duration_us: 500_000,
            local_schedule_count: 1000,
            overflow_count: 0,
        };
        let curr = WorkerCounters {
            noop_count: 8,
            steal_count: 2,
            poll_count: 1100,
            total_busy_duration_us: 1_500_000,
            local_schedule_count: 1100,
            overflow_count: 1,
        };
        let d = compute_worker_counter_deltas(Some(&prev), &curr);
        assert_eq!(d.noop_count, 3);
        assert_eq!(d.steal_count, 2);
        assert_eq!(d.poll_count, 100);
        assert_eq!(d.total_busy_duration_us, 1_000_000);
        assert_eq!(d.local_schedule_count, 100);
        assert_eq!(d.overflow_count, 1);
    }

    #[test]
    fn non_monotonic_clamps_to_zero() {
        // saturating_sub guards against any cumulative regression that
        // would otherwise wrap to a huge value.
        let prev = WorkerCounters {
            poll_count: 100,
            ..Default::default()
        };
        let curr = WorkerCounters {
            poll_count: 50,
            ..Default::default()
        };
        let d = compute_worker_counter_deltas(Some(&prev), &curr);
        assert_eq!(d.poll_count, 0);
    }
}

#[cfg(test)]
mod compute_runtime_counter_deltas_tests {
    use super::RuntimeCounters;
    use super::compute_runtime_counter_deltas;

    #[test]
    fn typical_deltas_are_field_wise_subtraction() {
        let prev = RuntimeCounters {
            budget_forced_yield_count: 5,
            remote_schedule_count: 1000,
        };
        let curr = RuntimeCounters {
            budget_forced_yield_count: 8,
            remote_schedule_count: 1100,
        };
        let d = compute_runtime_counter_deltas(&prev, &curr);
        assert_eq!(d.budget_forced_yield_count, 3);
        assert_eq!(d.remote_schedule_count, 100);
    }

    #[test]
    fn non_monotonic_clamps_to_zero() {
        let prev = RuntimeCounters {
            budget_forced_yield_count: 10,
            remote_schedule_count: 0,
        };
        let curr = RuntimeCounters {
            budget_forced_yield_count: 5,
            remote_schedule_count: 0,
        };
        let d = compute_runtime_counter_deltas(&prev, &curr);
        assert_eq!(d.budget_forced_yield_count, 0);
    }
}

#[cfg(test)]
mod dep_file_db_size_sampler_tests {
    use std::sync::Arc;

    use buck2_execute::dep_file_state::DepFileStore;
    use buck2_execute::dep_file_state::StoredDepFileDigests;
    use buck2_execute::dep_file_state::StoredDepFileState;

    use super::DepFileDbSizeSampler;

    struct NoopDepFileStore;

    impl DepFileStore for NoopDepFileStore {
        fn insert(&self, _logical_key: Vec<u8>, _config_key: Vec<u8>, _state: StoredDepFileState) {}

        fn delete(&self, _logical_key: Vec<u8>, _config_key: Vec<u8>) {}

        fn get_digests(&self, _logical_key: &[u8]) -> Vec<StoredDepFileDigests> {
            Vec::new()
        }

        fn get_entry(&self, _id: i64) -> Option<StoredDepFileState> {
            None
        }

        fn clear(&self) {}
    }

    /// The sampling task must not hold a strong reference back to the sampler. If it does, the
    /// strong count never reaches zero, `Drop` never runs, and the task is never aborted.
    #[tokio::test]
    async fn task_does_not_keep_the_sampler_alive() {
        let sampler = DepFileDbSizeSampler::start(
            Arc::new(NoopDepFileStore),
            &tokio::runtime::Handle::current(),
        );
        assert_eq!(1, Arc::strong_count(&sampler));
    }
}
