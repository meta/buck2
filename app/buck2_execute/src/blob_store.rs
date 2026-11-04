/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The BlobStore: where the blobs behind the daemon's tracked file digests are, and how to get
//! them back. It is the digest interner with its backings made explicit, a recorded CAS
//! expiration and a recorded local entry; the two places this daemon keeps content the CAS is
//! not known to hold, the stash directory under buck-out and compressed bytes in memory; and one
//! walk over the table that refreshes CAS expirations. Where a digest's entry says the store
//! holds the blob, the materializer puts the content on disk from there before asking the CAS.
//!
//! The stash and the memory hold exactly the blobs some `TrackedFileDigest` is held for and the
//! CAS is not known to hold, and start empty with every daemon. The stash takes content that is
//! on disk already, an action's output, by the cheapest link the filesystem has; the memory takes
//! content an action made and nothing has asked for on disk, a deferred write. Either insertion
//! puts the content under a nonce of its own and commits by recording it on the digest's entry in
//! one atomic step; a second insertion of the same digest finds the record and removes its own.
//! The record leaves the entry when the entry leaves the table, at the last handle's drop or at a
//! walk, or when a CAS expiration is recorded for the blob, and the interner hands it to the
//! collector of whichever place made it, by the id in its nonce: the stash's unlinks the file,
//! the memory's drops the bytes. Nothing coordinates beyond that, and nothing reads the
//! directory: what the table does not record is in neither.

use std::collections::hash_map::RandomState;
use std::fs::File;
use std::hash::BuildHasher;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use allocative::Allocative;
use buck2_common::cas_digest::CasDigestConfig;
use buck2_common::cas_digest::DigestInterner;
use buck2_common::cas_digest::RawDigest;
use buck2_common::cas_digest::StashRecord;
use buck2_common::cas_digest::SweepStats;
use buck2_common::cas_digest::TrackedFileDigest;
use buck2_common::cas_digest::Verdict;
use buck2_common::cas_digest::file_digest_interner;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_fs::fs_util::Linked;
use buck2_fs::fs_util::uncategorized as fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckDashMap;
use dupe::Dupe;
use jiff::Timestamp;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::MissedTickBehavior;

use crate::execute::blocking::BlockingExecutor;
use crate::re::ttl_refresh::StandaloneTtlRefreshCounters;
use crate::re::ttl_refresh::TtlRefresher;

/// Where the content of an insertion comes from, which decides what the store may do with it.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Origin {
    /// Content this daemon produced in buck-out. Nothing writes it in place, so the entry may
    /// share its inode.
    BuckOut,
    /// Content a user controls, a source file. Its digest is checked, since the file may have
    /// changed since the digest was computed, and the entry never shares its inode.
    Source,
}

/// A file to put into the stash.
#[derive(Debug)]
pub struct FileToInsert {
    pub path: AbsNormPathBuf,
    pub digest: TrackedFileDigest,
    pub executable: bool,
    pub origin: Origin,
}

/// A blob to put on disk out of the store.
#[derive(Debug)]
pub struct FileToLink {
    pub digest: TrackedFileDigest,
    pub executable: bool,
    /// Must not exist; the directories on the way to it are made as needed.
    pub dest: AbsNormPathBuf,
}

/// What the interner hands the collector: a record whose entry no longer carries it; or, for
/// tests, a request to be told once everything handed over before it is unlinked.
enum Collect {
    Orphan(FileDigest, StashRecord),
    Flush(oneshot::Sender<()>),
}

/// The store's two places as the snapshot reports them.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct BlobStoreStats {
    /// Files in the stash and their bytes, as insertions and the collector keep count.
    pub entries: u64,
    pub bytes: u64,
    /// What the collector has unlinked since this process started.
    pub removed_bytes: u64,
    /// Insertions whose IO ran; among them, the ones that found the digest recorded by then and
    /// gave their file up.
    pub insertions: u64,
    pub duplicates: u64,
    /// Files put at their destinations out of the stash since this process started.
    pub served: u64,
    /// Blobs in memory and their bytes as compressed, as insertions and the collector keep
    /// count.
    pub memory_entries: u64,
    pub memory_bytes: u64,
    /// Files written at their destinations out of memory since this process started.
    pub memory_served: u64,
}

#[derive(buck2_error::Error, Debug)]
enum BlobStoreError {
    /// A source file whose content is not what the build was told: the user's doing.
    #[error("`{path}` has digest {found}, not the {expected} it was inserted as")]
    #[buck2(tag = Input)]
    ContentChanged {
        path: AbsNormPathBuf,
        expected: FileDigest,
        found: FileDigest,
    },
    #[error("The BlobStore has no stash to put content in")]
    #[buck2(tag = Tier0)]
    NoStash,
    #[error("`{digest}` is not the digest of the {len} bytes offered as its content")]
    #[buck2(tag = Tier0)]
    WrongContent { digest: FileDigest, len: usize },
}

#[derive(Allocative)]
pub struct BlobStore {
    #[allocative(skip)]
    interner: &'static DigestInterner,
    /// The directory, once the daemon has given the store one.
    stash: OnceLock<Arc<AttachedStash>>,
    memory: Arc<Memory>,
    /// The CAS side of the walk, once the daemon has an RE connection to refresh through.
    #[allocative(skip)]
    refresh: OnceLock<TtlRefresher>,
    counters: Arc<Counters>,
}

#[derive(Allocative)]
struct AttachedStash {
    stash: Stash,
    #[allocative(skip)]
    blocking: Arc<dyn BlockingExecutor>,
    /// The collector's inbox: the records the interner hands over, to be unlinked.
    #[allocative(skip)]
    collector: mpsc::UnboundedSender<Collect>,
}

#[derive(Default, Allocative)]
struct Counters {
    entries: AtomicU64,
    bytes: AtomicU64,
    removed_bytes: AtomicU64,
    insertions: AtomicU64,
    duplicates: AtomicU64,
    served: AtomicU64,
    memory_entries: AtomicU64,
    memory_bytes: AtomicU64,
    memory_served: AtomicU64,
}

impl Counters {
    fn recorded(&self, bytes: u64) {
        self.entries.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// A record gone, its file unlinked or found gone.
    fn forgotten(&self, bytes: u64) {
        saturating_sub(&self.entries, 1);
        saturating_sub(&self.bytes, bytes);
    }

    fn unlinked(&self, bytes: u64) {
        self.removed_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    fn memory_recorded(&self, compressed_bytes: u64) {
        self.memory_entries.fetch_add(1, Ordering::Relaxed);
        self.memory_bytes
            .fetch_add(compressed_bytes, Ordering::Relaxed);
    }

    fn memory_forgotten(&self, compressed_bytes: u64) {
        saturating_sub(&self.memory_entries, 1);
        saturating_sub(&self.memory_bytes, compressed_bytes);
    }
}

fn saturating_sub(counter: &AtomicU64, n: u64) {
    let _ignored = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
        Some(v.saturating_sub(n))
    });
}

static GLOBAL: OnceLock<BlobStore> = OnceLock::new();

