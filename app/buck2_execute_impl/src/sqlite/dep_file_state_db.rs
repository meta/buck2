/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! SQLite database for the local dep-file (local action) cache. Mirrors `incremental_state_db`;
//! see that module for the shared open/create/version-gating machinery.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;

use buck2_common::sqlite::sqlite_db::SqliteDb;
use buck2_common::sqlite::sqlite_db::SqliteIdentity;
use buck2_common::sqlite::sqlite_db::SqliteTable;
use buck2_common::sqlite::sqlite_db::SqliteTables;
use buck2_core::error::SoftErrorContext;
use buck2_core::error::capture_soft_error_context;
use buck2_core::error::with_soft_error_context;
use buck2_core::soft_error_or_panic;
use buck2_error::BuckErrorContext;
use buck2_execute::dep_file_state::DepFileDbSize;
use buck2_execute::dep_file_state::DepFileReadStats;
use buck2_execute::dep_file_state::DepFileStore;
use buck2_execute::dep_file_state::DepFileWriteStats;
use buck2_execute::dep_file_state::StoredDepFileDigests;
use buck2_execute::dep_file_state::StoredDepFileState;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::execute::blocking::BlockingExecutor;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_hash::IntentionallyStdHashMap;
use buck2_util::threads::thread_spawn;
use dupe::Dupe;

use crate::sqlite::tables::dep_file_state_table::DepFileStateSqliteTable;

/// Hand-maintained schema version for the dep-file state sqlite db.
/// PLEASE bump this version if you are making a breaking change to the schema!
///
/// If you forget to bump this version, you can fix forward by bumping the
/// `buck2.sqlite_dep_file_state_version` buckconfig in the project root's .buckconfig.
pub const DEP_FILE_DB_SCHEMA_VERSION: u64 = 2;

impl SqliteTable for DepFileStateSqliteTable {
    fn create_table(&self) -> buck2_error::Result<()> {
        DepFileStateSqliteTable::create_table(self)
    }
}

/// DB that owns the sqlite connection to the dep-file state db on disk.
pub struct DepFileStateSqliteDb {
    tables: SqliteTables<DepFileStateSqliteTable>,
    identity: SqliteIdentity,
    lifetime_guard: Option<Arc<dyn Send + Sync>>,
}

impl SqliteDb for DepFileStateSqliteDb {
    // Entries are loaded on demand per logical key (see `PersistedDepFileStore::get`), not read in
    // bulk at startup, so there is no eagerly-loaded state type.
    type StateType = ();
    type TableType = DepFileStateSqliteTable;

    fn new(tables: SqliteTables<Self::TableType>) -> buck2_error::Result<Self> {
        let identity = tables.get_identity()?;
        Ok(Self {
            tables,
            identity,
            lifetime_guard: None,
        })
    }

    fn open_tables(path: &AbsNormPath) -> buck2_error::Result<SqliteTables<Self::TableType>> {
        let connection = SqliteTables::<Self::TableType>::create_connection(path)?;
        // Reads get their own connections. Opening them here rather than lazily ties their lifetime
        // to the table, so a database that is deleted and recreated on a version mismatch cannot
        // leave a reader pointing at the old file.
        let dep_file_state_table =
            DepFileStateSqliteTable::new_with_read_connections(connection.dupe(), path);
        Ok(SqliteTables::new(dep_file_state_table, connection))
    }

    fn identity(&self) -> &SqliteIdentity {
        &self.identity
    }
}

impl DepFileStateSqliteDb {
    /// Open (or recreate) the dep-file state db. Entries are not read here: they are loaded on demand
    /// per logical action at lookup time (see `PersistedDepFileStore::get`). Any failure opening a
    /// compatible db (missing path, version mismatch, rejected identity) results in a fresh empty db.
    /// Returns the connected db.
    pub async fn initialize(
        dep_file_state_dir: AbsNormPathBuf,
        versions: IntentionallyStdHashMap<String, String>,
        current_instance_metadata: IntentionallyStdHashMap<String, String>,
        io_executor: Arc<dyn BlockingExecutor>,
        reject_identity: Option<&SqliteIdentity>,
        // Bound the db at startup: drop entries older than `prune_cutoff` (unix seconds) and, if
        // set, the oldest beyond `max_entries`. See `DepFileStateSqliteTable::prune`.
        prune_cutoff: Option<i64>,
        max_entries: Option<usize>,
    ) -> buck2_error::Result<Self> {
        let reject_identity = reject_identity.cloned();
        let guard = io_executor.lifetime_guard();
        let mut db = io_executor
            .execute_io_inline(move || {
                Self::initialize_dep_file_sqlite_db(
                    dep_file_state_dir,
                    versions,
                    current_instance_metadata,
                    reject_identity.as_ref(),
                    prune_cutoff,
                    max_entries,
                )
            })
            .await?;
        db.lifetime_guard = guard;
        Ok(db)
    }

