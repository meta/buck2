/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::VecDeque;
use std::env;
use std::path::Path;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;

use bytesize::ByteSize;
use pagable::ReadTimedOut;
use pagable::arc_erase::ArcEraseDyn;
use pagable::storage::data::DataKey;
use pagable::storage::data::PagableData;
use pagable::storage::traits::CommitFrontier;
use pagable::storage::traits::DeserializedArcCache;
use pagable::storage::traits::PagableStorage;
use pagable::storage::traits::WriteTicket;
use pagable::traits::StorageContext;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::ToSql;

const NUM_SHARDS: usize = 10;
const _: () = assert!(NUM_SHARDS <= WriteTicket::MAX_SHARDS);

/// How long a connection waits for a shard's file lock before its statement
/// fails with `SQLITE_BUSY`.
///
/// With `journal_mode=OFF` there is no write-ahead log, so while the page-out
/// writer commits a transaction it holds the shard's file exclusively, and
/// every reader of that shard waits: from the start of the commit until it
/// finishes, or from earlier still if the transaction's dirty pages outgrew
/// the writer's page cache and had to be written out before the commit. A read
/// that waits longer than this fails, and the page-in with it. rusqlite's
/// default is 5 seconds, and page-ins have failed that way in production when a
/// command started while a cancelled page-out was still committing what it had
/// queued. 10 seconds covers a commit of ordinary size; it is deliberately not
/// so long that a timeout stops being visible. `fetch_data_read` names these
/// timeouts distinctly so their rate in production can be read off.
///
/// The writer waits the same amount for readers to clear before it can take
/// the lock; reads are single-row lookups, so that side never comes close.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// A read whose wait for the shard's lock ran out is tried again this many
/// times, each waiting `BUSY_TIMEOUT` afresh, before it fails. A page-out
/// commit holds the lock for as long as the commit takes; the timeout covers
/// most commits and the retries the tail. Each retry is logged and counted,
/// so lengthening the timeout instead would hide how often the tail is hit.
const BUSY_READ_RETRIES: u32 = 2;

/// What SQLite reports when its wait for the shard's lock runs out.
fn busy_error() -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY), None)
}

fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(sqlite_error, _)
            if sqlite_error.code == rusqlite::ErrorCode::DatabaseBusy
    )
}

/// The error for a failed read of `key`, where `what` is the step that failed
/// and `waits` how many times the read waited for the shard's lock. A wait
/// that ran out is reported as such, apart from every other failure, so its
/// rate can be read off production errors.
fn sqlite_read_error(
    key: &DataKey,
    what: &str,
    error: rusqlite::Error,
    waits: u32,
) -> anyhow::Error {
    if is_busy(&error) {
        return anyhow::Error::new(ReadTimedOut {
            key: *key,
            waited: BUSY_TIMEOUT,
            attempts: waits,
            waiting_for: "the shard's lock (SQLITE_BUSY): a page-out commit was in progress",
        });
    }
    anyhow::anyhow!("{what} failed for key {key:?}: {error}")
}

const DEFAULT_WRITE_BUFFER_ROWS: usize = 1 << 20;
const DEFAULT_WRITE_BUFFER_BYTES: usize = 128 << 20;
/// Buffers usually rotate on bytes well before the row cap, so preallocate
/// modestly and let the vec grow.
const WRITE_BUFFER_PREALLOC_ROWS: usize = 32768;

/// Rows a producer buffers before publishing one write transaction.
/// `BUCK2_PAGABLE_WRITE_BUFFER_ROWS` overrides, as a measurement probe:
/// shrinking it reproduces large-table index write amplification on a small
/// target.
fn write_buffer_rows() -> usize {
    static ROWS: OnceLock<usize> = OnceLock::new();
    *ROWS.get_or_init(|| {
        parse_positive_env("BUCK2_PAGABLE_WRITE_BUFFER_ROWS", DEFAULT_WRITE_BUFFER_ROWS)
    })
}

/// A set-but-unusable value is a panic: these are measurement probes, and a
/// typo silently measuring the default would produce a wrong conclusion.
/// `default` 0 = off unless set.
fn parse_positive_env(var: &str, default: usize) -> usize {
    let Some(raw) = env::var_os(var) else {
        return default;
    };
    match raw.to_str().and_then(|v| v.parse::<usize>().ok()) {
        Some(v) if v > 0 => v,
        _ => panic!("{var}={raw:?} is not a positive integer"),
    }
}

/// Serialized bytes a producer buffers before publishing one write
/// transaction; `BUCK2_PAGABLE_WRITE_BUFFER_BYTES` overrides.
///
/// The row cap alone would let byte volume swing with row size, and
/// transaction size is what page-out cost scales by: content-hash keys land
/// uniformly across the `UNIQUE(key_hi, key_lo)` index, the batch sort writes
/// each touched index page once per transaction, so total index churn is
/// (rows / transaction-rows) passes over an index that grows with row count
/// (buffer-size sweep in the diff summary).
fn write_buffer_bytes() -> usize {
    static BYTES: OnceLock<usize> = OnceLock::new();
    *BYTES.get_or_init(|| {
        parse_positive_env(
            "BUCK2_PAGABLE_WRITE_BUFFER_BYTES",
            DEFAULT_WRITE_BUFFER_BYTES,
        )
    })
}
const IDLE_SPARE_WRITE_BUFFERS: usize = 1;
/// Buffers a shard may have published but not yet committed, the one being
/// written included; producers block on a full active buffer until the writer
/// is below it. Values whose rows sit in those buffers stay resident until they
/// commit, so this bounds retained memory as much as buffering: one buffer
/// filling and one draining per shard. `BUCK2_PAGABLE_QUEUE_DEPTH` overrides,
/// as a measurement probe.
const DEFAULT_QUEUE_DEPTH: usize = 1;

fn queue_depth() -> usize {
    static DEPTH: OnceLock<usize> = OnceLock::new();
    *DEPTH.get_or_init(|| parse_positive_env("BUCK2_PAGABLE_QUEUE_DEPTH", DEFAULT_QUEUE_DEPTH))
}
const INSERT_BATCH_ROWS: usize = 8;
const INSERT_COLUMNS: usize = 3;
const INSERT_SINGLE_SQL: &str =
    "INSERT OR IGNORE INTO pagable_data (key_lo, key_hi, value) VALUES (?1, ?2, ?3)";
const FETCH_SQL: &str = "SELECT value FROM pagable_data WHERE key_lo = ?1 AND key_hi = ?2";
const CREATE_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS pagable_data (
        key_lo INTEGER NOT NULL,
        key_hi INTEGER NOT NULL,
        value BLOB NOT NULL,
        UNIQUE(key_hi, key_lo)
    );";

/// SQLite-backed storage backend for pagable data.
///
/// This storage implementation persists paged-out data to SQLite database files.
/// It provides:
/// - Serialized data storage indexed by content-addressable `DataKey`
/// - Arc caching to avoid redundant deserialization
///
/// SQLite serializes writes per database file, so data is sharded by `DataKey`
/// to allow page-out workers to write independent files in parallel.
pub struct SqliteBackedPagableStorage {
    shards: Vec<Shard>,
    /// Shared with every shard; see [`ShardInner::next_seq`].
    next_seq: Arc<AtomicU64>,
    arcs: DeserializedArcCache,
    storage_context: StorageContext,
    /// Reads retried after waiting `BUSY_TIMEOUT` for a shard's lock.
    busy_read_retries: AtomicU64,
}