impl BlobStore {
    /// The process-wide store, over the process-wide interner.
    pub fn global() -> &'static BlobStore {
        GLOBAL.get_or_init(|| BlobStore::new(file_digest_interner()))
    }

    pub fn new(interner: &'static DigestInterner) -> Self {
        let memory = Arc::new(Memory::new());
        let counters = Arc::new(Counters::default());
        interner.add_stash_collector(memory.nonces.id, {
            let (memory, counters) = (memory.dupe(), counters.dupe());
            Box::new(move |_digest, record| {
                if let Some(blob) = memory.remove(record) {
                    counters.memory_forgotten(blob.len() as u64);
                }
            })
        });
        Self {
            interner,
            stash: OnceLock::new(),
            memory,
            refresh: OnceLock::new(),
            counters,
        }
    }

    /// Gives the store its directory, and the executor it does its IO on, and starts the
    /// directory empty: what a previous daemon left there is moved aside and removed in the
    /// background, since nothing refers to it any more. Returns whether the store took the
    /// directory: the store has one, and a second materializer in the process shares the first's.
    /// Must be called from within a tokio runtime, which the collector's task then lives on.
    pub fn attach_stash(
        &self,
        root: AbsNormPathBuf,
        config: CasDigestConfig,
        blocking: Arc<dyn BlockingExecutor>,
    ) -> bool {
        if self.stash.get().is_some() {
            return false;
        }
        discard_stash(&root, blocking.dupe());
        let (sender, receiver) = mpsc::unbounded_channel();
        let attached = Arc::new(AttachedStash {
            stash: Stash::new(root, config),
            blocking,
            collector: sender,
        });
        if self.stash.set(attached.dupe()).is_err() {
            return false;
        }
        let inbox = attached.collector.clone();
        self.interner.add_stash_collector(
            attached.stash.nonces.id,
            Box::new(move |digest, record| {
                let _ignored = inbox.send(Collect::Orphan(digest.dupe(), record));
            }),
        );
        tokio::spawn(collect(receiver, attached, self.counters.dupe()));
        true
    }

    /// Starts the walk on `frequency`, refreshing through `refresher`, and returns the counters
    /// the refreshes report into. Must be called from within a tokio runtime, which the walk's
    /// task then lives on; a second call changes nothing.
    pub fn start_refresh(
        &'static self,
        refresher: TtlRefresher,
        frequency: Duration,
    ) -> Arc<StandaloneTtlRefreshCounters> {
        let counters = refresher.counters();
        if self.refresh.set(refresher).is_err() {
            return self.refresh.get().expect("set").counters();
        }
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval_at(tokio::time::Instant::now() + frequency, frequency);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if let Err(e) = self.walk().await {
                    tracing::info!("BlobStore walk failed: {:#}", e);
                }
            }
        });
        counters
    }

    pub fn stats(&self) -> BlobStoreStats {
        BlobStoreStats {
            entries: self.counters.entries.load(Ordering::Relaxed),
            bytes: self.counters.bytes.load(Ordering::Relaxed),
            removed_bytes: self.counters.removed_bytes.load(Ordering::Relaxed),
            insertions: self.counters.insertions.load(Ordering::Relaxed),
            duplicates: self.counters.duplicates.load(Ordering::Relaxed),
            served: self.counters.served.load(Ordering::Relaxed),
            memory_entries: self.counters.memory_entries.load(Ordering::Relaxed),
            memory_bytes: self.counters.memory_bytes.load(Ordering::Relaxed),
            memory_served: self.counters.memory_served.load(Ordering::Relaxed),
        }
    }

    /// Waits until every record handed to the collector so far is unlinked. For tests; the
    /// daemon never waits for the collector.
    pub async fn flush_collector(&self) {
        let Some(attached) = self.stash.get() else {
            return;
        };
        let (done, flushed) = oneshot::channel();
        if attached.collector.send(Collect::Flush(done)).is_ok() {
            let _ignored = flushed.await;
        }
    }

    /// When the CAS last said it holds `digest`, if it ever did: the table's record, whether or
    /// not anything holds the digest now.
    pub fn cas_expiry(&self, digest: &FileDigest) -> Option<Timestamp> {
        let entry = self.interner.peek(digest)?;
        if entry.expires_secs <= 0 {
            return None;
        }
        Timestamp::from_second(entry.expires_secs).ok()
    }

    fn attached(&self) -> buck2_error::Result<&AttachedStash> {
        self.stash
            .get()
            .map(|attached| &**attached)
            .ok_or_else(|| BlobStoreError::NoStash.into())
    }

    /// Puts the files in the stash as the content of their digests. A digest the stash holds
    /// already, or the CAS is known to, is left alone. Returns once every file is in.
    pub async fn insert_files(&self, files: Vec<FileToInsert>) -> buck2_error::Result<()> {
        let attached = self.attached()?;
        let (stash, counters) = (&attached.stash, &*self.counters);
        attached
            .blocking
            .execute_io_inline(move || {
                for file in files {
                    let source = Source::Path {
                        path: file.path,
                        origin: file.origin,
                    };
                    Self::insert_one(stash, counters, &file.digest, file.executable, source)?;
                }
                Ok(())
            })
            .await
    }

    /// Puts `content` in the stash as the content of `digest`, which it must hash to.
    pub async fn insert_bytes(
        &self,
        content: Vec<u8>,
        digest: &TrackedFileDigest,
        executable: bool,
    ) -> buck2_error::Result<()> {
        let attached = self.attached()?;
        let (stash, counters, digest) = (&attached.stash, &*self.counters, digest.dupe());
        attached
            .blocking
            .execute_io_inline(move || {
                Self::insert_one(stash, counters, &digest, executable, Source::Bytes(content))
            })
            .await
    }

    /// Keeps `content`, which must be `digest`'s, in memory as the content of the digest,
    /// compressed: for a write made here that nothing has asked for on disk, and that the
    /// materializer writes out of memory when something does. A digest the store holds already,
    /// or the CAS is known to, is left alone. Compresses on the calling thread.
    pub fn insert_in_memory(
        &self,
        content: &[u8],
        digest: &TrackedFileDigest,
        executable: bool,
    ) -> buck2_error::Result<()> {
        if digest.stash().is_some() || in_cas(digest) {
            return Ok(());
        }
        if digest.size() != content.len() as u64 {
            return Err(BlobStoreError::WrongContent {
                digest: digest.data().dupe(),
                len: content.len(),
            }
            .into());
        }
        let (record, compressed_bytes) = self.memory.insert(content, executable)?;
        if digest.register_stash(record) {
            self.counters.memory_recorded(compressed_bytes);
        } else {
            self.memory.remove(record);
        }
        Ok(())
    }

    /// The whole of an insertion into the stash: the file under a name of its own, then the
    /// record on the entry, which is the commit. Whoever finds a record there by then holds a
    /// duplicate and removes it; nothing else ever has to agree with anything. A blob the CAS
    /// is known to hold is not inserted: the CAS serves it, and the stash holds what the CAS does
    /// not.
    fn insert_one(
        stash: &Stash,
        counters: &Counters,
        digest: &TrackedFileDigest,
        executable: bool,
        source: Source,
    ) -> buck2_error::Result<()> {
        if digest.stash().is_some() || in_cas(digest) {
            return Ok(());
        }
        let record = stash.insert(digest.data(), executable, source)?;
        counters.insertions.fetch_add(1, Ordering::Relaxed);
        if digest.register_stash(record) {
            counters.recorded(digest.size());
        } else {
            counters.duplicates.fetch_add(1, Ordering::Relaxed);
            stash.unlink(digest.data(), record)?;
        }
        Ok(())
    }

    /// Puts each digest's blob at its destination, with the executable bit as asked: out of the
    /// stash, a clone of its file where the filesystem has them, else a hardlink when the mode
    /// agrees, else a copy; out of memory, written. One answer per file, `false` where the store
    /// does not hold the blob, which is every answer before `attach_stash`, since the IO runs on
    /// the executor that gives the store.
    pub async fn materialize(&self, files: Vec<FileToLink>) -> buck2_error::Result<Vec<bool>> {
        let Some(attached) = self.stash.get() else {
            return Ok(vec![false; files.len()]);
        };
        let (stash, memory, counters) = (&attached.stash, &*self.memory, &*self.counters);
        attached
            .blocking
            .execute_io_inline(move || {
                files
                    .into_iter()
                    .map(|file| {
                        let Some(record) = file.digest.stash() else {
                            return Ok(false);
                        };
                        let digest = file.digest.data();
                        let put = if record.stash_id() == memory.nonces.id {
                            memory.write(digest, record, file.executable, &file.dest)
                        } else {
                            stash.link(digest, record, file.executable, &file.dest)
                        };
                        match put {
                            Ok(true) => {
                                if record.stash_id() == memory.nonces.id {
                                    counters.memory_served.fetch_add(1, Ordering::Relaxed);
                                } else {
                                    counters.served.fetch_add(1, Ordering::Relaxed);
                                }
                                Ok(true)
                            }
                            Ok(false) => {
                                // The blob is gone from under the record: a file, by something
                                // outside buck2; bytes, by the collector, racing this read of the
                                // record. The record goes too, and the CAS is asked.
                                if file.digest.clear_stash(record)
                                    && record.stash_id() != memory.nonces.id
                                {
                                    counters.forgotten(file.digest.size());
                                }
                                Ok(false)
                            }
                            Err(e) => {
                                // A miss: the CAS is asked, which a half-made destination would
                                // be in the way of.
                                buck2_core::soft_error!(
                                    "blob_store_link_failed",
                                    e.context(format!(
                                        "Putting {} out of the store at {}",
                                        file.digest.data(),
                                        file.dest
                                    ))
                                );
                                let _ignored = fs_util::remove_all(&file.dest);
                                Ok(false)
                            }
                        }
                    })
                    .collect()
            })
            .await
    }

    /// One walk over the table. For every entry: referenced, CAS-confirmed and expiring soon, a
    /// handle for the refresher; referenced with a stash record although the CAS holds the
    /// blob, the record goes to the collector; unreferenced, dropped, and its record with it,
    /// which is the backstop for a last drop the interner's inexact check missed. The refresh
    /// only runs once the daemon has an RE connection to refresh through.
    pub async fn walk(&self) -> buck2_error::Result<SweepStats> {
        let now = Timestamp::now();
        let deadline = TtlRefresher::deadline(now);
        let now = now.as_second();
        let refreshing = self.refresh.get().is_some();
        let interner = self.interner;
        // One walk over the whole table under its shard locks: synchronous work that would stall
        // a runtime worker for as long as the table is large.
        let (sweep, handles) = tokio::task::spawn_blocking(move || {
            interner.sweep(|entry| {
                if !entry.referenced {
                    return Verdict::Drop;
                }
                let in_cas = entry.expires_secs > now;
                let evict = in_cas && entry.stash.is_some();
                let refresh = refreshing && entry.expires_secs > 0 && entry.expires_secs < deadline;
                if evict || refresh {
                    Verdict::Handle
                } else {
                    Verdict::Keep
                }
            })
        })
        .await
        .map_err(|e| internal_error!("The BlobStore walk did not complete: {e}"))?;
        let mut candidates = Vec::new();
        for handle in handles {
            let expires = handle.expires().map_or(0, |expires| expires.as_second());
            if handle.stash().is_some() && expires > now {
                self.interner.evict_stash(&handle);
            }
            if refreshing && expires > 0 && expires < deadline {
                candidates.push(handle);
            }
        }
        if refreshing {
            self.refresh
                .get()
                .expect("refreshing")
                .pass(sweep, candidates)
                .await?;
        }
        Ok(sweep)
    }
}