    fn initialize_dep_file_sqlite_db(
        dep_file_state_dir: AbsNormPathBuf,
        versions: IntentionallyStdHashMap<String, String>,
        current_instance_metadata: IntentionallyStdHashMap<String, String>,
        reject_identity: Option<&SqliteIdentity>,
        prune_cutoff: Option<i64>,
        max_entries: Option<usize>,
    ) -> buck2_error::Result<Self> {
        match Self::get_sqlite_db(
            &dep_file_state_dir,
            &versions,
            current_instance_metadata.clone(),
            reject_identity,
        ) {
            Ok(db) => {
                // Bound the db so it does not grow without limit across sessions (a prune failure is
                // non-fatal). Cheap: it reads only the small scalar table, not the outputs.
                if let Err(e) = db.tables.domain_table.prune(prune_cutoff, max_entries) {
                    tracing::debug!("Failed to prune dep-file state: {}", e);
                }
                Ok(db)
            }
            Err(e) => {
                tracing::debug!("Failed to open dep-file state db. Recreating it. {}", e);
                Self::create_sqlite_db(dep_file_state_dir, versions, current_instance_metadata)
            }
        }
    }

    pub(crate) fn dep_file_state_table(&self) -> &DepFileStateSqliteTable {
        &self.tables.domain_table
    }
}

/// A queued mutation of the db. Writes are applied on a dedicated thread (see
/// [`PersistedDepFileStore`]), in the order they were issued.
struct DepFileWrite {
    context: Option<Arc<SoftErrorContext>>,
    operation: DepFileWriteOperation,
}

impl DepFileWrite {
    fn capture(operation: DepFileWriteOperation) -> Self {
        Self {
            context: capture_soft_error_context(),
            operation,
        }
    }

    fn with_context<R>(self, func: impl FnOnce(DepFileWriteOperation) -> R) -> R {
        with_soft_error_context(self.context, || func(self.operation))
    }
}

enum DepFileWriteOperation {
    Insert {
        logical_key: Vec<u8>,
        config_key: Vec<u8>,
        state: StoredDepFileState,
    },
    Delete {
        logical_key: Vec<u8>,
        config_key: Vec<u8>,
    },
    Clear,
    /// Acknowledged once every write queued before it has been applied.
    Flush(crossbeam_channel::Sender<()>),
}

/// Applies one write, returning what it did, or `None` for a `Flush`.
fn apply_write(db: &DepFileStateSqliteDb, write: DepFileWriteOperation) -> Option<WriteKind> {
    let table = db.dep_file_state_table();
    let (result, category, kind) = match write {
        DepFileWriteOperation::Insert {
            logical_key,
            config_key,
            state,
        } => (
            table.insert(logical_key, config_key, state),
            "insert_to_dep_file_db",
            WriteKind::Insert,
        ),
        DepFileWriteOperation::Delete {
            logical_key,
            config_key,
        } => (
            table.delete(&logical_key, &config_key),
            "delete_from_dep_file_db",
            WriteKind::Delete,
        ),
        DepFileWriteOperation::Clear => (table.clear(), "clear_dep_file_db", WriteKind::Clear),
        DepFileWriteOperation::Flush(ack) => {
            // Dropping the sender would also wake the waiter, so the send result is irrelevant.
            let _ignored = ack.send(());
            return None;
        }
    };
    if let Err(e) = result {
        soft_error_or_panic!(
            category,
            buck2_error::buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "Failed to write to dep-file sqlite db. {}",
                e
            )
        );
    }
    // A failed write still took time and still happened; the soft error above is what says it did
    // not land, so it is counted like any other.
    Some(kind)
}

fn report_read_failure(e: buck2_error::Error) {
    soft_error_or_panic!(
        "read_from_dep_file_db",
        buck2_error::buck2_error!(
            buck2_error::ErrorTag::Tier0,
            "Failed to read from dep-file sqlite db. {}",
            e
        )
    );
}