struct Shard {
    /// The inner state of the shard, shared between the writer thread and producers
    inner: Arc<ShardInner>,
    /// The writer thread that flushes pending writes to the database.
    writer: Option<JoinHandle<()>>,
}

struct ShardInner {
    /// Shared by every shard: sequence numbers order rows across the backend.
    next_seq: Arc<AtomicU64>,
    /// Asks the write in progress to commit what it has and stop, because the
    /// buffer it is writing was discarded.
    stop_write: AtomicBool,
    /// The connection pool for this shard.
    conns: ConnectionPool,
    /// Immutable insertion settings derived from the writer connection.
    insert: SqliteInsertConfig,
    /// The state of the writer thread.
    write_state: Mutex<ShardWriteState>,
    /// The condition variable to signal the writer thread.
    write_state_changed: Condvar,
}

struct SqliteInsertConfig {
    /// The number of rows inserted by one SQL statement execution.
    batch_rows: usize,
    /// The SQL statement used for a full batch.
    batch_sql: String,
    /// The writer connection's maximum permitted string, BLOB, or row size.
    sqlite_length_limit: usize,
}

struct ShardWriteState {
    max_rows: usize,
    max_bytes: usize,
    /// The buffer that producers enqueue writes into. Once full it's moved to pending_buffers
    active_buffer: Vec<(DataKey, Vec<u8>)>,
    /// First sequence number of each buffer holding rows not yet committed,
    /// oldest first. Pushed with a buffer's first row and popped when it
    /// commits, which stays in step only because a buffer is never published
    /// empty. A buffer that fails to write keeps its entry.
    unwritten_first_seqs: VecDeque<u64>,
    /// Payload bytes held in `active_buffer`. Keys and per-row index overhead
    /// are not counted; the bound is on the dominant, variable part.
    active_bytes: usize,
    /// Empty buffers available to replace active_buffer once it becomes pending.
    /// May grow dynamically during page-out; flush and release_memory trim extras.
    spare_buffers: Vec<Vec<(DataKey, Vec<u8>)>>,
    /// Buffers that are full and ready to be written to the database by the writer thread
    pending_buffers: VecDeque<Vec<(DataKey, Vec<u8>)>>,
    /// Published buffers not yet committed, the one being written included,
    /// before producers block; see [`DEFAULT_QUEUE_DEPTH`].
    queue_depth: usize,
    /// Whether the writer thread is currently writing one of the pending_buffers to the database
    writing: bool,
    /// Whether the writer thread should stop and exit
    shutdown: bool,
    /// Error message if the writer thread failed
    error: Option<String>,
}

impl ShardWriteState {
    fn trim_idle_write_buffers(&mut self) {
        while self.spare_buffers.len() > IDLE_SPARE_WRITE_BUFFERS {
            self.spare_buffers.pop();
        }
        // A buffer grown toward the byte cap can hold hundreds of thousands
        // of slots; give that back when idle. Only when empty: `shrink_to` on
        // a filled Vec reallocates and copies it.
        if self.active_buffer.is_empty() {
            self.active_buffer.shrink_to(WRITE_BUFFER_PREALLOC_ROWS);
        }
        for spare in &mut self.spare_buffers {
            spare.shrink_to(WRITE_BUFFER_PREALLOC_ROWS);
        }
    }

    fn queue_active_for_later_flush(&mut self) {
        debug_assert!(self.can_publish());
        // An empty buffer has no `unwritten_first_seqs` entry; publishing one
        // would make the writer pop the next buffer's entry when it commits.
        debug_assert!(!self.active_buffer.is_empty());
        let replacement = self
            .spare_buffers
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(WRITE_BUFFER_PREALLOC_ROWS));
        self.pending_buffers
            .push_back(std::mem::replace(&mut self.active_buffer, replacement));
        self.active_bytes = 0;
    }

    /// Whether another buffer may be published within the queue depth.
    fn can_publish(&self) -> bool {
        self.pending_buffers.len() + usize::from(self.writing) < self.queue_depth
    }

    /// Sequence number of the shard's oldest row not yet committed.
    fn oldest_unwritten_seq(&self) -> Option<u64> {
        self.unwritten_first_seqs.front().copied()
    }

    fn active_is_full(&self) -> bool {
        self.is_full_at(self.max_rows, self.max_bytes)
    }

    fn is_full_at(&self, max_rows: usize, max_bytes: usize) -> bool {
        self.active_buffer.len() >= max_rows || self.active_bytes >= max_bytes
    }
}

struct ConnectionPool {
    readwrite: Mutex<Connection>,
    /// `parking_lot`, for a lock that gives up after a time: a read does not wait for a
    /// reader connection without limit.
    readers: Vec<parking_lot::Mutex<Connection>>,
    next_reader: AtomicUsize,
    /// How long a read waits for the shard, in milliseconds: for a reader connection, and
    /// then for SQLite's lock on the file. `BUSY_TIMEOUT`, except in tests.
    read_timeout_ms: AtomicU64,
}

impl ConnectionPool {
    fn open(path: &Path, num_readers: usize) -> anyhow::Result<Self> {
        let writer = Connection::open(path)?;
        Self::init_pragmas(&writer)?;
        Self::init_writer_pragmas(&writer)?;
        writer.execute_batch(CREATE_TABLE_SQL)?;

        // Readers load the schema with their first statement and keep it. One
        // that loaded it before the table existed, or that first loads it while
        // the writer holds the file, fails its read with "no such table" rather
        // than waiting: SQLite's check for a changed schema gives up silently
        // when it cannot take the lock. So readers are opened after the table
        // exists and read it once now, while nothing can hold the file.
        let mut readers = Vec::with_capacity(num_readers);
        for _ in 0..num_readers {
            let reader = Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            Self::init_pragmas(&reader)?;
            reader.prepare_cached(FETCH_SQL)?;
            readers.push(parking_lot::Mutex::new(reader));
        }

        Ok(Self {
            readwrite: Mutex::new(writer),
            readers,
            next_reader: AtomicUsize::new(0),
            read_timeout_ms: AtomicU64::new(BUSY_TIMEOUT.as_millis() as u64),
        })
    }

    fn init_pragmas(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            "PRAGMA synchronous=OFF;
            PRAGMA journal_mode=OFF;
            PRAGMA page_size=8192;",
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(())
    }

    /// Pragmas for the writer connection only; readers keep the defaults.
    fn init_writer_pragmas(conn: &Connection) -> anyhow::Result<()> {
        // Measurement probe. The default page cache (~2MiB) is far below the
        // per-shard index working set at tens of millions of rows, so
        // transactions spill and rewrite the same index pages many times.
        // Writer-only: sizing every reader's cache too would multiply the
        // memory cost by the reader count per shard.
        // Cached so a bad value warns once, not once per shard connection.
        static CACHE_KB: OnceLock<usize> = OnceLock::new();
        let kb = *CACHE_KB.get_or_init(|| parse_positive_env("BUCK2_PAGABLE_CACHE_KB", 0));
        if kb > 0 {
            conn.execute_batch(&format!("PRAGMA cache_size=-{kb};"))?;
        }
        Ok(())
    }