/// Whether the CAS is known to hold the blob right now.
fn in_cas(digest: &TrackedFileDigest) -> bool {
    matches!(digest.expires(), Ok(expires) if expires > Timestamp::now())
}

/// The collector: unlinks the files of the records the interner hands over, in batches on the
/// blocking executor, for as long as the store lives.
async fn collect(
    mut inbox: mpsc::UnboundedReceiver<Collect>,
    attached: Arc<AttachedStash>,
    counters: Arc<Counters>,
) {
    let mut batch = Vec::new();
    while inbox.recv_many(&mut batch, 1024).await > 0 {
        let mut orphans = Vec::new();
        let mut flushes = Vec::new();
        for item in batch.drain(..) {
            match item {
                Collect::Orphan(digest, record) => orphans.push((digest, record)),
                Collect::Flush(done) => flushes.push(done),
            }
        }
        if !orphans.is_empty() {
            let (stash, counters) = (&attached.stash, &*counters);
            let unlinked = attached
                .blocking
                .execute_io_inline(move || {
                    for (digest, record) in orphans {
                        match stash.unlink(&digest, record) {
                            Ok(unlinked) => {
                                counters.forgotten(digest.size());
                                if unlinked {
                                    counters.unlinked(digest.size());
                                }
                            }
                            Err(e) => {
                                buck2_core::soft_error_or_panic!(
                                    "blob_store_unlink_failed",
                                    e.context(format!("Removing {digest} from the stash"))
                                );
                            }
                        }
                    }
                    Ok(())
                })
                .await;
            if let Err(e) = unlinked {
                buck2_core::soft_error_or_panic!("blob_store_unlink_failed", e);
            }
        }
        for done in flushes {
            let _ignored = done.send(());
        }
    }
}

/// The directory next to a stash that its previous contents are moved into, to be removed.
const TRASH: &str = "stash.trash";

/// Moves what is at `root`, the stash a previous daemon left, aside and removes it in the
/// background: nothing refers to it any more. For the daemon that goes on to attach the directory
/// and for one that runs without a stash alike; a start that dies before the removal is done
/// leaves the rest for the next. A failure is reported and costs disk, not the build. Must be
/// called from within a tokio runtime.
pub fn discard_stash(root: &AbsNormPath, blocking: Arc<dyn BlockingExecutor>) {
    let Some(parent) = root.parent() else {
        return;
    };
    let trash = parent.join(ForwardRelativePath::unchecked_new(TRASH));
    let moved = (|| -> buck2_error::Result<bool> {
        if fs_util::try_exists(root)? {
            fs_util::create_dir_all(&trash)?;
            let aside = trash.join(ForwardRelativePathBuf::unchecked_new(format!(
                "{:x}",
                RandomState::new().hash_one(0u64)
            )));
            fs_util::rename(root, &aside)?;
            return Ok(true);
        }
        fs_util::try_exists(&trash)
    })();
    match moved {
        Ok(false) => {}
        Ok(true) => {
            tokio::spawn(async move {
                let removed = blocking
                    .execute_io_inline(move || fs_util::remove_all(&trash))
                    .await;
                if let Err(e) = removed {
                    buck2_core::soft_error_or_panic!(
                        "blob_store_stash_removal_failed",
                        e.context("Removing a previous daemon's stash")
                    );
                }
            });
        }
        Err(e) => {
            buck2_core::soft_error_or_panic!(
                "blob_store_stash_removal_failed",
                e.context("Discarding a previous daemon's stash")
            );
        }
    }
}