/// Write-through [`DepFileStore`] over a [`DepFileStateSqliteDb`]. Owned by the repo state once the
/// daemon opens the db. Every db error is downgraded to a quiet soft error so a database hiccup
/// never fails a build (the in-memory cache remains authoritative).
///
/// Writes are queued and applied on a dedicated thread rather than inline: they are issued once per
/// locally-executed action, and running them on the action's own thread would make every write
/// contend with concurrent lookups for the single connection mutex. Losing a queued write is
/// harmless -- it costs a cache miss in a later session -- but [`DepFileStore::flush`] is called at
/// the end of each command so a restart afterwards sees everything that command produced.
///
/// The queue is unbounded. Dropping writes on a full bounded queue would be consistent with losing
/// them being harmless, but it does not generalize: `clear` relies on its `Clear` reaching the
/// writer, since dropping that one leaves rows on disk that a later session would serve after the
/// user explicitly invalidated them. Bounding therefore needs one discipline for `Insert`/`Delete`
/// and another for `Clear`/`Flush`, and batching several rows into one transaction needs the `Flush`
/// acknowledgement to stay behind the rows it covers. `dep_file_db_queue_size` is reported per
/// snapshot so a queue that does grow is visible before either is built.
///
/// Reads do not wait for queued writes. They do not need to: the in-memory cache is consulted before
/// this store and already holds anything just written, and an entry read before its queued delete
/// lands is still re-validated against the action's digests before use.
///
/// They do run synchronously on the calling (async) thread, so a read that waits parks a runtime
/// worker. Readers take connections of their own, which is what WAL's concurrent readers need: a
/// read does not wait on the transaction a write holds. The connection is released before rows are
/// deserialized, and a read only runs when the in-memory cache misses.
/// Writer-thread counters. Atomic despite a single writer: the writer thread increments them while
/// the snapshot thread reads them once a second, so the sharing is across threads even though the
/// writes themselves are serialized. Grouped so the thread takes one handle rather than one per
/// counter, and so a new counter is added in one place.
#[derive(Default)]
struct WriteCounters {
    /// Writes accepted. Its difference from `applied` is the queue depth, reported per snapshot;
    /// the channel is unbounded, so this is the only signal that it is growing.
    queued: AtomicU64,
    /// Every message the writer handled, `Flush` included, so the difference above stays correct.
    applied: AtomicU64,
    /// Time spent applying writes, and the slowest single one: a total alone would only give a mean,
    /// and it is one slow write that stalls `flush` at the end of a command.
    duration_us: AtomicU64,
    max_us: AtomicU64,
    /// The writes that reached the database, split by what they did, so a slow insert and a slow
    /// prune-driven delete are distinguishable. The three counts sum to the writes `duration_us`
    /// covers; `applied` also counts `Flush`, which does no database work.
    inserts: AtomicU64,
    insert_duration_us: AtomicU64,
    deletes: AtomicU64,
    delete_duration_us: AtomicU64,
    clears: AtomicU64,
    clear_duration_us: AtomicU64,
}

/// What an applied write did. `Flush` has no variant: it touches no table.
#[derive(Copy, Clone)]
enum WriteKind {
    Insert,
    Delete,
    Clear,
}

impl WriteCounters {
    /// Records one applied message. `write` is `None` for a `Flush`, which does no database work.
    fn record(&self, write: Option<(WriteKind, u64)>) {
        self.applied.fetch_add(1, Ordering::Relaxed);
        if let Some((kind, elapsed)) = write {
            self.duration_us.fetch_add(elapsed, Ordering::Relaxed);
            self.max_us.fetch_max(elapsed, Ordering::Relaxed);
            let (count, duration) = match kind {
                WriteKind::Insert => (&self.inserts, &self.insert_duration_us),
                WriteKind::Delete => (&self.deletes, &self.delete_duration_us),
                WriteKind::Clear => (&self.clears, &self.clear_duration_us),
            };
            count.fetch_add(1, Ordering::Relaxed);
            duration.fetch_add(elapsed, Ordering::Relaxed);
        }
    }
}