    fn get_readwrite(&self) -> MutexGuard<'_, Connection> {
        self.readwrite.lock().expect("lock poisoned")
    }

    /// Round-robin with try_lock to skip connections held by other threads.
    /// Waits for a reader if all readers are busy, and gives up when the wait runs out.
    fn get_reader(&self) -> Option<parking_lot::MutexGuard<'_, Connection>> {
        let start = self.next_reader.fetch_add(1, Ordering::Relaxed);
        let len = self.readers.len();
        for i in 0..len {
            if let Some(guard) = self.readers[(start + i) % len].try_lock() {
                return Some(guard);
            }
        }
        let timeout = Duration::from_millis(self.read_timeout_ms.load(Ordering::Relaxed));
        self.readers[start % len].try_lock_for(timeout)
    }

    #[cfg(test)]
    fn set_read_timeout(&self, timeout: Duration) -> anyhow::Result<()> {
        self.read_timeout_ms
            .store(timeout.as_millis() as u64, Ordering::Relaxed);
        for reader in &self.readers {
            reader.lock().busy_timeout(timeout)?;
        }
        Ok(())
    }
}

impl Shard {
    fn open(
        path: &Path,
        num_readers: usize,
        shard_id: usize,
        queue_depth: usize,
        next_seq: Arc<AtomicU64>,
    ) -> anyhow::Result<Self> {
        let conns = ConnectionPool::open(path, num_readers)?;
        let insert = {
            let conn = conns.get_readwrite();
            SqliteInsertConfig::new(&conn, INSERT_BATCH_ROWS)?
        };
        let inner = Arc::new(ShardInner {
            conns,
            insert,
            next_seq,
            stop_write: AtomicBool::new(false),
            write_state: Mutex::new(ShardWriteState {
                max_rows: write_buffer_rows(),
                max_bytes: write_buffer_bytes(),
                active_buffer: Vec::with_capacity(WRITE_BUFFER_PREALLOC_ROWS),
                unwritten_first_seqs: VecDeque::new(),
                active_bytes: 0,
                spare_buffers: (0..IDLE_SPARE_WRITE_BUFFERS)
                    .map(|_| Vec::with_capacity(WRITE_BUFFER_PREALLOC_ROWS))
                    .collect(),
                pending_buffers: VecDeque::with_capacity(queue_depth),
                queue_depth,
                writing: false,
                shutdown: false,
                error: None,
            }),
            write_state_changed: Condvar::new(),
        });
        let writer_inner = inner.clone();
        let writer = std::thread::Builder::new()
            .name(format!("page-sql-{shard_id}"))
            .spawn(move || writer_inner.writer_loop())
            .map_err(|e| anyhow::anyhow!("failed to spawn sqlite page-out writer: {}", e))?;
        Ok(Self {
            inner,
            writer: Some(writer),
        })
    }

    fn flush(&self) -> anyhow::Result<()> {
        self.inner.flush()
    }

    #[inline]
    fn enqueue(&self, item: (DataKey, Vec<u8>)) -> anyhow::Result<u64> {
        self.inner.enqueue(item)
    }
}

impl Drop for Shard {
    fn drop(&mut self) {
        {
            let mut state = self.inner.write_state.lock().expect("lock poisoned");
            if !state.active_buffer.is_empty() {
                while !state.can_publish() && state.error.is_none() {
                    state = self
                        .inner
                        .write_state_changed
                        .wait(state)
                        .expect("lock poisoned");
                }
            }
            if !state.active_buffer.is_empty() && state.error.is_none() {
                state.queue_active_for_later_flush();
            }
            state.shutdown = true;
            self.inner.write_state_changed.notify_all();
        }
        if let Some(writer) = self.writer.take() {
            if let Err(payload) = writer.join() {
                eprintln!(
                    "Warning: sqlite page-out writer thread panicked: {}",
                    panic_payload_to_string(payload.as_ref())
                );
            }
        }
    }
}

impl ShardInner {
    fn flush(&self) -> anyhow::Result<()> {
        let mut state = self.write_state.lock().expect("lock poisoned");
        Self::check_error(&state)?;
        if !state.active_buffer.is_empty() {
            while !state.can_publish() {
                state = self.write_state_changed.wait(state).expect("lock poisoned");
                Self::check_error(&state)?;
            }
            if !state.active_buffer.is_empty() {
                state.queue_active_for_later_flush();
                self.write_state_changed.notify_all();
            }
        }
        while !state.pending_buffers.is_empty() || state.writing {
            state = self.write_state_changed.wait(state).expect("lock poisoned");
            Self::check_error(&state)?;
        }
        Self::check_error(&state)?;
        state.trim_idle_write_buffers();
        Ok(())
    }

    fn release_memory(&self) {
        let mut state = self.write_state.lock().expect("lock poisoned");
        state.trim_idle_write_buffers();
    }

    /// Buffers `item` and returns its sequence number, assigned under the
    /// state lock so the shard's rows are in sequence order.
    #[inline]
    fn enqueue(&self, item: (DataKey, Vec<u8>)) -> anyhow::Result<u64> {
        let mut state = self.write_state.lock().expect("lock poisoned");
        Self::check_error(&state)?;
        state = self.queue_full_active_for_later_flush(state)?;
        debug_assert!(!state.active_is_full());

        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        if state.active_buffer.is_empty() {
            state.unwritten_first_seqs.push_back(seq);
        }
        state.active_bytes += item.1.len();
        state.active_buffer.push(item);
        if !state.active_is_full() {
            return Ok(seq);
        }

        drop(self.queue_full_active_for_later_flush(state)?);
        Ok(seq)
    }

    fn oldest_unwritten_seq(&self) -> Option<u64> {
        self.write_state
            .lock()
            .expect("lock poisoned")
            .oldest_unwritten_seq()
    }

    /// See [`PagableStorage::discard_unwritten`]. The rows a cut write already
    /// inserted stay in the file as orphans of a content-addressed cache, which
    /// a later page-out finds already present.
    fn discard_unwritten(&self) {
        let mut guard = self.write_state.lock().expect("lock poisoned");
        let state = &mut *guard;
        if state.writing {
            self.stop_write.store(true, Ordering::Relaxed);
        }
        for mut buffer in state.pending_buffers.drain(..) {
            buffer.clear();
            state.spare_buffers.push(buffer);
        }
        state.active_buffer.clear();
        state.active_bytes = 0;
        state.unwritten_first_seqs.clear();
        self.write_state_changed.notify_all();
    }