enum Source {
    Path {
        path: AbsNormPathBuf,
        origin: Origin,
    },
    Bytes(Vec<u8>),
}

/// The nonces one of the store's places names its entries by: nonzero and below 2^62, as the
/// record wants, with the place's id in the upper half, by which the interner hands a record to
/// the place's collector (`StashRecord::stash_id`). A stash's id is odd and a memory's even, so
/// that the two of one store never share one; the stores of one process, the daemon's and a
/// test's several, draw theirs at random.
#[derive(Allocative)]
struct Nonces {
    id: u32,
    /// The lower half of the next nonce, started at random.
    next: AtomicU64,
}

impl Nonces {
    fn new(id: u32) -> Self {
        Self {
            id,
            next: AtomicU64::new(RandomState::new().hash_one(0u64)),
        }
    }

    /// A nonce this place has not used.
    fn next(&self) -> u64 {
        let low = self.next.fetch_add(1, Ordering::Relaxed) as u32;
        (u64::from(self.id) << 32) | u64::from(low)
    }
}

/// A random id below 2^30, odd or even as asked.
fn random_id(odd: bool) -> u32 {
    (RandomState::new().hash_one(1u64) as u32 & 0x3fff_fffc) + if odd { 1 } else { 2 }
}

/// The directory: CASd's layout, `<algo>/<h0h1>/<h2h3>/<rest>.<size>[_non_exec].<nonce>`, so
/// that no directory grows large and so that the store can follow CASd if it changes its mind.
/// Every insertion writes a file of its own, named by a nonce, so that no two insertions and no
/// insertion and removal ever touch one name; the mode is in the name because a hardlink shares
/// it. A file is never modified once written, and nothing reads a name the table does not
/// record, so a file is in the stash exactly from its record on; a crash before that leaves a
/// file the next daemon's start discards (`discard_stash`).
#[derive(Allocative)]
struct Stash {
    root: AbsNormPathBuf,
    #[allocative(skip)]
    config: CasDigestConfig,
    nonces: Nonces,
}

const NON_EXECUTABLE: &str = "_non_exec";

impl Stash {
    fn new(root: AbsNormPathBuf, config: CasDigestConfig) -> Self {
        Self {
            root,
            config,
            nonces: Nonces::new(random_id(true)),
        }
    }

    fn file(&self, digest: &FileDigest, record: StashRecord) -> AbsNormPathBuf {
        self.root.join(file_name(digest, record))
    }

    fn insert(
        &self,
        digest: &FileDigest,
        executable: bool,
        source: Source,
    ) -> buck2_error::Result<StashRecord> {
        let record = StashRecord {
            nonce: self.nonces.next(),
            executable,
        };
        let file = self.file(digest, record);
        let leaf = file
            .parent()
            .internal_error("an entry has a leaf directory")?;
        fs_util::create_dir_all(leaf)?;
        let made_a_file = match source {
            Source::Path {
                path,
                origin: Origin::BuckOut,
            } => fs_util::reflink_or_hardlink_or_copy(&path, &file)? != Linked::Hardlinked,
            Source::Path {
                path,
                origin: Origin::Source,
            } => {
                fs_util::reflink_or_copy(&path, &file)?;
                if let Err(e) = self.verify(&file, digest) {
                    let _ignored = fs_util::remove_file(&file);
                    return Err(e);
                }
                true
            }
            Source::Bytes(content) => {
                fs_util::write(&file, content)?;
                true
            }
        };
        // A hardlink is the source's inode, mode included; the other legs made a new file.
        if made_a_file {
            fs_util::set_executable(&file, executable)?;
        }
        Ok(record)
    }

    /// Puts the recorded file's content at `dest`: a clone where the filesystem has them, so that
    /// a write to the file in place cannot reach the stash; else a hardlink when the mode agrees
    /// and the filesystem allows another link; else a copy. `Ok(false)` when the file is not
    /// there.
    fn link(
        &self,
        digest: &FileDigest,
        record: StashRecord,
        executable: bool,
        dest: &AbsNormPath,
    ) -> buck2_error::Result<bool> {
        let file = self.file(digest, record);
        if !fs_util::try_exists(&file)? {
            return Ok(false);
        }
        if let Some(parent) = dest.parent() {
            fs_util::create_dir_all(parent)?;
        }
        if record.executable == executable {
            fs_util::reflink_or_hardlink_or_copy(&file, dest)?;
        } else {
            // A clone or a copy has its own mode; a hardlink would share the file's.
            fs_util::reflink_or_copy(&file, dest)?;
            fs_util::set_executable(dest, executable)?;
        }
        Ok(true)
    }

    /// Removes the recorded file; `Ok(false)` when it is not there.
    fn unlink(&self, digest: &FileDigest, record: StashRecord) -> buck2_error::Result<bool> {
        let file = self.file(digest, record);
        if !fs_util::try_exists(&file)? {
            return Ok(false);
        }
        fs_util::remove_file(&file)?;
        Ok(true)
    }

    fn verify(&self, path: &AbsNormPath, expected: &FileDigest) -> buck2_error::Result<()> {
        let algorithm = self
            .config
            .algorithm_for_family(expected.raw_digest().algorithm())
            .internal_error("the digest's algorithm is one this daemon computes")?;
        let file = File::open(path).with_buck_error_context(|| format!("Opening `{path}`"))?;
        let found = FileDigest::from_reader_for_algorithm(file, algorithm)?;
        if &found != expected {
            return Err(BlobStoreError::ContentChanged {
                path: path.to_owned(),
                expected: expected.dupe(),
                found,
            }
            .into());
        }
        Ok(())
    }
}

/// The bytes: content compressed with zstd, by nonce. A deferred write's, which nothing has asked
/// for on disk and most never will: a cold build's writes are hundreds of thousands of small
/// files, nearly all of them inputs of actions that run remotely or hit the cache, and they are
/// a small fraction of the daemon's memory compressed where on disk they are gigabytes and the IO
/// to put them there. Nothing is modified once inserted, and a record names its bytes exactly
/// until the collector takes them.
#[derive(Allocative)]
struct Memory {
    nonces: Nonces,
    blobs: BuckDashMap<u64, Box<[u8]>>,
}

impl Memory {
    fn new() -> Self {
        Self {
            nonces: Nonces::new(random_id(false)),
            blobs: BuckDashMap::default(),
        }
    }

    /// Puts `content` in under a nonce of its own: the record for it, and how many bytes it
    /// takes compressed.
    fn insert(&self, content: &[u8], executable: bool) -> buck2_error::Result<(StashRecord, u64)> {
        // The zstd crate does not release the spare capacity of its encoding buffer, so the
        // compressed `Vec` is as large as the input until it is shrunk here.
        let compressed = zstd::bulk::compress(content, 0)
            .with_buck_error_context(|| format!("Compressing {} bytes", content.len()))?
            .into_boxed_slice();
        let compressed_bytes = compressed.len() as u64;
        let record = StashRecord {
            nonce: self.nonces.next(),
            executable,
        };
        self.blobs.insert(record.nonce, compressed);
        Ok((record, compressed_bytes))
    }