/// Read counters, measured in the store rather than at the call site so they cover the wait for a
/// connection as well as the query. Grouped so that recording a read cannot update the count and
/// miss the duration.
#[derive(Default)]
struct ReadCounters {
    probes: AtomicU64,
    probe_duration_us: AtomicU64,
    fetches: AtomicU64,
    fetches_found: AtomicU64,
    fetch_duration_us: AtomicU64,
    /// Spans probes and fetches together: it exists to surface a single pathological read, and which
    /// half it came from does not change that.
    max_us: AtomicU64,
    /// Entries actually served, reported by the caller: a fetched row can still be rejected.
    hits: AtomicU64,
}

impl ReadCounters {
    fn record_probe(&self, elapsed: u64) {
        self.probes.fetch_add(1, Ordering::Relaxed);
        self.probe_duration_us.fetch_add(elapsed, Ordering::Relaxed);
        self.max_us.fetch_max(elapsed, Ordering::Relaxed);
    }

    fn record_fetch(&self, found: bool, elapsed: u64) {
        self.fetches.fetch_add(1, Ordering::Relaxed);
        if found {
            self.fetches_found.fetch_add(1, Ordering::Relaxed);
        }
        self.fetch_duration_us.fetch_add(elapsed, Ordering::Relaxed);
        self.max_us.fetch_max(elapsed, Ordering::Relaxed);
    }
}

pub struct PersistedDepFileStore {
    db: Arc<DepFileStateSqliteDb>,
    digest_config: DigestConfig,
    writes: crossbeam_channel::Sender<DepFileWrite>,
    /// Set once the writer thread is found to be gone, so the soft error is reported only once.
    writer_gone: AtomicBool,
    /// Shared with the writer thread, which is why this one is an `Arc`.
    write: Arc<WriteCounters>,
    read: ReadCounters,
}

impl PersistedDepFileStore {
    /// Fails only if the writer thread cannot be spawned.
    pub fn try_new(
        db: DepFileStateSqliteDb,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<Self> {
        let db = Arc::new(db);
        let (writes, receiver) = crossbeam_channel::unbounded::<DepFileWrite>();
        let writer_db = db.dupe();
        let write = Arc::new(WriteCounters::default());
        let writer_counters = write.dupe();
        // The thread exits when the last sender is dropped, i.e. when the store is dropped. The
        // daemon's store lives in a process-global `LateBinding`, so there it runs until the process
        // exits; the drop path is what lets tests reclaim the thread.
        thread_spawn("buck2-dep-file-db", move || {
            for write in receiver.iter() {
                let started = Instant::now();
                let kind = write.with_context(|operation| apply_write(&writer_db, operation));
                let applied =
                    kind.map(|kind| (kind, (Instant::now() - started).as_micros() as u64));
                writer_counters.record(applied);
            }
        })
        .buck_error_context("Failed to spawn the dep-file db writer thread")?;
        Ok(Self {
            db,
            digest_config,
            writes,
            writer_gone: AtomicBool::new(false),
            write,
            read: ReadCounters::default(),
        })
    }

    fn get_digests_inner(&self, logical_key: &[u8]) -> Vec<StoredDepFileDigests> {
        match self
            .db
            .dep_file_state_table()
            .read_digests_by_logical(logical_key, self.digest_config)
        {
            Ok(digests) => digests,
            Err(e) => {
                report_read_failure(e);
                Vec::new()
            }
        }
    }

    fn get_entry_inner(&self, id: i64) -> Option<StoredDepFileState> {
        match self
            .db
            .dep_file_state_table()
            .read_entry(id, self.digest_config)
        {
            Ok(state) => state,
            Err(e) => {
                report_read_failure(e);
                None
            }
        }
    }

    /// Queue a write. The channel only fails once the writer thread is gone (it panicked, since it
    /// otherwise lives as long as this store), after which every later write is dropped too. That
    /// silently disables persistence, so report it -- once, because the failure is permanent.
    fn queue(&self, operation: DepFileWriteOperation) {
        let write = DepFileWrite::capture(operation);
        // Counted only once accepted, so a rejected write does not leave `queued` permanently ahead of
        // `applied` and turn the depth gauge into a monotonic counter.
        if self.writes.send(write).is_ok() {
            self.write.queued.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if !self.writer_gone.swap(true, Ordering::Relaxed) {
            soft_error_or_panic!(
                "dep_file_db_writer_gone",
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "The dep-file sqlite writer thread is gone; persistence is disabled for the \
                     rest of this daemon's life"
                )
            );
        }
    }
}

impl DepFileStore for PersistedDepFileStore {
    fn insert(&self, logical_key: Vec<u8>, config_key: Vec<u8>, state: StoredDepFileState) {
        self.queue(DepFileWriteOperation::Insert {
            logical_key,
            config_key,
            state,
        });
    }