    fn queue_full_active_for_later_flush<'a>(
        &self,
        mut state: MutexGuard<'a, ShardWriteState>,
    ) -> anyhow::Result<MutexGuard<'a, ShardWriteState>> {
        if !state.active_is_full() {
            return Ok(state);
        }

        while !state.can_publish() {
            state = self.write_state_changed.wait(state).expect("lock poisoned");
            Self::check_error(&state)?;
            if !state.active_is_full() {
                return Ok(state);
            }
        }

        if state.active_is_full() {
            state.queue_active_for_later_flush();
            // Blocked producers share the condvar with the writer; a single
            // wake-up could land on one of them and leave the writer asleep.
            self.write_state_changed.notify_all();
        }
        Ok(state)
    }

    fn writer_loop(&self) {
        let mut state = self.write_state.lock().expect("lock poisoned");
        loop {
            // Wait until producers publish a buffer, final flush asks us
            // to drain a partial one, or shutdown/error stops the writer.
            while state.pending_buffers.is_empty() && !state.shutdown && state.error.is_none() {
                state = self.write_state_changed.wait(state).expect("lock poisoned");
            }
            if state.error.is_some() || (state.pending_buffers.is_empty() && state.shutdown) {
                return;
            }
            state.writing = true;

            while state.error.is_none() && !state.pending_buffers.is_empty() {
                // Fetch a pending buffer for writing to the database.
                let mut items = state
                    .pending_buffers
                    .pop_front()
                    .expect("writer was woken with no pending work");
                // One freed pending slot can only unblock one producer.
                self.write_state_changed.notify_one();
                drop(state);

                // Write the items to the database, catching any panics so that we can clean up.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    self.write_items_impl(&mut items)
                }))
                .map_err(|payload| {
                    format!(
                        "sqlite page-out writer panicked: {}",
                        panic_payload_to_string(payload.as_ref())
                    )
                })
                .and_then(|result| result.map_err(|e| format!("{e:#}")));

                // Return the empty buffer back to the spares.
                items.clear();
                state = self.write_state.lock().expect("lock poisoned");
                state.spare_buffers.push(items);
                // A discard during the write already dropped this buffer's
                // frontier entry, whether or not the write finished.
                let discarded = self.stop_write.swap(false, Ordering::Relaxed);
                if let Err(e) = result {
                    state.error = Some(e);
                } else if !discarded {
                    state.unwritten_first_seqs.pop_front();
                }
            }

            state.writing = false;
            // Draining or error completion can satisfy every concurrent flush waiter.
            self.write_state_changed.notify_all();
        }
    }

    fn write_items_impl(&self, items: &mut [(DataKey, Vec<u8>)]) -> anyhow::Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        items.sort_unstable_by_key(|(key, _bytes)| key.get());

        let mut conn = self.conns.get_readwrite();
        let tx = conn.transaction()?;
        {
            self.insert.insert_items(&tx, items, &self.stop_write)?;
        }
        tx.commit()?;
        Ok(())
    }

    #[inline]
    fn check_error(state: &ShardWriteState) -> anyhow::Result<()> {
        match &state.error {
            Some(e) => Err(anyhow::anyhow!("sqlite page-out writer failed: {}", e)),
            None => Ok(()),
        }
    }
}

fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "<non-string panic payload>".to_owned()
    }
}