    /// Takes the recorded bytes out, if they are there.
    fn remove(&self, record: StashRecord) -> Option<Box<[u8]>> {
        self.blobs.remove(&record.nonce).map(|(_, blob)| blob)
    }

    /// Writes the recorded content, `digest`'s, at `dest` with the mode asked. `Ok(false)` when
    /// the bytes are not there.
    fn write(
        &self,
        digest: &FileDigest,
        record: StashRecord,
        executable: bool,
        dest: &AbsNormPath,
    ) -> buck2_error::Result<bool> {
        // The compressed bytes are copied out from under the shard's lock, which an insertion
        // into the shard would otherwise wait on for the length of the decompression and the
        // write.
        let Some(compressed) = self.blobs.get(&record.nonce).map(|blob| blob.clone()) else {
            return Ok(false);
        };
        let content = zstd::bulk::decompress(&compressed, digest.size() as usize)
            .with_buck_error_context(|| format!("Decompressing {digest}"))?;
        if let Some(parent) = dest.parent() {
            fs_util::create_dir_all(parent)?;
        }
        fs_util::write(dest, content)?;
        fs_util::set_executable(dest, executable)?;
        Ok(true)
    }
}

fn algorithm_dir(raw: &RawDigest) -> &'static str {
    match raw {
        RawDigest::Sha1(_) => "sha1",
        RawDigest::Sha256(_) => "sha256",
        RawDigest::Blake3(_) => "blake3",
        RawDigest::Blake3Keyed(_) => "keyed_blake3",
    }
}