    fn delete(&self, logical_key: Vec<u8>, config_key: Vec<u8>) {
        self.queue(DepFileWriteOperation::Delete {
            logical_key,
            config_key,
        });
    }

    fn get_digests(&self, logical_key: &[u8]) -> Vec<StoredDepFileDigests> {
        let started = Instant::now();
        let result = self.get_digests_inner(logical_key);
        let elapsed = (Instant::now() - started).as_micros() as u64;
        self.read.record_probe(elapsed);
        result
    }

    fn get_entry(&self, id: i64) -> Option<StoredDepFileState> {
        let started = Instant::now();
        let result = self.get_entry_inner(id);
        let elapsed = (Instant::now() - started).as_micros() as u64;
        self.read.record_fetch(result.is_some(), elapsed);
        result
    }

    fn note_persisted_hit(&self) {
        self.read.hits.fetch_add(1, Ordering::Relaxed);
    }

    fn db_size(&self) -> Option<DepFileDbSize> {
        self.db.dep_file_state_table().measure()
    }

    fn read_stats(&self) -> DepFileReadStats {
        let (lock_wait, lock_wait_max) = self.db.dep_file_state_table().read_lock_wait();
        DepFileReadStats {
            probes: self.read.probes.load(Ordering::Relaxed),
            probe_duration_us: self.read.probe_duration_us.load(Ordering::Relaxed),
            fetches: self.read.fetches.load(Ordering::Relaxed),
            fetches_found: self.read.fetches_found.load(Ordering::Relaxed),
            fetch_duration_us: self.read.fetch_duration_us.load(Ordering::Relaxed),
            max_us: self.read.max_us.load(Ordering::Relaxed),
            hits: self.read.hits.load(Ordering::Relaxed),
            lock_wait_us: lock_wait,
            lock_wait_max_us: lock_wait_max,
            read_connections: self.db.dep_file_state_table().read_connections(),
        }
    }

    fn clear(&self) {
        // Queued (not applied inline) so it cannot overtake writes issued before it, then waited on:
        // the in-memory cache is cleared synchronously by the caller, so leaving rows on disk that a
        // lookup could still reach would defeat the invalidation. It is rare enough to block for.
        self.queue(DepFileWriteOperation::Clear);
        self.flush();
    }

    fn flush(&self) {
        let (ack, wait) = crossbeam_channel::bounded(1);
        self.queue(DepFileWriteOperation::Flush(ack));
        // Resolves either on acknowledgement or when the writer thread drops the sender.
        let _ignored = wait.recv();
    }

    fn write_stats(&self) -> DepFileWriteStats {
        DepFileWriteStats {
            applied: self.write.applied.load(Ordering::Relaxed),
            duration_us: self.write.duration_us.load(Ordering::Relaxed),
            max_us: self.write.max_us.load(Ordering::Relaxed),
            inserts: self.write.inserts.load(Ordering::Relaxed),
            insert_duration_us: self.write.insert_duration_us.load(Ordering::Relaxed),
            deletes: self.write.deletes.load(Ordering::Relaxed),
            delete_duration_us: self.write.delete_duration_us.load(Ordering::Relaxed),
            clears: self.write.clears.load(Ordering::Relaxed),
            clear_duration_us: self.write.clear_duration_us.load(Ordering::Relaxed),
        }
    }

    fn queue_size(&self) -> u64 {
        // Both counters only increase, so their difference is the depth. The writer can apply a write
        // before `queue` counts it, so the difference saturates at zero rather than underflowing.
        // Relaxed ordering makes it approximate, which is all a gauge needs.
        self.write
            .queued
            .load(Ordering::Relaxed)
            .saturating_sub(self.write.applied.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_write_preserves_soft_error_context() -> buck2_error::Result<()> {
        let context = Arc::new(SoftErrorContext::new("", "")?);
        let write = with_soft_error_context(Some(context.clone()), || {
            DepFileWrite::capture(DepFileWriteOperation::Clear)
        });

        let observed = write.with_context(|_| capture_soft_error_context());
        assert!(observed.is_some_and(|observed| Arc::ptr_eq(&context, &observed)));
        Ok(())
    }
}