impl SqliteBackedPagableStorage {
    /// Creates a new SQLite-backed storage in the given directory.
    pub fn try_new(path: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(path)?;

        // Readers only serve page-in. Keep the total connection count bounded
        // now that writers are sharded across database files.
        let num_shards = NUM_SHARDS;
        let queue_depth = queue_depth();
        let next_seq = Arc::new(AtomicU64::new(1));
        let readers_per_shard = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .div_ceil(num_shards)
            .clamp(1, 2);
        let shards = (0..num_shards)
            .map(|i| {
                Shard::open(
                    &path.join(format!("pagable.{i}.db")),
                    readers_per_shard,
                    i,
                    queue_depth,
                    next_seq.clone(),
                )
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Self {
            shards,
            next_seq,
            arcs: DeserializedArcCache::new(),
            storage_context: StorageContext::new(),
            busy_read_retries: AtomicU64::new(0),
        })
    }

    /// How many reads so far waited `BUSY_TIMEOUT` for a shard's lock and were
    /// tried again.
    pub fn busy_read_retries(&self) -> u64 {
        self.busy_read_retries.load(Ordering::Relaxed)
    }

    #[inline]
    fn shard_index(&self, key: &DataKey) -> usize {
        (key.get() % self.shards.len() as u128) as usize
    }

    #[inline]
    fn shard_for(&self, key: &DataKey) -> &Shard {
        &self.shards[self.shard_index(key)]
    }

    fn fetch_data_read(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        let shard = self.shard_for(key);
        let (key_lo, key_hi) = data_key_parts(*key);
        let read_once = || -> Result<Option<Vec<u8>>, (&'static str, rusqlite::Error)> {
            // No reader connection in time. Reported as SQLite reports a lock it could not
            // get, so that it is retried and counted the same way.
            let conn = shard
                .inner
                .conns
                .get_reader()
                .ok_or_else(|| ("wait for a reader connection", busy_error()))?;
            let mut stmt = conn.prepare_cached(FETCH_SQL).map_err(|e| ("prepare", e))?;
            stmt.query_row(rusqlite::params![key_lo, key_hi], |row| row.get(0))
                .optional()
                .map_err(|e| ("fetch", e))
        };
        // Each attempt takes and releases a reader, so a retry does not hold
        // one across its wait.
        let read = || -> anyhow::Result<Option<Vec<u8>>> {
            let mut waits = 0;
            loop {
                match read_once() {
                    Ok(row) => return Ok(row),
                    Err((_, error)) if is_busy(&error) && waits < BUSY_READ_RETRIES => {
                        waits += 1;
                        self.busy_read_retries.fetch_add(1, Ordering::Relaxed);
                    }
                    Err((what, error)) => {
                        return Err(sqlite_read_error(key, what, error, waits + 1));
                    }
                }
            }
        };
        let bytes = match read()? {
            Some(bytes) => bytes,
            None => {
                // A returned key may still be in the shard's write buffers.
                // `read` releases its SQLite reader before waiting for the writer.
                shard.flush()?;
                read()?.ok_or_else(|| anyhow::anyhow!("fetch failed for key {:?}: no row", key))?
            }
        };
        Self::decode_pagable_data(&bytes, key)
    }

    fn encode_pagable_data(data: &PagableData) -> Vec<u8> {
        let bytes_size = 8 + 8 + data.data.len() + data.arcs.len() * 16;
        let mut bytes = Vec::with_capacity(bytes_size);
        bytes.extend_from_slice(&(data.data.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(data.arcs.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&data.data);
        bytes.extend_from_slice(bytemuck::cast_slice(&data.arcs));
        assert_eq!(bytes.len(), bytes_size);
        bytes
    }

    pub fn shrink_memory(&self) {
        for shard in &self.shards {
            if let Ok(conn) = shard.inner.conns.readwrite.lock() {
                let _unused = conn.execute_batch("PRAGMA shrink_memory;");
            }
            for reader in &shard.inner.conns.readers {
                let conn = reader.lock();
                let _unused = conn.execute_batch("PRAGMA shrink_memory;");
            }
        }
    }

    fn decode_pagable_data(bytes: &[u8], key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        if bytes.len() < 16 {
            return Err(anyhow::anyhow!(
                "corrupt sqlite entry for key {:?}: too short for header",
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
                    "corrupt sqlite entry for key {:?}: length overflow (data_len={}, arcs_len={})",
                    key,
                    data_len,
                    arcs_len
                )
            })?;
        if bytes.len() < expected_len {
            return Err(anyhow::anyhow!(
                "corrupt sqlite entry for key {:?}: expected {} bytes, got {}",
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
impl PagableStorage for SqliteBackedPagableStorage {
    fn arc_cache(&self) -> &DeserializedArcCache {
        &self.arcs
    }

    fn fetch_data_blocking(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        self.fetch_data_read(key)
    }

    #[cfg(any(feature = "tokio", test))]
    async fn fetch_data(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        self.fetch_data_read(key)
    }

    #[cfg(not(any(feature = "tokio", test)))]
    async fn fetch_data(&self, _key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        Err(anyhow::anyhow!("sqlite backend requires tokio feature"))
    }

    fn schedule_for_paging(&self, _arc: Box<dyn ArcEraseDyn>) {
        panic!("schedule_for_paging not implemented");
    }

    fn storage_context(&self) -> &StorageContext {
        &self.storage_context
    }

    fn store_data_ticketed(&self, data: PagableData) -> anyhow::Result<(DataKey, WriteTicket)> {
        let key = data.compute_key();
        let bytes = Self::encode_pagable_data(&data);
        let shard = self.shard_index(&key);
        let seq = self.shards[shard].enqueue((key, bytes))?;
        Ok((key, WriteTicket::new(shard, seq)))
    }

    fn commit_frontier(&self) -> CommitFrontier {
        // A shard with nothing unwritten has committed every row it was given
        // so far. Sequence numbers are assigned under the shard's state lock,
        // so a row enqueued after the shard is inspected gets at least the
        // value read here.
        let next_seq = self.next_seq.load(Ordering::Relaxed);
        CommitFrontier::new(
            self.shards
                .iter()
                .map(|shard| shard.inner.oldest_unwritten_seq().unwrap_or(next_seq))
                .collect(),
        )
    }

    fn discard_unwritten(&self) {
        for shard in &self.shards {
            shard.inner.discard_unwritten();
        }
    }

    fn flush(&self) -> anyhow::Result<()> {
        std::thread::scope(|scope| {
            let handles: Vec<_> = self
                .shards
                .iter()
                .map(|shard| scope.spawn(move || shard.flush().map(drop)))
                .collect();
            for handle in handles {
                handle.join().expect("flush thread panicked")?;
            }
            anyhow::Ok(())
        })?;
        Ok(())
    }

    fn release_memory(&self) {
        for shard in &self.shards {
            shard.inner.release_memory();
        }
        self.shrink_memory();
    }
}

fn data_key_parts(key: DataKey) -> (i64, i64) {
    ((key.get() as u64) as i64, ((key.get() >> 64) as u64) as i64)
}

fn sqlite_insert_batch_row_limit(conn: &Connection) -> anyhow::Result<usize> {
    let max_values: usize = conn
        .limit(rusqlite::limits::Limit::SQLITE_LIMIT_VARIABLE_NUMBER)?
        .try_into()
        .unwrap_or(999);
    Ok((max_values / INSERT_COLUMNS).max(1))
}

fn insert_sql(row_count: usize) -> String {
    let mut sql =
        String::from("INSERT OR IGNORE INTO pagable_data (key_lo, key_hi, value) VALUES ");
    for i in 0..row_count {
        if i > 0 {
            sql.push_str(", ");
        }
        sql.push_str("(?, ?, ?)");
    }
    sql
}

impl SqliteInsertConfig {
    fn new(conn: &Connection, requested_batch_rows: usize) -> anyhow::Result<Self> {
        let batch_rows = requested_batch_rows.min(sqlite_insert_batch_row_limit(conn)?);
        Ok(Self {
            batch_rows,
            batch_sql: insert_sql(batch_rows),
            sqlite_length_limit: conn
                .limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH)?
                .try_into()?,
        })
    }

    /// Inserts `items`, stopping after the current statement once `stop` is
    /// set. The transaction stays committable either way.
    fn insert_items(
        &self,
        tx: &rusqlite::Transaction<'_>,
        items: &[(DataKey, Vec<u8>)],
        stop: &AtomicBool,
    ) -> anyhow::Result<()> {
        if self.batch_rows <= 1 {
            let mut single_stmt = tx.prepare_cached(INSERT_SINGLE_SQL)?;
            for item in items {
                if stop.load(Ordering::Relaxed) {
                    return Ok(());
                }
                self.execute_insert(&mut single_stmt, std::slice::from_ref(item))?;
            }
            return Ok(());
        }

        let mut batch_stmt = tx.prepare_cached(&self.batch_sql)?;
        let mut chunks = items.chunks_exact(self.batch_rows);
        for chunk in &mut chunks {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            self.execute_insert(&mut batch_stmt, chunk)?;
        }
        let remainder = chunks.remainder();
        if !remainder.is_empty() {
            let mut single_stmt = tx.prepare_cached(INSERT_SINGLE_SQL)?;
            for item in remainder {
                if stop.load(Ordering::Relaxed) {
                    return Ok(());
                }
                self.execute_insert(&mut single_stmt, std::slice::from_ref(item))?;
            }
        }
        Ok(())
    }

    fn execute_insert(
        &self,
        stmt: &mut rusqlite::Statement<'_>,
        items: &[(DataKey, Vec<u8>)],
    ) -> anyhow::Result<()> {
        let key_parts = items
            .iter()
            .map(|(key, _bytes)| data_key_parts(*key))
            .collect::<Vec<_>>();
        let params: Vec<&dyn ToSql> = key_parts
            .iter()
            .zip(items)
            .flat_map(|((key_lo, key_hi), (_key, bytes))| {
                [
                    key_lo as &dyn ToSql,
                    key_hi as &dyn ToSql,
                    bytes as &dyn ToSql,
                ]
            })
            .collect();
        stmt.execute(rusqlite::params_from_iter(params))
            .map(drop)
            .map_err(|error| self.insert_error(error, items))
    }

    #[cold]
    fn insert_error(&self, error: rusqlite::Error, items: &[(DataKey, Vec<u8>)]) -> anyhow::Error {
        if matches!(
            &error,
            rusqlite::Error::SqliteFailure(sqlite_error, _)
                if sqlite_error.code == rusqlite::ErrorCode::TooBig
        ) {
            let Some((largest_key, largest_value_bytes)) = items
                .iter()
                .max_by_key(|(_key, bytes)| bytes.len())
                .map(|(key, bytes)| (key, bytes.len()))
            else {
                return anyhow::Error::new(error).context(format!(
                    "SQLite rejected an empty page-out insert; SQLITE_LIMIT_LENGTH is {} ({} bytes)",
                    ByteSize::b(self.sqlite_length_limit as u64).display().iec(),
                    self.sqlite_length_limit,
                ));
            };
            return anyhow::Error::new(error).context(format!(
                "SQLite rejected page-out insert containing {} item(s): largest serialized value in the failed batch is {} ({} bytes) for {largest_key:?}; SQLITE_LIMIT_LENGTH is {} ({} bytes)",
                items.len(),
                ByteSize::b(largest_value_bytes as u64).display().iec(),
                largest_value_bytes,
                ByteSize::b(self.sqlite_length_limit as u64).display().iec(),
                self.sqlite_length_limit,
            ));
        }
        error.into()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    use super::*;

    struct TempStorageDir {
        path: PathBuf,
    }

    impl TempStorageDir {
        fn new(test_name: &str) -> anyhow::Result<Self> {
            let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let path = std::env::temp_dir().join(format!(
                "pagable_storage_sqlite_{test_name}_{}_{}",
                std::process::id(),
                id
            ));
            std::fs::create_dir_all(&path)?;
            Ok(Self { path })
        }
    }

    impl Drop for TempStorageDir {
        fn drop(&mut self) {
            drop(std::fs::remove_dir_all(&self.path));
        }
    }

    fn pagable_data(data: &[u8], arcs: Vec<DataKey>) -> PagableData {
        PagableData {
            data: data.to_vec(),
            arcs,
        }
    }

    #[test]
    fn sqlite_reads_pending_rows_without_explicit_flush() -> anyhow::Result<()> {
        let dir = TempStorageDir::new("pending_read")?;
        let storage = SqliteBackedPagableStorage::try_new(&dir.path)?;
        let arcs = vec![DataKey::testing_new(17)];
        let key = storage.store_data(pagable_data(b"pending", arcs.clone()))?;
        assert_eq!(
            key,
            storage.store_data(pagable_data(b"pending", arcs.clone()))?
        );
        let fetched = storage.fetch_data_blocking(&key)?;
        assert_eq!(fetched.data, b"pending");
        assert_eq!(fetched.arcs, arcs);
        assert_eq!(shard_row_count(storage.shard_for(&key))?, 1);
        assert!(
            storage
                .fetch_data_blocking(&DataKey::testing_new(123))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn sqlite_pending_read_reports_writer_failure() -> anyhow::Result<()> {
        let dir = TempStorageDir::new("pending_read_failure")?;
        let storage = SqliteBackedPagableStorage::try_new(&dir.path)?;
        let key = storage.store_data(pagable_data(b"pending failure", Vec::new()))?;
        storage
            .shard_for(&key)
            .inner
            .write_state
            .lock()
            .unwrap()
            .error = Some("injected write failure".to_owned());
        let error = storage
            .fetch_data_blocking(&key)
            .err()
            .expect("pending row must report the writer failure");
        assert!(format!("{error:#}").contains("injected write failure"));
        Ok(())
    }

    /// A shard held exclusively, as it is while the writer commits, times a
    /// read out with a message that names the cause; the read succeeds once
    /// the lock is released.
    #[test]
    fn sqlite_read_reports_a_busy_timeout_distinctly() -> anyhow::Result<()> {
        let dir = TempStorageDir::new("busy_timeout")?;
        let storage = SqliteBackedPagableStorage::try_new(&dir.path)?;
        let key = storage.store_data(pagable_data(b"busy", Vec::new()))?;
        storage.flush()?;
        let shard = storage.shard_for(&key);
        // The production timeout would make this test take that long.
        shard
            .inner
            .conns
            .set_read_timeout(Duration::from_millis(50))?;

        let writer = shard.inner.conns.readwrite.lock().unwrap();
        writer.execute_batch("BEGIN EXCLUSIVE;")?;
        let error = storage
            .fetch_data_blocking(&key)
            .err()
            .expect("a read of an exclusively locked shard times out");
        writer.execute_batch("COMMIT;")?;
        drop(writer);
        assert!(
            format!("{error:#}").contains("SQLITE_BUSY"),
            "the timeout is named as such: {error:#}"
        );
        assert!(
            format!("{error:#}").contains("gave up after 3 waits"),
            "the error counts every wait: {error:#}"
        );
        assert_eq!(storage.busy_read_retries(), 2);
        assert_eq!(storage.fetch_data_blocking(&key)?.data, b"busy");
        Ok(())
    }

    /// A read that cannot get a reader connection gives up after the same
    /// waits as one that meets SQLite's lock, and the readers are usable
    /// afterwards.
    #[test]
    fn sqlite_read_gives_up_waiting_for_a_reader_connection() -> anyhow::Result<()> {
        let dir = TempStorageDir::new("reader_wait")?;
        let storage = SqliteBackedPagableStorage::try_new(&dir.path)?;
        let key = storage.store_data(pagable_data(b"waited", Vec::new()))?;
        storage.flush()?;
        let shard = storage.shard_for(&key);
        shard
            .inner
            .conns
            .set_read_timeout(Duration::from_millis(50))?;

        let readers = &shard.inner.conns.readers;
        let held: Vec<_> = readers.iter().map(|reader| reader.lock()).collect();
        let error = storage
            .fetch_data_blocking(&key)
            .err()
            .expect("a read gives up when every reader is held");
        drop(held);
        assert!(
            format!("{error:#}").contains("gave up after 3 waits"),
            "the error counts every wait: {error:#}"
        );
        assert!(pagable::is_transient_read_error(&error));
        assert_eq!(storage.busy_read_retries(), 2);
        assert_eq!(storage.fetch_data_blocking(&key)?.data, b"waited");
        Ok(())
    }

    /// A read that meets the lock while a commit is in progress succeeds once
    /// the commit is done, on one of its retries.
    #[test]
    fn sqlite_read_retries_after_a_busy_timeout() -> anyhow::Result<()> {
        let dir = TempStorageDir::new("busy_retry")?;
        let storage = Arc::new(SqliteBackedPagableStorage::try_new(&dir.path)?);
        let key = storage.store_data(pagable_data(b"retried", Vec::new()))?;
        storage.flush()?;
        let shard = storage.shard_for(&key);
        shard
            .inner
            .conns
            .set_read_timeout(Duration::from_millis(50))?;

        let writer = shard.inner.conns.readwrite.lock().unwrap();
        writer.execute_batch("BEGIN EXCLUSIVE;")?;
        let reader = std::thread::spawn({
            let storage = Arc::clone(&storage);
            move || storage.fetch_data_blocking(&key)
        });
        // Release the lock once the read has waited for it once, so the read
        // succeeds on a retry whatever the thread timing.
        while storage.busy_read_retries() == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        writer.execute_batch("COMMIT;")?;
        drop(writer);
        let data = reader
            .join()
            .expect("the reading thread does not panic")
            .expect("the read succeeds once the commit is done");
        assert_eq!(data.data, b"retried");
        assert!(storage.busy_read_retries() >= 1);
        Ok(())
    }

    fn shard_row_count(shard: &Shard) -> anyhow::Result<usize> {
        shard.flush()?;
        let conn = shard.inner.conns.readwrite.lock().expect("lock poisoned");
        Ok(
            conn.query_row("SELECT COUNT(*) FROM pagable_data", [], |row| {
                row.get::<_, i64>(0)
            })? as usize,
        )
    }

    /// A queued row is not covered by the frontier until its buffer commits.
    #[test]
    fn sqlite_frontier_covers_a_row_once_its_buffer_commits() -> anyhow::Result<()> {
        let dir = TempStorageDir::new("frontier")?;
        let storage = SqliteBackedPagableStorage::try_new(&dir.path)?;
        let (_key, ticket) = storage.store_data_ticketed(pagable_data(b"queued", Vec::new()))?;
        assert_ne!(ticket, WriteTicket::DURABLE);
        assert!(!storage.commit_frontier().covers(&ticket));
        assert!(storage.commit_frontier().covers(&WriteTicket::DURABLE));
        storage.flush()?;
        assert!(storage.commit_frontier().covers(&ticket));
        Ok(())
    }

    /// The evictor compares tickets against a frontier it took earlier; a row
    /// enqueued on an idle shard after that must not count as committed.
    #[test]
    fn sqlite_frontier_does_not_cover_rows_enqueued_after_it() -> anyhow::Result<()> {
        let dir = TempStorageDir::new("stale_frontier")?;
        let storage = SqliteBackedPagableStorage::try_new(&dir.path)?;
        let idle = storage.commit_frontier();
        let (_key, ticket) = storage.store_data_ticketed(pagable_data(b"later", Vec::new()))?;
        assert!(!idle.covers(&ticket));
        storage.flush()?;
        assert!(
            !idle.covers(&ticket),
            "a frontier describes the commits as of when it was taken"
        );
        assert!(storage.commit_frontier().covers(&ticket));
        Ok(())
    }

    /// Discarding drops queued rows; a later page-out writes them afresh.
    #[test]
    fn sqlite_discard_drops_queued_rows() -> anyhow::Result<()> {
        let dir = TempStorageDir::new("discard")?;
        let storage = SqliteBackedPagableStorage::try_new(&dir.path)?;
        let (key, ticket) = storage.store_data_ticketed(pagable_data(b"dropped", Vec::new()))?;
        storage.discard_unwritten();
        storage.flush()?;
        assert!(
            storage.fetch_data_blocking(&key).is_err(),
            "a discarded row is not in the file"
        );
        assert!(
            storage.commit_frontier().covers(&ticket),
            "nothing is left for the frontier to wait on"
        );
        let (again, _) = storage.store_data_ticketed(pagable_data(b"dropped", Vec::new()))?;
        storage.flush()?;
        assert_eq!(again, key);
        assert_eq!(storage.fetch_data_blocking(&key)?.data, b"dropped");
        Ok(())
    }

    /// An arc a page-out writes is bound to its key only once its row has
    /// committed and the caller asks, never by the write itself.
    #[test]
    fn sqlite_arcs_are_bound_once_their_rows_commit() -> anyhow::Result<()> {
        use pagable::PagableArc;
        use pagable::PagableSerialize;
        use pagable::storage::handle::PagableStorageHandle;
        use pagable::storage::support::SerializerForPaging;
        use pagable::storage::traits::ArcSerCache;
        use pagable::storage::traits::PageOutError;

        let dir = TempStorageDir::new("bind")?;
        let storage = Arc::new(SqliteBackedPagableStorage::try_new(&dir.path)?);
        let handle = PagableStorageHandle::new(storage.clone() as Arc<dyn PagableStorage>);
        let arc = PagableArc::new(vec![7u8; 64], handle);
        let context = storage.storage_context();
        let mut serializer = SerializerForPaging::new(context);
        1u8.pagable_serialize(&mut serializer)?;
        arc.pagable_serialize(&mut serializer)?;
        let (data, arcs) = serializer.finish();
        let finished = ArcSerCache::new();
        let (item_key, ticket) = storage
            .page_out_item_ticketed(data, arcs, &finished, context)
            .map_err(|error| match error {
                PageOutError::Failed(error) => error,
                PageOutError::AlreadyFailed => anyhow::anyhow!("a fresh cache cannot have failed"),
            })?;

        finished.bind_covered(&*storage, &storage.commit_frontier());
        assert!(!storage.commit_frontier().covers(&ticket));
        assert_eq!(
            ArcEraseDyn::data_key(&arc),
            None,
            "nothing binds before the commit"
        );
        storage.flush()?;
        assert_eq!(
            ArcEraseDyn::data_key(&arc),
            None,
            "committing alone binds nothing"
        );
        finished.bind_covered(&*storage, &storage.commit_frontier());
        let arc_key = storage.fetch_data_blocking(&item_key)?.arcs[0];
        assert_eq!(ArcEraseDyn::data_key(&arc), Some(arc_key));
        // The binding keeps the arc alive, and the arc keeps the storage alive,
        // until the sweep that follows a command.
        drop(arc);
        storage.arc_cache().evict_expired();
        Ok(())
    }

    #[test]
    fn sqlite_write_buffer_fills_on_bytes_or_rows() {
        let mut state = ShardWriteState {
            max_rows: 2,
            max_bytes: 100,
            active_buffer: Vec::new(),
            active_bytes: 0,
            unwritten_first_seqs: VecDeque::new(),
            spare_buffers: Vec::new(),
            pending_buffers: VecDeque::new(),
            queue_depth: 2,
            writing: false,
            shutdown: false,
            error: None,
        };
        state
            .active_buffer
            .push((DataKey::testing_new(1), Vec::new()));
        state.active_bytes = 99;
        assert!(
            !state.is_full_at(2, 100),
            "1 of 2 rows and 99 of 100 bytes is not full"
        );
        assert!(state.is_full_at(1, 100), "the row cap alone must rotate");
        assert!(state.is_full_at(2, 99), "the byte cap alone must rotate");
    }

    #[test]
    fn sqlite_enqueue_accumulates_payload_bytes() -> anyhow::Result<()> {
        let dir = TempStorageDir::new("enqueue_bytes")?;
        let storage = SqliteBackedPagableStorage::try_new(&dir.path)?;
        let shard = &storage.shards[0];
        {
            let mut state = shard.inner.write_state.lock().unwrap();
            state.max_rows = 10;
            state.max_bytes = 19;
        }

        shard.enqueue((DataKey::testing_new(1), vec![0u8; 7]))?;
        shard.enqueue((DataKey::testing_new(2), vec![0u8; 11]))?;

        let state = shard.inner.write_state.lock().expect("lock poisoned");
        assert_eq!(
            18, state.active_bytes,
            "active_bytes must be the payload lengths (7 + 11), or byte \
             rotation silently degrades to row-only"
        );
        assert_eq!(2, state.active_buffer.len());
        drop(state);
        shard.enqueue((DataKey::testing_new(3), vec![0u8; 1]))?;
        let state = shard.inner.write_state.lock().unwrap();
        assert_eq!(
            0, state.active_bytes,
            "reaching the byte cap rotates the buffer"
        );
        assert!(state.active_buffer.is_empty());
        drop(state);
        assert_eq!(shard_row_count(shard)?, 3);
        Ok(())
    }

    #[test]
    fn sqlite_write_buffer_rotation_allocates_replacement_when_no_spare_exists() {
        let queue_depth = 2;
        let mut state = ShardWriteState {
            max_rows: DEFAULT_WRITE_BUFFER_ROWS,
            max_bytes: DEFAULT_WRITE_BUFFER_BYTES,
            active_buffer: Vec::with_capacity(WRITE_BUFFER_PREALLOC_ROWS),
            active_bytes: 0,
            unwritten_first_seqs: VecDeque::new(),
            spare_buffers: Vec::new(),
            pending_buffers: VecDeque::with_capacity(queue_depth),
            queue_depth,
            writing: false,
            shutdown: false,
            error: None,
        };

        state.active_buffer.extend(
            (0..WRITE_BUFFER_PREALLOC_ROWS)
                .map(|i| (DataKey::testing_new((i + 1) as u128), Vec::new())),
        );
        state.active_bytes = 1;
        state.queue_active_for_later_flush();

        assert_eq!(1, state.pending_buffers.len());
        assert!(state.active_buffer.is_empty());
        assert_eq!(
            0, state.active_bytes,
            "rotation should reset the byte count for the fresh buffer"
        );
        assert!(
            state.active_buffer.capacity() >= WRITE_BUFFER_PREALLOC_ROWS,
            "rotation should leave active with enough capacity for the next writes",
        );
    }

    #[test]
    fn sqlite_flush_trims_hot_write_buffers() -> anyhow::Result<()> {
        let dir = TempStorageDir::new("trim_hot_buffers")?;
        let storage = SqliteBackedPagableStorage::try_new(&dir.path)?;
        let shard = &storage.shards[0];

        {
            let mut state = shard.inner.write_state.lock().expect("lock poisoned");
            state
                .spare_buffers
                .push(Vec::with_capacity(WRITE_BUFFER_PREALLOC_ROWS));
            state
                .spare_buffers
                .push(Vec::with_capacity(WRITE_BUFFER_PREALLOC_ROWS));
            assert!(state.spare_buffers.len() > IDLE_SPARE_WRITE_BUFFERS);
        }

        shard.flush()?;

        let state = shard.inner.write_state.lock().expect("lock poisoned");
        assert_eq!(IDLE_SPARE_WRITE_BUFFERS, state.spare_buffers.len());
        Ok(())
    }

    #[test]
    fn sqlite_batch_insert_handles_boundary_cases() -> anyhow::Result<()> {
        for (case, item_count, batch_rows) in [
            ("empty input", 0, 8),
            ("single-row fallback", 3, 1),
            ("exact batch", 32, 16),
            ("tail rows", 35, 32),
        ] {
            let mut conn = Connection::open_in_memory()?;
            conn.execute_batch(
                "CREATE TABLE pagable_data (
                    key_lo INTEGER NOT NULL,
                    key_hi INTEGER NOT NULL,
                    value BLOB NOT NULL,
                    UNIQUE(key_hi, key_lo)
                );",
            )?;
            let insert = SqliteInsertConfig::new(&conn, batch_rows)?;
            let tx = conn.transaction()?;
            let items = (0..item_count)
                .map(|i| (DataKey::testing_new((i + 1) as u128 + 1), vec![i as u8]))
                .collect::<Vec<_>>();
            insert.insert_items(&tx, &items, &AtomicBool::new(false))?;
            tx.commit()?;

            let row_count: usize =
                conn.query_row("SELECT COUNT(*) FROM pagable_data", [], |row| {
                    row.get::<_, i64>(0)
                })? as usize;
            assert_eq!(
                item_count, row_count,
                "{case}: every queued item should be inserted exactly once",
            );
            for (key, expected_value) in &items {
                let (key_lo, key_hi) = data_key_parts(*key);
                let value: Vec<u8> = conn.query_row(
                    "SELECT value FROM pagable_data WHERE key_lo = ?1 AND key_hi = ?2",
                    rusqlite::params![key_lo, key_hi],
                    |row| row.get(0),
                )?;
                assert_eq!(
                    expected_value, &value,
                    "{case}: stored value should match the value bound for {key:?}",
                );
            }
        }
        Ok(())
    }

    #[test]
    fn sqlite_sharded_storage_round_trips_data_and_arcs() -> anyhow::Result<()> {
        let dir = TempStorageDir::new("sharded_round_trip")?;
        let storage = SqliteBackedPagableStorage::try_new(&dir.path)?;

        let mut expected = Vec::new();
        for i in 0..128 {
            let data = pagable_data(
                format!("serialized payload {i}").as_bytes(),
                vec![DataKey::testing_new(i + 1), DataKey::testing_new(i + 1000)],
            );
            let expected_data = data.data.clone();
            let expected_arcs = data.arcs.clone();
            let key = storage.store_data(data)?;
            expected.push((key, expected_data, expected_arcs));
        }
        storage.flush()?;

        let nonempty_shards = storage
            .shards
            .iter()
            .map(shard_row_count)
            .collect::<anyhow::Result<Vec<_>>>()?
            .into_iter()
            .filter(|count| *count > 0)
            .count();
        assert!(
            nonempty_shards > 1,
            "expected writes to use multiple shards"
        );

        for (key, expected_data, expected_arcs) in expected {
            let fetched = storage.fetch_data_blocking(&key)?;
            assert_eq!(expected_data, fetched.data);
            assert_eq!(expected_arcs, fetched.arcs);
        }
        Ok(())
    }

    #[test]
    fn sqlite_oversized_insert_reports_value_size_and_limit() -> anyhow::Result<()> {
        const SQLITE_LENGTH_LIMIT: i32 = 256;
        const VALUE_BYTES: usize = 512;

        let mut conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE pagable_data (
                key_lo INTEGER NOT NULL,
                key_hi INTEGER NOT NULL,
                value BLOB NOT NULL,
                UNIQUE(key_hi, key_lo)
            );",
        )?;
        conn.set_limit(
            rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
            SQLITE_LENGTH_LIMIT,
        )?;
        let insert = SqliteInsertConfig::new(&conn, 1)?;
        let tx = conn.transaction()?;
        let data_key = DataKey::testing_new(1);
        let items = vec![(data_key, vec![0; VALUE_BYTES])];
        let error = insert
            .insert_items(&tx, &items, &AtomicBool::new(false))
            .expect_err("SQLite should reject a BLOB larger than SQLITE_LIMIT_LENGTH");
        let message = format!("{error:#}");

        assert!(
            message.contains("largest serialized value in the failed batch is 512 B (512 bytes)"),
            "missing value size in error: {message}",
        );
        assert!(
            message.contains(&format!("{data_key:?}")),
            "missing data key in error: {message}",
        );
        assert!(
            message.contains("SQLITE_LIMIT_LENGTH is 256 B (256 bytes)"),
            "missing SQLite limit in error: {message}",
        );
        assert!(
            message.contains("string or blob too big"),
            "missing SQLite source error: {message}",
        );
        Ok(())
    }

    #[test]
    fn sqlite_sharded_storage_ignores_duplicate_keys() -> anyhow::Result<()> {
        let dir = TempStorageDir::new("duplicate_keys")?;
        let storage = SqliteBackedPagableStorage::try_new(&dir.path)?;

        let data = pagable_data(b"duplicate payload", vec![DataKey::testing_new(11)]);
        let key = data.compute_key();
        assert_eq!(key, storage.store_data(data)?);
        assert_eq!(
            key,
            storage.store_data(pagable_data(
                b"duplicate payload",
                vec![DataKey::testing_new(11)]
            ))?
        );
        storage.flush()?;

        let row_count = storage
            .shards
            .iter()
            .map(shard_row_count)
            .try_fold(0, |acc, count| count.map(|count| acc + count))?;
        assert_eq!(1, row_count);

        let fetched = storage.fetch_data_blocking(&key)?;
        assert_eq!(b"duplicate payload", fetched.data.as_slice());
        assert_eq!(vec![DataKey::testing_new(11)], fetched.arcs);
        Ok(())
    }

    #[test]
    fn sqlite_storage_drop_drains_unflushed_active_buffer() -> anyhow::Result<()> {
        let dir = TempStorageDir::new("drop_drains")?;
        let key = {
            let storage = SqliteBackedPagableStorage::try_new(&dir.path)?;
            storage.store_data(pagable_data(
                b"drop drains payload",
                vec![DataKey::testing_new(7)],
            ))?
        };

        let shard_id = (key.get() % NUM_SHARDS as u128) as usize;
        let conn = Connection::open(dir.path.join(format!("pagable.{shard_id}.db")))?;
        let (key_lo, key_hi) = data_key_parts(key);
        let value: Vec<u8> = conn.query_row(
            "SELECT value FROM pagable_data WHERE key_lo = ?1 AND key_hi = ?2",
            rusqlite::params![key_lo, key_hi],
            |row| row.get(0),
        )?;
        let fetched = SqliteBackedPagableStorage::decode_pagable_data(&value, &key)?;
        assert_eq!(b"drop drains payload", fetched.data.as_slice());
        assert_eq!(vec![DataKey::testing_new(7)], fetched.arcs);
        Ok(())
    }
}