fn file_name(digest: &FileDigest, record: StashRecord) -> ForwardRelativePathBuf {
    let hex = digest.raw_digest().to_string();
    ForwardRelativePathBuf::unchecked_new(format!(
        "{}/{}/{}/{}.{}{}.{:x}",
        algorithm_dir(digest.raw_digest()),
        &hex[..2],
        &hex[2..4],
        &hex[4..],
        digest.size(),
        if record.executable {
            ""
        } else {
            NON_EXECUTABLE
        },
        record.nonce,
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use buck2_core::fs::project::ProjectRoot;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    use jiff::SignedDuration;

    use super::*;
    use crate::execute::blocking::testing::DummyBlockingExecutor;
    use crate::re::presence::Presence;
    use crate::re::ttl_refresh::PassPresenceChecker;
    use crate::re::ttl_refresh::PresenceChecker;
    use crate::re::ttl_refresh::StandaloneTtlRefreshStats;

    /// A table of a test's own, so that a walk sees exactly what the test put there and its
    /// stamping reaches nothing else in the binary. Handles taken from it are dropped through the
    /// global interner, which does not know these entries, so their last drop leaves them for
    /// the walk exactly as the drop-side race would.
    fn private_table() -> &'static DigestInterner {
        Box::leak(Box::new(DigestInterner::new()))
    }

    fn config() -> CasDigestConfig {
        CasDigestConfig::testing_default()
    }

    /// A store over a private table with a stash under the project root.
    fn store(fs: &ProjectRoot) -> (BlobStore, &'static DigestInterner) {
        let table = private_table();
        let store = BlobStore::new(table);
        assert!(store.attach_stash(
            fs.root().join(ForwardRelativePath::unchecked_new("stash")),
            config(),
            Arc::new(DummyBlockingExecutor { fs: fs.dupe() }),
        ));
        (store, table)
    }

    fn digest(content: &[u8]) -> FileDigest {
        FileDigest::from_content(content, config())
    }

    /// A path next to the stash, for the files a test makes and materializes.
    fn scratch(fs: &ProjectRoot, name: &str) -> AbsNormPathBuf {
        fs.root().join(ForwardRelativePath::unchecked_new(name))
    }

    fn is_executable(path: &AbsNormPath) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs_util::metadata(path).unwrap().permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            true
        }
    }

    async fn materialize_one(
        store: &BlobStore,
        digest: &TrackedFileDigest,
        executable: bool,
        dest: &AbsNormPath,
    ) -> buck2_error::Result<bool> {
        Ok(store
            .materialize(vec![FileToLink {
                digest: digest.dupe(),
                executable,
                dest: dest.to_owned(),
            }])
            .await?[0])
    }

    /// The files in the stash for `digest`.
    fn files_of(store: &BlobStore, digest: &FileDigest) -> Vec<String> {
        let stash = &store.stash.get().unwrap().stash;
        let any = stash.file(
            digest,
            StashRecord {
                nonce: 1,
                executable: true,
            },
        );
        let leaf = any.parent().unwrap();
        let hex = digest.raw_digest().to_string();
        let mut names: Vec<String> = fs_util::read_dir(leaf)
            .map(|entries| {
                entries
                    .map(|e| e.unwrap().file_name().to_str().unwrap().to_owned())
                    .filter(|name| name.starts_with(&hex[4..]))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn test_file_names_follow_casd_s_layout() {
        let digest = FileDigest::new(
            RawDigest::parse_sha1(b"6bf21a57b4cd6e4d4b2577b7597849edb04b12f2").unwrap(),
            7,
        );
        let record = StashRecord {
            nonce: 0xab,
            executable: false,
        };
        assert_eq!(
            file_name(&digest, record).as_str(),
            "sha1/6b/f2/1a57b4cd6e4d4b2577b7597849edb04b12f2.7_non_exec.ab"
        );
        let exec = StashRecord {
            nonce: 0xab,
            executable: true,
        };
        assert_eq!(
            file_name(&digest, exec).as_str(),
            "sha1/6b/f2/1a57b4cd6e4d4b2577b7597849edb04b12f2.7.ab"
        );
    }

    #[tokio::test]
    async fn test_bytes_go_in_and_come_out_in_either_mode() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let (store, table) = store(temp.path());
        let digest = table.intern(digest(b"hello"), 0);
        store
            .insert_bytes(b"hello".to_vec(), &digest, false)
            .await?;
        // A second insertion of a recorded digest does nothing.
        store
            .insert_bytes(b"hello".to_vec(), &digest, false)
            .await?;
        let stats = store.stats();
        assert_eq!((stats.entries, stats.bytes, stats.insertions), (1, 5, 1));
        assert_eq!(files_of(&store, digest.data()).len(), 1);

        let plain = scratch(temp.path(), "plain");
        assert!(materialize_one(&store, &digest, false, &plain).await?);
        assert_eq!(fs_util::read_to_string(&plain)?, "hello");

        // The other mode is a clone of the file, not its inode. Windows has no mode to tell
        // the two apart by.
        let exec = scratch(temp.path(), "exec");
        assert!(materialize_one(&store, &digest, true, &exec).await?);
        assert_eq!(fs_util::read_to_string(&exec)?, "hello");
        if cfg!(unix) {
            assert!(is_executable(&exec));
            assert!(!is_executable(&plain));
        }

        let missing = scratch(temp.path(), "missing");
        let unknown = table.intern(self::digest(b"else"), 0);
        assert!(!materialize_one(&store, &unknown, false, &missing).await?);
        assert!(!fs_util::try_exists(&missing)?);
        Ok(())
    }

    #[tokio::test]
    async fn test_a_file_from_buck_out_goes_in_by_its_cheapest_link() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let (store, table) = store(temp.path());
        let src = scratch(temp.path(), "out");
        fs_util::write(&src, b"built")?;
        fs_util::set_executable(&src, true)?;
        let digest = table.intern(digest(b"built"), 0);
        store
            .insert_files(vec![FileToInsert {
                path: src,
                digest: digest.dupe(),
                executable: true,
                origin: Origin::BuckOut,
            }])
            .await?;

        let dest = scratch(temp.path(), "again");
        assert!(materialize_one(&store, &digest, true, &dest).await?);
        assert_eq!(fs_util::read_to_string(&dest)?, "built");
        assert!(is_executable(&dest));
        Ok(())
    }

    #[tokio::test]
    async fn test_a_source_is_checked_and_never_shares_its_inode() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let (store, table) = store(temp.path());
        let src = scratch(temp.path(), "src");
        fs_util::write(&src, b"source")?;

        // A digest the file no longer matches is refused and leaves nothing behind.
        let stale = table.intern(digest(b"what it used to be"), 0);
        let from_source = |digest: &TrackedFileDigest| FileToInsert {
            path: src.clone(),
            digest: digest.dupe(),
            executable: false,
            origin: Origin::Source,
        };
        assert!(store.insert_files(vec![from_source(&stale)]).await.is_err());
        assert_eq!(stale.stash(), None);
        assert_eq!(files_of(&store, stale.data()), Vec::<String>::new());

        let digest = table.intern(digest(b"source"), 0);
        store.insert_files(vec![from_source(&digest)]).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let stash = &store.stash.get().unwrap().stash;
            assert_ne!(
                fs_util::metadata(&src)?.ino(),
                fs_util::metadata(stash.file(digest.data(), digest.stash().unwrap()))?.ino()
            );
        }
        Ok(())
    }

    /// The walk drops what nothing holds, and the records of the entries it drops go to the
    /// collector; a record the CAS makes unnecessary goes too, at the confirmation or, if it
    /// slipped in after it, at the walk.
    #[tokio::test]
    async fn test_a_walk_drops_what_nothing_holds_and_what_the_cas_has() -> buck2_error::Result<()>
    {
        let temp = ProjectRootTemp::new()?;
        let (store, table) = store(temp.path());
        let held = table.intern(digest(b"held content"), 0);
        let dropped = table.intern(digest(b"dropped content"), 0);
        let confirmed = table.intern(digest(b"confirmed content"), 0);
        store
            .insert_bytes(b"held content".to_vec(), &held, false)
            .await?;
        store
            .insert_bytes(b"dropped content".to_vec(), &dropped, true)
            .await?;
        store
            .insert_bytes(b"confirmed content".to_vec(), &confirmed, false)
            .await?;
        assert_eq!((store.stats().entries, store.stats().bytes), (3, 44));
        // Handles of a private table drop through the global interner, which does not know them,
        // so this entry is the walk's to drop, as one the drop-side check missed would be.
        let dropped_data = dropped.data().dupe();
        drop(dropped);
        // The CAS turns out to hold the third, as a presence check would tell this table.
        let in_four_hours = (Timestamp::now() + SignedDuration::from_hours(4)).as_second();
        drop(table.intern(confirmed.data().dupe(), in_four_hours));
        store.flush_collector().await;
        assert_eq!(confirmed.stash(), None);
        assert_eq!(files_of(&store, confirmed.data()), Vec::<String>::new());
        // A record that arrives after the confirmation, which an insertion racing it can leave.
        let stash = &store.stash.get().unwrap().stash;
        let late = stash.insert(
            confirmed.data(),
            false,
            Source::Bytes(b"confirmed content".to_vec()),
        )?;
        assert!(confirmed.register_stash(late));
        store.counters.recorded(confirmed.size());

        let sweep = store.walk().await?;
        store.flush_collector().await;
        assert_eq!(sweep.swept, 1);
        assert!(table.peek(&dropped_data).is_none());
        assert_eq!(files_of(&store, &dropped_data), Vec::<String>::new());
        assert_eq!(confirmed.stash(), None);
        assert_eq!(files_of(&store, confirmed.data()), Vec::<String>::new());
        assert_eq!(files_of(&store, held.data()).len(), 1);
        assert_eq!((store.stats().entries, store.stats().bytes), (1, 12));
        assert_eq!(store.stats().removed_bytes, 15 + 17 + 17);
        Ok(())
    }

    /// Attaching starts the stash empty: a previous daemon's files are moved aside, since nothing
    /// refers to them any more, and removed in the background.
    #[tokio::test]
    async fn test_attaching_discards_what_a_previous_daemon_left() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let (first, table) = store(temp.path());
        let left = table.intern(digest(b"left behind"), 0);
        first
            .insert_bytes(b"left behind".to_vec(), &left, true)
            .await?;
        let root = temp
            .path()
            .root()
            .join(ForwardRelativePath::unchecked_new("stash"));
        let file = first
            .stash
            .get()
            .unwrap()
            .stash
            .file(left.data(), left.stash().unwrap());
        assert!(fs_util::try_exists(&file)?);

        // A new daemon as far as the directory goes: the same root, a table recording nothing.
        let (second, table) = store(temp.path());
        assert!(!fs_util::try_exists(&file)?);
        assert!(!fs_util::try_exists(&root)?);
        let left = table.intern(left.data().dupe(), 0);
        let dest = scratch(temp.path(), "dest");
        assert!(!materialize_one(&second, &left, true, &dest).await?);
        assert_eq!((second.stats().entries, second.stats().bytes), (0, 0));
        Ok(())
    }

    /// A store over the process's table, whose handles' drops reach the table, with a stash
    /// under the project root. The content such a test uses is its own, so that nothing else
    /// in the binary holds the digests.
    fn store_over_global(fs: &ProjectRoot) -> BlobStore {
        let store = BlobStore::new(file_digest_interner());
        assert!(store.attach_stash(
            fs.root().join(ForwardRelativePath::unchecked_new("stash")),
            config(),
            Arc::new(DummyBlockingExecutor { fs: fs.dupe() }),
        ));
        store
    }

    /// The last handle takes the stash's file with it.
    #[tokio::test]
    async fn test_the_last_handle_takes_the_stash_file_with_it() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let store = store_over_global(temp.path());
        let content = b"blob_store test: held until its last handle drops";
        let handle = TrackedFileDigest::from_content(content, config());
        store.insert_bytes(content.to_vec(), &handle, false).await?;
        let file = store
            .stash
            .get()
            .unwrap()
            .stash
            .file(handle.data(), handle.stash().unwrap());
        assert!(fs_util::try_exists(&file)?);

        // A second handle keeps it.
        let again = TrackedFileDigest::from_content(content, config());
        drop(handle);
        store.flush_collector().await;
        assert!(fs_util::try_exists(&file)?);
        assert!(again.stash().is_some());

        drop(again);
        store.flush_collector().await;
        assert!(!fs_util::try_exists(&file)?);
        assert_eq!((store.stats().entries, store.stats().bytes), (0, 0));
        assert_eq!(store.stats().removed_bytes, content.len() as u64);
        Ok(())
    }

    /// A blob the CAS is known to hold is not inserted, and one the CAS turns out to hold leaves.
    #[tokio::test]
    async fn test_what_the_cas_holds_is_not_the_stash_s() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let store = store_over_global(temp.path());
        let in_four_hours = Timestamp::now() + SignedDuration::from_hours(4);

        let known_content = b"blob_store test: in the CAS already";
        let known = TrackedFileDigest::new_expires(
            FileDigest::from_content(known_content, config()),
            in_four_hours,
            config(),
        );
        store
            .insert_bytes(known_content.to_vec(), &known, false)
            .await?;
        assert_eq!(known.stash(), None);
        assert_eq!(store.stats().insertions, 0);

        let later_content = b"blob_store test: in the CAS later";
        let later = TrackedFileDigest::from_content(later_content, config());
        store
            .insert_bytes(later_content.to_vec(), &later, false)
            .await?;
        let file = store
            .stash
            .get()
            .unwrap()
            .stash
            .file(later.data(), later.stash().unwrap());
        later.update_expires(in_four_hours);
        store.flush_collector().await;
        assert_eq!(later.stash(), None);
        assert!(!fs_util::try_exists(&file)?);
        assert_eq!((store.stats().entries, store.stats().bytes), (0, 0));
        Ok(())
    }

    #[tokio::test]
    async fn test_a_link_that_fails_is_a_miss_that_keeps_the_record() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let (store, table) = store(temp.path());
        let digest = table.intern(digest(b"stuck"), 0);
        store
            .insert_bytes(b"stuck".to_vec(), &digest, false)
            .await?;
        // Nothing can be made under a file.
        let blocker = scratch(temp.path(), "blocker");
        fs_util::write(&blocker, b"")?;
        let dest = blocker.join(ForwardRelativePath::unchecked_new("dest"));
        assert!(!materialize_one(&store, &digest, false, &dest).await?);
        assert!(digest.stash().is_some());
        Ok(())
    }

    /// Bytes kept in memory come out at any destination in either mode, and nothing of them is
    /// on disk until then.
    #[tokio::test]
    async fn test_bytes_kept_in_memory_come_out_in_either_mode() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let (store, table) = store(temp.path());
        let content = b"deferred write ".repeat(64);
        let digest = table.intern(self::digest(&content), 0);
        store.insert_in_memory(&content, &digest, false)?;
        // A second insertion of a recorded digest does nothing.
        store.insert_in_memory(&content, &digest, true)?;
        let stats = store.stats();
        assert_eq!(stats.memory_entries, 1);
        assert!(stats.memory_bytes > 0 && stats.memory_bytes < content.len() as u64);
        assert_eq!((stats.entries, stats.insertions), (0, 0));
        assert_eq!(files_of(&store, digest.data()), Vec::<String>::new());

        let plain = scratch(temp.path(), "plain");
        assert!(materialize_one(&store, &digest, false, &plain).await?);
        assert_eq!(fs_util::read_to_string(&plain)?.as_bytes(), &content[..]);
        let exec = scratch(temp.path(), "exec");
        assert!(materialize_one(&store, &digest, true, &exec).await?);
        assert_eq!(fs_util::read_to_string(&exec)?.as_bytes(), &content[..]);
        if cfg!(unix) {
            assert!(is_executable(&exec));
            assert!(!is_executable(&plain));
        }
        assert_eq!((store.stats().memory_served, store.stats().served), (2, 0));

        // Content that is not the digest's is refused.
        let other = table.intern(self::digest(b"other"), 0);
        assert!(store.insert_in_memory(&content, &other, false).is_err());
        assert_eq!(other.stash(), None);
        Ok(())
    }

    /// One place holds a blob, whichever recorded it first; the other leaves it alone.
    #[tokio::test]
    async fn test_one_place_holds_a_blob() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let (store, table) = store(temp.path());
        let memory_first = table.intern(digest(b"memory first"), 0);
        store.insert_in_memory(b"memory first", &memory_first, false)?;
        store
            .insert_bytes(b"memory first".to_vec(), &memory_first, false)
            .await?;
        let stash_first = table.intern(digest(b"stash first"), 0);
        store
            .insert_bytes(b"stash first".to_vec(), &stash_first, false)
            .await?;
        store.insert_in_memory(b"stash first", &stash_first, false)?;
        let stats = store.stats();
        assert_eq!((stats.memory_entries, stats.entries), (1, 1));
        assert_eq!(files_of(&store, memory_first.data()), Vec::<String>::new());
        assert_eq!(files_of(&store, stash_first.data()).len(), 1);
        Ok(())
    }

    /// The last handle takes the bytes with it, as it takes the stash's file, and so does the
    /// CAS turning out to hold the blob.
    #[tokio::test]
    async fn test_memory_lets_go_as_the_stash_does() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let store = store_over_global(temp.path());
        let content = b"blob_store test: in memory until its last handle drops";
        let handle = TrackedFileDigest::from_content(content, config());
        store.insert_in_memory(content, &handle, false)?;
        assert_eq!(store.stats().memory_entries, 1);
        // A second handle keeps it.
        let again = TrackedFileDigest::from_content(content, config());
        drop(handle);
        assert_eq!(store.stats().memory_entries, 1);
        assert!(again.stash().is_some());
        drop(again);
        assert_eq!(
            (store.stats().memory_entries, store.stats().memory_bytes),
            (0, 0)
        );

        let later_content = b"blob_store test: in memory until the CAS has it";
        let later = TrackedFileDigest::from_content(later_content, config());
        store.insert_in_memory(later_content, &later, false)?;
        assert_eq!(store.stats().memory_entries, 1);
        later.update_expires(Timestamp::now() + SignedDuration::from_hours(4));
        assert_eq!(later.stash(), None);
        assert_eq!(store.stats().memory_entries, 0);
        let dest = scratch(temp.path(), "dest");
        assert!(!materialize_one(&store, &later, false, &dest).await?);
        Ok(())
    }

    /// The walk drops the bytes of an entry nothing holds, as it drops a stash record.
    #[tokio::test]
    async fn test_a_walk_drops_memory_nothing_holds() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let (store, table) = store(temp.path());
        let dropped = table.intern(digest(b"dropped from memory"), 0);
        store.insert_in_memory(b"dropped from memory", &dropped, false)?;
        // A private table's handles drop through the global interner, so the walk is what
        // drops this entry.
        let dropped_data = dropped.data().dupe();
        drop(dropped);
        assert_eq!(store.stats().memory_entries, 1);

        let sweep = store.walk().await?;
        assert_eq!(sweep.swept, 1);
        assert!(table.peek(&dropped_data).is_none());
        assert_eq!(
            (store.stats().memory_entries, store.stats().memory_bytes),
            (0, 0)
        );
        Ok(())
    }

    #[test]
    fn test_nonces_carry_the_place_s_id_and_never_repeat() {
        let stash = Stash::new(AbsNormPathBuf::new(std::env::temp_dir()).unwrap(), config());
        let (a, b) = (stash.nonces.next(), stash.nonces.next());
        assert_ne!(a, b);
        assert!(a != 0 && a < 1 << 62);
        assert_ne!(stash.nonces.id, 0);
        assert_eq!(
            StashRecord {
                nonce: a,
                executable: false
            }
            .stash_id(),
            stash.nonces.id
        );
        // The two places of one store never draw the same id.
        let memory = Memory::new();
        assert_eq!(stash.nonces.id % 2, 1);
        assert_eq!(memory.nonces.id % 2, 0);
        let c = memory.nonces.next();
        assert!(c != 0 && c < 1 << 62);
    }

    // The refresh side of the walk, with a CAS that answers as the test says.

    enum Answer {
        Alive(SignedDuration),
        Gone,
        Fail,
    }

    struct StubInner {
        answer: Box<dyn Fn(&FileDigest) -> Answer + Send + Sync>,
        asked: Mutex<Vec<FileDigest>>,
    }

    #[derive(Clone, Dupe)]
    struct Stub(Arc<StubInner>);

    impl Stub {
        fn new(answer: impl Fn(&FileDigest) -> Answer + Send + Sync + 'static) -> Self {
            Self(Arc::new(StubInner {
                answer: Box::new(answer),
                asked: Mutex::new(Vec::new()),
            }))
        }

        fn asked(&self) -> Vec<FileDigest> {
            let mut asked = self.0.asked.lock().unwrap().clone();
            asked.sort();
            asked
        }
    }

    impl PresenceChecker for Stub {
        fn begin_pass(&self) -> Box<dyn PassPresenceChecker> {
            Box::new(self.dupe())
        }
    }

    #[async_trait]
    impl PassPresenceChecker for Stub {
        async fn check(
            &self,
            digests: Vec<TrackedFileDigest>,
        ) -> buck2_error::Result<Vec<Presence>> {
            self.0
                .asked
                .lock()
                .unwrap()
                .extend(digests.iter().map(|d| d.data().dupe()));
            let mut answers = Vec::with_capacity(digests.len());
            for digest in &digests {
                answers.push(match (self.0.answer)(digest.data()) {
                    Answer::Alive(ttl) => {
                        let expires = Timestamp::now() + ttl;
                        digest.update_expires(expires);
                        Some(expires)
                    }
                    Answer::Gone => None,
                    Answer::Fail => return Err(internal_error!("injected failure")),
                });
            }
            Ok(answers)
        }
    }

    fn refreshing_store(
        stub: Stub,
        chunk_size: usize,
    ) -> (&'static BlobStore, &'static DigestInterner) {
        let table = private_table();
        let store: &'static BlobStore = Box::leak(Box::new(BlobStore::new(table)));
        assert!(
            store
                .refresh
                .set(TtlRefresher::new(Arc::new(stub)).with_chunk_size(chunk_size))
                .is_ok()
        );
        (store, table)
    }

    fn text_digest(content: &str) -> FileDigest {
        digest(content.as_bytes())
    }

    #[tokio::test]
    async fn test_a_scheduled_walk_refreshes_what_is_held_and_expiring() {
        let now = Timestamp::now();
        let soon = text_digest("ttl_refresh soon");
        let later = text_digest("ttl_refresh later");
        let never = text_digest("ttl_refresh never confirmed");
        let unheld = text_digest("ttl_refresh unheld");
        let gone = text_digest("ttl_refresh gone");

        let extended = SignedDuration::from_hours(36);
        let gone_data = gone.dupe();
        let stub = Stub::new(move |digest| {
            if digest == &gone_data {
                Answer::Gone
            } else {
                Answer::Alive(extended)
            }
        });
        let (store, table) = refreshing_store(stub.dupe(), 5000);

        let in_ten_minutes = (now + SignedDuration::from_mins(10)).as_second();
        let in_five_hours = (now + SignedDuration::from_hours(5)).as_second();
        let soon_handle = table.intern(soon.dupe(), in_ten_minutes);
        let later_handle = table.intern(later.dupe(), in_five_hours);
        let never_handle = table.intern(never.dupe(), 0);
        drop(table.intern(unheld.dupe(), in_ten_minutes));
        let gone_handle = table.intern(gone.dupe(), in_ten_minutes);
        assert_eq!(table.stats().entries, 5);

        store.walk().await.unwrap();

        let mut expected = vec![soon.dupe(), gone.dupe()];
        expected.sort();
        assert_eq!(
            stub.asked(),
            expected,
            "held and expiring digests are asked about; `later` is outside the window, `never` \
             was never confirmed, `unheld` was swept"
        );
        assert!(soon_handle.expires().unwrap() >= now + extended - SignedDuration::from_secs(5));
        assert!(gone_handle.expires().unwrap() <= Timestamp::now());
        assert_eq!(later_handle.expires().unwrap().as_second(), in_five_hours);
        assert_eq!(never_handle.expires().unwrap(), Timestamp::UNIX_EPOCH);
        assert!(
            table.get(&unheld).is_none(),
            "the unheld pinned entry was swept"
        );

        let counters = store.refresh.get().unwrap().counters();
        assert_eq!(
            counters.stats(),
            StandaloneTtlRefreshStats {
                passes: 1,
                errors: 0,
                last_pass_visited: 5,
                last_pass_swept: 1,
                last_pass_candidates: 2,
                last_pass_refreshed: 1,
                last_pass_gone: 1,
                last_pass_duration_us: counters.stats().last_pass_duration_us,
            }
        );
    }

    #[tokio::test]
    async fn test_a_failed_check_is_counted_and_stamps_nothing() {
        let now = Timestamp::now();
        let data = text_digest("ttl_refresh failing check");
        let stub = Stub::new(|_| Answer::Fail);
        let (store, table) = refreshing_store(stub.dupe(), 5000);
        let before = (now + SignedDuration::from_mins(10)).as_second();
        let handle = table.intern(data.dupe(), before);

        assert!(store.walk().await.is_err());
        assert_eq!(stub.asked(), vec![data]);
        assert_eq!(handle.expires().unwrap().as_second(), before);

        let stats = store.refresh.get().unwrap().counters().stats();
        assert_eq!(stats.passes, 1);
        assert_eq!(stats.errors, 1);
        assert_eq!(stats.last_pass_refreshed, 0);
        assert_eq!(stats.last_pass_gone, 0);
    }

    #[tokio::test]
    async fn test_a_failed_chunk_does_not_stop_the_pass() {
        let now = Timestamp::now();
        let failing = text_digest("ttl_refresh failing chunk");
        let fine = text_digest("ttl_refresh fine chunk");
        let failing_data = failing.dupe();
        let stub = Stub::new(move |digest| {
            if digest == &failing_data {
                Answer::Fail
            } else {
                Answer::Alive(SignedDuration::from_hours(36))
            }
        });
        // One digest per chunk, so the failing one cannot take the other down with it.
        let (store, table) = refreshing_store(stub.dupe(), 1);
        let before = (now + SignedDuration::from_mins(10)).as_second();
        let _failing_handle = table.intern(failing.dupe(), before);
        let fine_handle = table.intern(fine.dupe(), before);

        let result = store.walk().await;
        assert!(result.is_err());
        assert!(
            format!("{:#}", result.unwrap_err()).contains("1 of 2 presence checks failed"),
            "the error names how much of the pass failed"
        );

        let mut expected = vec![failing, fine];
        expected.sort();
        assert_eq!(stub.asked(), expected, "the pass went on after the failure");
        assert!(fine_handle.expires().unwrap() > now + SignedDuration::from_hours(35));

        let stats = store.refresh.get().unwrap().counters().stats();
        assert_eq!(stats.passes, 1);
        assert_eq!(stats.errors, 1);
        assert_eq!(stats.last_pass_candidates, 2);
        assert_eq!(stats.last_pass_refreshed, 1);
    }
}
