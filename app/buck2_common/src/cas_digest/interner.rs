/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Deduplication of tracked digests by value, so that one blob's digest is one allocation and
//! one record of where the blob is, however many holders it has: in the CAS until a recorded
//! expiration, and in the stash under a recorded entry, for as long as a holder remains.

use std::borrow::Borrow;
use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::LazyLock;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use buck2_hash::BuckDashMap;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use pagable::arc_erase::ArcErase;
use pagable::arc_erase::ArcEraseType;
use pagable::arc_erase::ArcSerializeOutcome;
use pagable::arc_erase::StdArcEraseType;
use pagable::arc_erase::deserialize_arc;
use parking_lot::RwLock;
use strong_hash::StrongHash;
use triomphe::Arc;

use crate::cas_digest::CasDigest;
use crate::cas_digest::CasDigestConfig;
use crate::cas_digest::RawDigest;
use crate::file_ops::metadata::FileDigest;

/// Every live [`TrackedFileDigest`] in the process shares an entry here.
#[allocative::root]
static FILE_DIGEST_INTERNER: LazyLock<DigestInterner> = LazyLock::new(DigestInterner::new);

pub fn file_digest_interner() -> &'static DigestInterner {
    &FILE_DIGEST_INTERNER
}

pub fn file_digest_interner_stats() -> DigestInternerStats {
    FILE_DIGEST_INTERNER.stats()
}

/// The digest of a blob buck2 puts in or takes out of the CAS: a file's contents, or a directory
/// node serialized the way RE expects it (an `ActionDirectory` fingerprint; uploads ask about
/// those the same way as about files). With it, when the CAS last said it would drop the blob.
/// There is one of these per distinct digest in the process.
#[derive(Allocative, Debug, Pagable)]
struct TrackedFileDigestInner {
    data: FileDigest,
    /// Seconds since the unix epoch at which the CAS last told us this blob expires; the epoch
    /// itself means the CAS has never been asked. A positive value pins the entry: see
    /// [`DigestInterner`].
    expires: AtomicI64,
    /// The stash entry holding this blob, as [`StashRecord`] packs it; zero when there is none.
    /// No pin: it leaves with the entry, or sooner, and is the stash's to collect then. Signed
    /// only because that is what pages know how to carry.
    stash: AtomicI64,
}

impl TrackedFileDigestInner {
    fn new(data: FileDigest, expires: i64) -> Self {
        Self {
            data,
            expires: AtomicI64::new(expires),
            stash: AtomicI64::new(0),
        }
    }

    /// A recorded expiration is always positive; a non-positive value records nothing.
    fn has_expiry(&self) -> bool {
        self.expires.load(Ordering::Relaxed) > 0
    }

    /// Whether the entry stays past its last handle: it does while a CAS expiration is recorded.
    fn is_pinned(&self) -> bool {
        self.has_expiry()
    }

    /// Takes the stash record off the entry, if there is one: for the stash's collector.
    fn take_stash(&self) -> Option<StashRecord> {
        StashRecord::unpack(self.stash.swap(0, Ordering::AcqRel))
    }

    /// Extends the recorded expiration to `expires` if that is later. Returns whether the entry
    /// just became pinned, i.e. whether nothing had been recorded before.
    ///
    /// Extending is the only safe merge for an observation of unknown age (a second holder
    /// constructing the digest with an expiration it was told, a value paging back in): a blob
    /// has one lifetime, every remaining-TTL answer the CAS gave is a lower bound on it, and
    /// an older answer must not win over a newer one.
    ///
    /// Non-positive values carry no information (they only arise from the clamp in
    /// `re_expiration_from_ttl`) and are ignored, so that "pinned" and "has a positive
    /// expiration" stay the same thing.
    fn extend_expires(&self, expires: i64) -> bool {
        if expires <= 0 {
            return false;
        }
        let previous = self.expires.fetch_max(expires, Ordering::Relaxed);
        previous <= 0
    }

    /// Records an expiration the CAS just reported for this blob. Returns whether the entry
    /// just became pinned.
    ///
    /// Like `extend_expires` while the blob is alive. A fresh report that the blob has already
    /// expired replaces whatever was recorded instead, because the CAS never brings a blob back
    /// on its own, and callers deciding whether a blob can be re-fetched need to see that it is
    /// gone.
    fn record_expires(&self, expires: i64, now: i64) -> bool {
        if expires > now {
            return self.extend_expires(expires);
        }
        if expires <= 0 {
            return false;
        }
        let previous = self.expires.swap(expires, Ordering::Relaxed);
        previous <= 0
    }
}

/// Where in the stash a blob is: the nonce its file's name carries, unique among every insertion
/// the stash has ever made, and the mode the file was stored with. Packed into one word on the
/// digest's entry, so that recording it is one atomic step: the step that commits an insertion.
#[derive(Copy, Clone, Dupe, Debug, Eq, PartialEq, Hash)]
pub struct StashRecord {
    /// Never zero, and below 2^62. The upper half is the id of the stash that made it, by which
    /// the stash's collector knows its own records.
    pub nonce: u64,
    pub executable: bool,
}

impl StashRecord {
    pub fn stash_id(self) -> u32 {
        (self.nonce >> 32) as u32
    }

    fn pack(self) -> i64 {
        debug_assert!(self.nonce != 0 && self.nonce < 1 << 62);
        ((self.nonce << 1) | self.executable as u64) as i64
    }

    fn unpack(word: i64) -> Option<Self> {
        (word != 0).then_some(Self {
            nonce: (word as u64) >> 1,
            executable: word & 1 == 1,
        })
    }
}

#[derive(Display, Allocative)]
#[display("{}", self.data())]
pub struct TrackedFileDigest {
    inner: Arc<TrackedFileDigestInner>,
}

impl Clone for TrackedFileDigest {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl Dupe for TrackedFileDigest {}

impl Drop for TrackedFileDigest {
    fn drop(&mut self) {
        FILE_DIGEST_INTERNER.dropped(&self.inner);
    }
}

impl Borrow<FileDigest> for TrackedFileDigest {
    fn borrow(&self) -> &FileDigest {
        self.data()
    }
}

impl Borrow<FileDigest> for &TrackedFileDigest {
    fn borrow(&self) -> &FileDigest {
        self.data()
    }
}

impl PartialOrd for TrackedFileDigest {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TrackedFileDigest {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.data().cmp(other.data())
    }
}

impl PartialEq for TrackedFileDigest {
    fn eq(&self, other: &Self) -> bool {
        // Equal digests nearly always share the allocation; comparing the handles first keeps
        // the hit from touching two allocations other threads are counting on.
        Arc::ptr_eq(&self.inner, &other.inner) || self.data().eq(other.data())
    }
}

impl Eq for TrackedFileDigest {}

impl Hash for TrackedFileDigest {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.data().hash(state)
    }
}

impl StrongHash for TrackedFileDigest {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.data().strong_hash(state)
    }
}

impl fmt::Debug for TrackedFileDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{} expires at {}]",
            self,
            self.inner.expires.load(Ordering::Relaxed)
        )
    }
}

impl buck2_core::directory_digest::DirectoryDigest for TrackedFileDigest {}

/// Paging preserves identity the same way the interner does: the payload is the digest and its
/// expiration, and page-in constructs through the interner, so a paged-in digest shares the
/// live entry rather than becoming an untracked duplicate of it. A row holding the digest counts
/// as a holder of the entry for the rest of the daemon's life: rows are never removed, and one is
/// read back as often as its value is paged in, so the entry is still there, and still being
/// refreshed, each time.
impl ArcErase for TrackedFileDigest {
    type Weak = ();

    fn dupe_strong(&self) -> Self {
        self.dupe()
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        None
    }

    fn is_unique(&self) -> bool {
        // The table's own reference plus this handle. The table only removes an
        // entry whose handles are all gone, so the count is never below two.
        Arc::count(&self.inner) == 2
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<ArcSerializeOutcome> {
        // Dispatched explicitly: `triomphe::Arc<T>` has its own `PagableSerialize`, which would
        // write a nested arc reference here instead of the payload.
        TrackedFileDigestInner::pagable_serialize(&self.inner, ser)?;
        FILE_DIGEST_INTERNER.page_out(self.data());
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: pagable::PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> pagable::Result<Self> {
        let inner = TrackedFileDigestInner::pagable_deserialize(deser)?;
        Ok(Self::from_parts(inner.data, inner.expires.into_inner()))
    }
}

impl PagableSerialize for TrackedFileDigest {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl<'de> PagableDeserialize<'de> for TrackedFileDigest {
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        deserialize_arc::<Self, _>(deserializer)
    }
}

impl TrackedFileDigest {
    /// The one place handles come into existence.
    pub(super) fn from_parts(data: FileDigest, expires: i64) -> Self {
        FILE_DIGEST_INTERNER.intern(data, expires)
    }

    pub fn new(data: FileDigest, config: CasDigestConfig) -> Self {
        if config.is_empty_digest(&data) {
            return Self::empty(config);
        }

        Self::from_parts(data, 0)
    }

    pub fn new_expires(data: FileDigest, expiry: jiff::Timestamp, config: CasDigestConfig) -> Self {
        if config.is_empty_digest(&data) {
            // The empty blob is in every CAS, so an expiration reported for it is as true for
            // every other holder of the shared instance as it is for this caller.
            let empty = Self::empty(config);
            empty.update_expires(expiry);
            return empty;
        }
        Self::from_parts(data, expiry.as_second())
    }

    pub fn empty(config: CasDigestConfig) -> Self {
        config.empty_file_digest()
    }

    pub fn from_content(bytes: &[u8], config: CasDigestConfig) -> Self {
        if bytes.is_empty() {
            return Self::empty(config);
        }

        Self::from_parts(CasDigest::from_content(bytes, config), 0)
    }

    pub fn data(&self) -> &FileDigest {
        &self.inner.data
    }

    pub fn raw_digest(&self) -> &RawDigest {
        self.inner.data.raw_digest()
    }

    pub fn size(&self) -> u64 {
        self.inner.data.size()
    }

    /// Whether two handles share one allocation. Handles that compare equal normally do; the
    /// exceptions are handles that outlived a removal of their entry.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn expires(&self) -> buck2_error::Result<jiff::Timestamp> {
        let expires = self.inner.expires.load(Ordering::Relaxed);
        jiff::Timestamp::from_second(expires).map_err(|_| {
            buck2_error::buck2_error!(
                buck2_error::ErrorTag::Environment,
                "CAS Digest expiration is out of the representable time range: {}",
                expires
            )
        })
    }

    /// Records an expiration the CAS just reported for this blob, for every holder of the
    /// digest. Reports that the blob is alive only ever extend the recorded expiration; a report
    /// that it has already expired replaces it.
    pub fn update_expires(&self, time: jiff::Timestamp) {
        let now = jiff::Timestamp::now().as_second();
        if self.inner.record_expires(time.as_second(), now) {
            FILE_DIGEST_INTERNER.pinned.add(1);
        }
        if time.as_second() > now {
            FILE_DIGEST_INTERNER.evict_stash_of(&self.inner);
        }
    }

    /// The stash entry holding this blob, if one is recorded.
    pub fn stash(&self) -> Option<StashRecord> {
        StashRecord::unpack(self.inner.stash.load(Ordering::Relaxed))
    }

    /// Records that `record` holds this blob, for every holder of the digest, unless an entry is
    /// recorded already, in which case the caller's is a duplicate. Returns whether it was
    /// recorded. The record lives at most as long as the table's entry: when the last handle
    /// goes, or the CAS turns out to hold the blob, the record is handed to the stash's collector
    /// ([`DigestInterner::add_stash_collector`]), which is how the stash holds what live values
    /// hold and nothing else.
    pub fn register_stash(&self, record: StashRecord) -> bool {
        self.inner
            .stash
            .compare_exchange(0, record.pack(), Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    /// Forgets `record`, if it is the one recorded: for a stash entry found gone. Returns
    /// whether it was.
    pub fn clear_stash(&self, record: StashRecord) -> bool {
        self.inner
            .stash
            .compare_exchange(record.pack(), 0, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }
}

/// Deduplicates [`TrackedFileDigest`]s by value: every holder of a digest shares one allocation
/// and one record of where the blob is.
///
/// An entry stays for as long as any handle refers to it. An entry with a CAS expiration
/// recorded stays past its last handle as well, and is *pinned*: the next construction of that
/// digest knows the blob is in the CAS without asking, and the case that pays is an action's
/// input tree, dropped right after its upload, whose files the next action uploads too. Nothing
/// but a [`sweep`](Self::sweep) releases a pinned entry. Smarter retention is possible, but the
/// table is a small fraction of the daemon's memory, so this is deliberately the simplest policy
/// with that property. A recorded stash entry does not pin: it leaves with the entry, at the
/// last handle's drop or at a sweep, or sooner, when a CAS expiration is recorded for the blob,
/// and goes to the stash's collector each time, so that the stash holds exactly what live values
/// hold and the CAS does not.
#[derive(Allocative)]
pub struct DigestInterner {
    /// Dropping a handle takes its shard's write lock, so never construct or drop a
    /// `TrackedFileDigest` while holding a guard on this map; anything that walks it
    /// ([`sweep`](Self::sweep)) or looks into it ([`peek`](Self::peek)) works on the bare
    /// `Arc`s and hands out handles only once the guard is gone.
    ///
    /// The value is whether a row on disk holds the digest; see [`page_out`](Self::page_out).
    /// It lives here rather than in the entry so that it is read and written under the same
    /// shard lock that decides removal.
    entries: BuckDashMap<InternedDigest, bool>,
    /// Entries whose expiration is set; maintained at the transition, since counting them by
    /// walking the table would make snapshots proportional to the table.
    pinned: StripedCounter,
    /// Entries a row on disk holds, likewise maintained at the transition.
    paged: StripedCounter,
    hits: StripedCounter,
    misses: StripedCounter,
    /// Who takes a stash record once its entry no longer carries it, by the stash id in the
    /// record's nonce. One per stash; the daemon has one.
    #[allocative(skip)]
    stash_collectors: RwLock<Vec<(u32, StashCollector)>>,
}

/// A counter spread over 32 cache lines, threads assigned to them round robin as they are
/// created: an atomic add that at most a few threads share for the counting thread, a sum
/// over the stripes for the rare snapshot. Being boxed also keeps the stripes out of the line
/// that holds [`FILE_DIGEST_INTERNER`]'s initialization state, which every
/// `TrackedFileDigest::drop` reads.
///
/// A snapshot reads the stripes one by one while other threads keep adding and subtracting,
/// so it is approximate; the sum is clamped so that a decrement observed before its increment
/// cannot wrap.
#[derive(Allocative)]
struct StripedCounter {
    stripes: Box<[CounterStripe; STRIPES]>,
}

const STRIPES: usize = 32;

#[derive(Allocative)]
#[repr(align(128))]
struct CounterStripe(AtomicI64);

impl StripedCounter {
    fn new() -> Self {
        Self {
            stripes: Box::new(std::array::from_fn(|_| CounterStripe(AtomicI64::new(0)))),
        }
    }

    #[inline]
    fn stripe(&self) -> &AtomicI64 {
        thread_local! {
            static STRIPE: usize = {
                static NEXT: AtomicUsize = AtomicUsize::new(0);
                NEXT.fetch_add(1, Ordering::Relaxed) % STRIPES
            };
        }
        &self.stripes[STRIPE.with(|s| *s)].0
    }

    #[inline]
    fn add(&self, n: u64) {
        self.stripe().fetch_add(n as i64, Ordering::Relaxed);
    }

    #[inline]
    fn sub(&self, n: u64) {
        self.stripe().fetch_sub(n as i64, Ordering::Relaxed);
    }

    fn get(&self) -> u64 {
        let sum: i128 = self
            .stripes
            .iter()
            .map(|s| s.0.load(Ordering::Relaxed) as i128)
            .sum();
        sum.max(0) as u64
    }
}

/// What a stash registers to be handed its records: see [`DigestInterner::add_stash_collector`].
pub type StashCollector = Box<dyn Fn(&FileDigest, StashRecord) + Send + Sync>;

/// A table entry. Hashes and compares as the digest it holds, so lookups take a `FileDigest`.
#[derive(Allocative)]
struct InternedDigest(Arc<TrackedFileDigestInner>);

impl Hash for InternedDigest {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.data.hash(state)
    }
}

impl PartialEq for InternedDigest {
    fn eq(&self, other: &Self) -> bool {
        self.0.data == other.0.data
    }
}

impl Eq for InternedDigest {}

impl Borrow<FileDigest> for InternedDigest {
    fn borrow(&self) -> &FileDigest {
        &self.0.data
    }
}

/// A point-in-time view of a [`DigestInterner`], for telemetry.
#[derive(Clone, Copy, Debug, Default, Dupe, PartialEq, Eq)]
pub struct DigestInternerStats {
    /// Live entries, one per distinct digest.
    pub entries: u64,
    /// Entries kept alive past their last handle because a CAS expiration is recorded for them.
    pub pinned: u64,
    /// Entries a row on disk holds; they live as long as the daemon.
    pub paged: u64,
    /// Constructions that found an existing entry, cumulatively.
    pub hits: u64,
    /// Constructions that had to create an entry, cumulatively.
    pub misses: u64,
}

/// What one [`DigestInterner::sweep`] found and did.
#[derive(Clone, Copy, Debug, Default, Dupe, PartialEq, Eq)]
pub struct SweepStats {
    /// Entries the sweep looked at.
    pub visited: u64,
    /// Entries it removed: ones nothing but the table held, whose verdict was to drop them.
    pub swept: u64,
}

/// What a sweep does with one entry, decided by the caller for each.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    Keep,
    /// Remove the entry from the table. Honored only for an entry nothing holds: one with
    /// handles has to stay, so that they keep pointing at the table's entry.
    Drop,
    /// Keep the entry and hand the caller a handle to it, once the sweep is over.
    Handle,
}

/// One table entry as it was the moment [`DigestInterner::sweep`] or [`DigestInterner::peek`]
/// looked at it.
#[derive(Clone, Copy, Debug)]
pub struct DigestEntry {
    pub data: FileDigest,
    /// Seconds since the unix epoch at which the CAS last said the blob expires; zero when it was
    /// never asked.
    pub expires_secs: i64,
    /// The stash entry recorded for the blob, if any.
    pub stash: Option<StashRecord>,
    /// Whether anything but the table refers to the entry: a handle, or a row on disk.
    pub referenced: bool,
}

impl DigestEntry {
    fn of(inner: &Arc<TrackedFileDigestInner>, paged: bool) -> Self {
        Self {
            data: inner.data.dupe(),
            expires_secs: inner.expires.load(Ordering::Relaxed),
            stash: StashRecord::unpack(inner.stash.load(Ordering::Relaxed)),
            referenced: Arc::count(inner) > 1 || paged,
        }
    }
}

impl Default for DigestInterner {
    fn default() -> Self {
        Self::new()
    }
}

impl DigestInterner {
    pub fn new() -> Self {
        Self {
            entries: BuckDashMap::default(),
            pinned: StripedCounter::new(),
            paged: StripedCounter::new(),
            hits: StripedCounter::new(),
            misses: StripedCounter::new(),
            stash_collectors: RwLock::new(Vec::new()),
        }
    }

    /// Registers who takes the records of stash `id` once their entries no longer carry them: at
    /// the last handle's drop, at a sweep that drops the entry, or when a CAS expiration is
    /// recorded for the blob, which the stash then need not hold. The collector runs on
    /// whichever thread did that, with no lock of the table held, and must not block.
    pub fn add_stash_collector(&self, id: u32, collector: StashCollector) {
        self.stash_collectors.write().push((id, collector));
    }

    /// Hands the stash record of `handle`'s entry to its collector, if there is one: for a blob
    /// the CAS turns out to hold.
    pub fn evict_stash(&self, handle: &TrackedFileDigest) {
        self.evict_stash_of(&handle.inner);
    }

    fn evict_stash_of(&self, inner: &TrackedFileDigestInner) {
        if let Some(record) = inner.take_stash() {
            self.collect_stash(&inner.data, record);
        }
    }

    fn collect_stash(&self, data: &FileDigest, record: StashRecord) {
        for (id, collector) in self.stash_collectors.read().iter() {
            if *id == record.stash_id() {
                collector(data, record);
            }
        }
    }

    /// The live entry for `data`, if there is one.
    pub fn get(&self, data: &FileDigest) -> Option<TrackedFileDigest> {
        let inner = self.entries.get(data)?.key().0.clone();
        Some(TrackedFileDigest { inner })
    }

    pub fn stats(&self) -> DigestInternerStats {
        DigestInternerStats {
            entries: self.entries.len() as u64,
            pinned: self.pinned.get(),
            paged: self.paged.get(),
            hits: self.hits.get(),
            misses: self.misses.get(),
        }
    }

    /// One walk over the table, with a verdict from `decide` for every entry: dropped, kept, or
    /// kept with a handle handed to the caller afterwards.
    ///
    /// Together with `Self::dropped` the removal is the whole lifetime story of an entry: an
    /// unpinned entry normally leaves with its last handle, a pinned one stays until a sweep drops
    /// it, and either kind that the drop-side check missed is caught here. The time between a
    /// pinned entry's last drop and the next sweep is deliberate: it is the window in which a
    /// recorded expiration keeps answering for a blob that a later construction asks about again,
    /// which is what makes an RE input tree that is dropped right after its upload still pay off
    /// for the next action. A dropped entry's stash record goes to the stash's collector.
    ///
    /// `decide` runs under a shard's write lock and sees entries as they are at that moment, so
    /// it must not construct or drop a `TrackedFileDigest`, which takes that same lock. The
    /// handles are made only after every lock is gone, which is what makes it safe to hold or
    /// drop them freely afterwards. A sweep holds each shard's write lock while it visits that
    /// shard, so it is meant to run rarely, not per build.
    pub fn sweep(
        &self,
        mut decide: impl FnMut(&DigestEntry) -> Verdict,
    ) -> (SweepStats, Vec<TrackedFileDigest>) {
        let mut stats = SweepStats::default();
        let mut pinned_swept = 0u64;
        let mut picked = Vec::new();
        let mut orphaned = Vec::new();
        self.entries.retain(|entry, paged| {
            stats.visited += 1;
            let view = DigestEntry::of(&entry.0, *paged);
            match decide(&view) {
                Verdict::Drop if !view.referenced => {
                    stats.swept += 1;
                    if entry.0.has_expiry() {
                        pinned_swept += 1;
                    }
                    if let Some(record) = entry.0.take_stash() {
                        orphaned.push((view.data, record));
                    }
                    false
                }
                Verdict::Drop | Verdict::Keep => true,
                Verdict::Handle => {
                    picked.push(entry.0.clone());
                    true
                }
            }
        });
        if pinned_swept > 0 {
            self.pinned.sub(pinned_swept);
        }
        for (data, record) in orphaned {
            self.collect_stash(&data, record);
        }
        let handles = picked
            .into_iter()
            .map(|inner| TrackedFileDigest { inner })
            .collect();
        (stats, handles)
    }

    /// The table's entry for `data` as it is right now, if there is one. Looking hands out no
    /// handle, so whether anything but the table holds the entry is not changed by the looking.
    pub fn peek(&self, data: &FileDigest) -> Option<DigestEntry> {
        self.entries
            .get(data)
            .map(|entry| DigestEntry::of(&entry.key().0, *entry.value()))
    }

    /// Handles for this table's kind come into existence through [`TrackedFileDigest`]'s
    /// constructors, which intern into the kind's own table; calling this directly is for tests
    /// over a private table.
    pub fn intern(&self, data: FileDigest, expires: i64) -> TrackedFileDigest {
        // The common case is a hit, which only needs a shard read lock and no allocation.
        let existing = self.entries.get(&data).map(|entry| entry.key().0.clone());
        let inner = match existing {
            Some(inner) => inner,
            None => {
                let candidate = Arc::new(TrackedFileDigestInner::new(data, expires.max(0)));
                // `or_insert` is the atomic get-or-insert; whoever loses the race between the
                // read above and this write adopts the winner's entry and drops its candidate,
                // which is a bare `Arc` and so never reaches `dropped`.
                let entry = self
                    .entries
                    .entry(InternedDigest(candidate.clone()))
                    .or_insert(false);
                let inner = entry.key().0.clone();
                drop(entry);
                if Arc::ptr_eq(&inner, &candidate) {
                    self.misses.add(1);
                    if expires > 0 {
                        self.pinned.add(1);
                    }
                    return TrackedFileDigest { inner };
                }
                inner
            }
        };
        self.hits.add(1);
        if inner.extend_expires(expires) {
            self.pinned.add(1);
        }
        // A blob the CAS holds is the CAS's to serve: the stash lets go of it.
        if expires > 0
            && inner.stash.load(Ordering::Relaxed) != 0
            && expires > jiff::Timestamp::now().as_second()
        {
            self.evict_stash_of(&inner);
        }
        TrackedFileDigest { inner }
    }

    /// Records that a row on disk holds this digest, so that the entry outlives the handles that
    /// went with it: its expiration keeps being refreshed, and a value read back from the row
    /// finds the same entry, with the refreshed expiration rather than the one written with it.
    ///
    /// Nothing undoes this. A row is never removed while the daemon runs, and its value may be
    /// read back any number of times, after each of which it can be evicted again without being
    /// rewritten; so no page-in can tell that the last reference from disk is gone. The cost is
    /// the entry's 64 bytes and a TTL refresh until the daemon exits, per digest ever paged out.
    fn page_out(&self, data: &FileDigest) {
        // The entry is there: the caller holds a handle to it.
        if let Some(mut entry) = self.entries.get_mut(data)
            && !std::mem::replace(entry.value_mut(), true)
        {
            self.paged.add(1);
        }
    }

    /// Called for every handle drop. Removes the entry when this handle is the last one and
    /// the entry is not pinned, and hands its stash record, if any, to the stash's collector.
    ///
    /// "Last one" is read off the `Arc`'s count, which cannot be done exactly: two threads
    /// dropping the last two handles of one digest at the same time can both read a count of
    /// three and both skip the removal, leaving an entry that only the table refers to. Such an
    /// entry is indistinguishable from a pinned one to everything but a sweep (it is simply an
    /// unpinned entry with a count of one), keeps serving later constructions of the digest in
    /// the meantime, and costs its 64 bytes plus a table slot until the sweep reclaims it. Exact
    /// detection means owning the refcount: a thin `Arc` whose count is its first word keeps the
    /// allocation size, and the decrement's previous value says whether this was the last
    /// external handle. That is unsafe code, not memory, and it waits until the sweep's telemetry
    /// shows that stranding matters.
    fn dropped(&self, inner: &Arc<TrackedFileDigestInner>) {
        // Cheap checks first: the table's own reference plus this handle make two. Whether a page
        // refers to the entry is only known under the lock, and is checked there.
        if Arc::count(inner) != 2 || inner.is_pinned() {
            return;
        }
        // Between the check above and taking the shard's write lock, another thread may have
        // interned the same digest (the count is then three or more) or this allocation may no
        // longer be the table's entry for it (a previous drop removed it and a later intern
        // inserted a fresh one). Deciding under the lock keeps every live handle pointing at
        // an entry the table still holds.
        let removed = self.entries.remove_if(&inner.data, |entry, paged| {
            Arc::ptr_eq(&entry.0, inner)
                && Arc::count(&entry.0) == 2
                && !entry.0.is_pinned()
                && !*paged
        });
        if let Some((entry, _)) = removed
            && let Some(record) = entry.0.take_stash()
        {
            self.collect_stash(&entry.0.data, record);
        }
    }
}

#[cfg(test)]
mod tests {
    use pagable::PagableDeserialize;
    use pagable::PagableSerialize;

    use super::*;
    use crate::cas_digest::DigestAlgorithmFamily;
    use crate::cas_digest::testing;
    use crate::file_ops::metadata::FileDigest;

    fn digest_of(content: &[u8]) -> FileDigest {
        FileDigest::from_content(content, testing::sha1())
    }

    fn interner() -> &'static DigestInterner {
        file_digest_interner()
    }

    /// The verdict of a sweep that only collects: drop what nothing holds, keep the rest.
    fn drop_unreferenced(entry: &DigestEntry) -> Verdict {
        if entry.referenced {
            Verdict::Keep
        } else {
            Verdict::Drop
        }
    }

    #[test]
    fn test_new_expires_empty_extends_the_shared_singleton() {
        // For the empty digest every constructor returns the per-config singleton. The empty blob
        // is in every CAS, so an expiration reported for it holds for every other holder too,
        // and recording it on the shared instance is right; lowering it would not be, which is
        // what the monotone update rules out.
        let config = testing::sha1();
        let singleton = TrackedFileDigest::empty(config);

        let requested = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(24 * 7);
        let from_empty =
            TrackedFileDigest::new_expires(FileDigest::empty(config), requested, config);
        assert!(from_empty.ptr_eq(&singleton));
        assert_eq!(
            singleton.expires().unwrap().as_second(),
            requested.as_second()
        );

        let earlier = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(24);
        let again = TrackedFileDigest::new_expires(FileDigest::empty(config), earlier, config);
        assert!(again.ptr_eq(&singleton));
        assert_eq!(
            singleton.expires().unwrap().as_second(),
            requested.as_second(),
            "an earlier alive expiration must not lower the recorded one"
        );
    }

    /// A digest declared with size 0 but a hash other than the empty hash names nothing in a CAS;
    /// it is kept as declared rather than replaced by the config's empty-file digest.
    #[test]
    fn test_new_with_size_zero_keeps_the_declared_hash() {
        let config = testing::sha1();
        let foo = digest_of(b"foo");
        let declared = FileDigest::new(*foo.raw_digest(), 0);

        let tracked = TrackedFileDigest::new(declared.dupe(), config);
        assert!(!tracked.ptr_eq(&TrackedFileDigest::empty(config)));
        assert_eq!(tracked.data(), &declared);

        let expiry = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1);
        let tracked = TrackedFileDigest::new_expires(declared.dupe(), expiry, config);
        assert!(!tracked.ptr_eq(&TrackedFileDigest::empty(config)));
        assert_eq!(tracked.data(), &declared);
        assert_eq!(tracked.expires().unwrap().as_second(), expiry.as_second());

        // The empty digest of an allowed non-preferred algorithm is the shared empty file.
        let sha256_sha1 = testing::sha256_sha1();
        let tracked = TrackedFileDigest::new(FileDigest::empty(testing::sha1()), sha256_sha1);
        assert!(tracked.ptr_eq(&TrackedFileDigest::empty(sha256_sha1)));
        assert_eq!(
            tracked.raw_digest().algorithm(),
            DigestAlgorithmFamily::Sha256
        );
    }

    #[test]
    fn test_interning_shares_one_allocation_per_digest() {
        let config = testing::sha1();
        let content = b"test_interning_shares_one_allocation_per_digest";
        let a = TrackedFileDigest::from_content(content, config);
        let b = TrackedFileDigest::new(digest_of(content), config);
        let c = interner().get(a.data()).unwrap();
        assert!(a.ptr_eq(&b));
        assert!(a.ptr_eq(&c));
        assert_eq!(a.expires().unwrap(), jiff::Timestamp::UNIX_EPOCH);
    }

    #[test]
    fn test_unpinned_entry_is_removed_on_last_drop() {
        let config = testing::sha1();
        let content = b"test_unpinned_entry_is_removed_on_last_drop";
        let data = digest_of(content);
        let a = TrackedFileDigest::from_content(content, config);
        let b = a.dupe();
        drop(a);
        assert!(interner().get(&data).is_some());
        drop(b);
        assert!(interner().get(&data).is_none());

        // A fresh construction after removal gets a fresh entry.
        let c = TrackedFileDigest::from_content(content, config);
        assert!(interner().get(&data).unwrap().ptr_eq(&c));
    }

    #[test]
    fn test_non_positive_expirations_record_nothing() {
        // `re_expiration_from_ttl` clamps nonsense TTLs to the minimum timestamp. Such a value
        // must neither be recorded nor pin the entry, or the pin count and `is_pinned` disagree.
        let config = testing::sha1();
        let content = b"test_non_positive_expirations_record_nothing";
        let data = digest_of(content);
        let a = TrackedFileDigest::new_expires(data.dupe(), jiff::Timestamp::MIN, config);
        assert_eq!(a.expires().unwrap(), jiff::Timestamp::UNIX_EPOCH);
        a.update_expires(jiff::Timestamp::MIN);
        assert_eq!(a.expires().unwrap(), jiff::Timestamp::UNIX_EPOCH);
        drop(a);
        assert!(
            interner().get(&data).is_none(),
            "nothing was recorded, so the entry is unpinned and goes with its last handle"
        );
    }

    #[test]
    fn test_pinned_entry_survives_last_drop() {
        let config = testing::sha1();
        let content = b"test_pinned_entry_survives_last_drop";
        let data = digest_of(content);
        let expiry = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1);

        let a = TrackedFileDigest::new_expires(data.dupe(), expiry, config);
        drop(a);
        let survivor = interner()
            .get(&data)
            .expect("an entry with a recorded expiration outlives its last handle");
        assert_eq!(survivor.expires().unwrap().as_second(), expiry.as_second());

        // Pinning through `update_expires` on an existing entry works the same way.
        let content = b"test_pinned_entry_survives_last_drop (update_expires)";
        let data = digest_of(content);
        let b = TrackedFileDigest::from_content(content, config);
        b.update_expires(expiry);
        drop(b);
        assert!(interner().get(&data).is_some());
    }

    #[test]
    fn test_new_expires_on_a_hit_extends_the_shared_entry() {
        let config = testing::sha1();
        let content = b"test_new_expires_on_a_hit_extends_the_shared_entry";
        let a = TrackedFileDigest::from_content(content, config);
        let expiry = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(2);
        let b = TrackedFileDigest::new_expires(digest_of(content), expiry, config);
        assert!(a.ptr_eq(&b));
        assert_eq!(a.expires().unwrap().as_second(), expiry.as_second());
    }

    #[test]
    fn test_update_expires_is_monotone_for_alive_and_overriding_for_expired() {
        let config = testing::sha1();
        let content = b"test_update_expires_is_monotone_for_alive_and_overriding_for_expired";
        let d = TrackedFileDigest::from_content(content, config);
        let now = jiff::Timestamp::now();
        let later = now + jiff::SignedDuration::from_hours(3);
        let sooner = now + jiff::SignedDuration::from_hours(1);
        let past = now - jiff::SignedDuration::from_secs(1);

        d.update_expires(later);
        d.update_expires(sooner);
        assert_eq!(d.expires().unwrap().as_second(), later.as_second());

        d.update_expires(past);
        assert_eq!(
            d.expires().unwrap().as_second(),
            past.as_second(),
            "a report that the blob is gone replaces the recorded expiration"
        );

        d.update_expires(sooner);
        assert_eq!(
            d.expires().unwrap().as_second(),
            sooner.as_second(),
            "a later alive report replaces an expired one"
        );
    }

    #[test]
    fn test_concurrent_interning_shares_the_anchored_entry() {
        let config = testing::sha1();
        let content = b"test_concurrent_interning_shares_the_anchored_entry";
        let data = digest_of(content);
        let anchor = TrackedFileDigest::from_content(content, config);

        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..2000 {
                        let d = TrackedFileDigest::from_content(content, config);
                        assert!(d.ptr_eq(&anchor));
                        assert!(interner().get(&data).unwrap().ptr_eq(&anchor));
                    }
                });
            }
        });

        drop(anchor);
        assert!(interner().get(&data).is_none());
    }

    #[test]
    fn test_concurrent_interning_and_dropping_never_loses_a_handle() {
        // Entries come and go under the droppers' feet here. Every handle must observe a valid
        // entry for its digest and the table must hold at most one entry for it; whether that
        // entry is still present at the end is deliberately not asserted (see `dropped`).
        let config = testing::sha1();
        let content = b"test_concurrent_interning_and_dropping_never_loses_a_handle";
        let data = digest_of(content);

        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..2000 {
                        let d = TrackedFileDigest::from_content(content, config);
                        assert_eq!(d.data(), &data);
                        assert_eq!(d.expires().unwrap(), jiff::Timestamp::UNIX_EPOCH);
                        if let Some(live) = interner().get(&data) {
                            assert_eq!(live.data(), &data);
                        }
                    }
                });
            }
        });

        if let Some(stranded) = interner().get(&data) {
            assert_eq!(stranded.data(), &data);
        }
    }

    #[test]
    fn test_sweep_removes_what_only_the_table_holds() {
        // A private table, so that the counts are not shared with the rest of the test binary.
        // Its handles are dropped through the global interner, which does not know these
        // entries, so their last drop strands them exactly as the race in `dropped` would.
        let table = DigestInterner::new();
        let expiry = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(4);

        let stranded = digest_of(b"test_sweep_removes_what_only_the_table_holds stranded");
        let pinned = digest_of(b"test_sweep_removes_what_only_the_table_holds pinned");
        let held = digest_of(b"test_sweep_removes_what_only_the_table_holds held");
        let held_pinned = digest_of(b"test_sweep_removes_what_only_the_table_holds held pinned");

        drop(table.intern(stranded.dupe(), 0));
        drop(table.intern(pinned.dupe(), expiry.as_second()));
        let _held = table.intern(held.dupe(), 0);
        let _held_pinned = table.intern(held_pinned.dupe(), expiry.as_second());
        assert_eq!(table.stats().entries, 4);
        assert_eq!(table.stats().pinned, 2);

        assert_eq!(
            table.sweep(drop_unreferenced).0,
            SweepStats {
                visited: 4,
                swept: 2
            }
        );
        assert!(table.get(&stranded).is_none());
        assert!(table.get(&pinned).is_none());
        assert!(table.get(&held).is_some());
        assert!(table.get(&held_pinned).is_some());
        assert_eq!(table.stats().entries, 2);
        assert_eq!(table.stats().pinned, 1);

        assert_eq!(
            table.sweep(drop_unreferenced).0,
            SweepStats {
                visited: 2,
                swept: 0
            }
        );
    }

    #[test]
    fn test_sweep_hands_out_handles_to_the_survivors_it_picks() {
        let table = DigestInterner::new();
        let data = digest_of(b"test_sweep_hands_out_handles_to_the_survivors_it_picks");
        let other = digest_of(b"test_sweep_hands_out_handles_to_the_survivors_it_picks other");
        let expiry = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(4);
        let live = table.intern(data.dupe(), expiry.as_second());
        let _other = table.intern(other.dupe(), 0);

        let (stats, picked) = table.sweep(|entry| {
            if !entry.referenced {
                Verdict::Drop
            } else if entry.data == data {
                assert_eq!(entry.expires_secs, expiry.as_second());
                Verdict::Handle
            } else {
                Verdict::Keep
            }
        });
        assert_eq!(
            stats,
            SweepStats {
                visited: 2,
                swept: 0
            }
        );
        assert_eq!(picked.len(), 1);
        assert!(picked[0].ptr_eq(&live));

        // `peek` sees an entry without becoming a holder of it. The entry is pinned, so it
        // outlives its last handle (dropped through the global interner, which does not know
        // this table's entries, so nothing removes it either way).
        drop(picked);
        drop(live);
        let entry = table
            .peek(&data)
            .expect("a pinned entry outlives its last handle");
        assert!(!entry.referenced);
        assert_eq!(entry.expires_secs, expiry.as_second());
        assert!(table.peek(&other).unwrap().referenced);
        assert!(table.peek(&digest_of(b"never interned")).is_none());
    }

    #[test]
    fn test_sweeping_under_concurrent_interning_and_dropping() {
        // Sweeps race constructions and last drops of the same digests here, on a private table
        // so that the sweeps cannot disturb other tests' entries. A handle must always find its
        // own entry, and only entries nothing holds may disappear.
        let table = DigestInterner::new();
        let digests: Vec<FileDigest> = (0..16)
            .map(|i| {
                digest_of(
                    format!("test_sweeping_under_concurrent_interning_and_dropping {i}").as_bytes(),
                )
            })
            .collect();

        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for round in 0..500 {
                        let data = &digests[round % digests.len()];
                        let handle = table.intern(data.dupe(), 0);
                        let live = table
                            .get(data)
                            .expect("an entry with a live handle is never swept");
                        assert!(live.ptr_eq(&handle));
                    }
                });
            }
            s.spawn(|| {
                for _ in 0..200 {
                    table.sweep(drop_unreferenced);
                }
            });
        });

        // Every handle is gone, so one more sweep empties the table.
        table.sweep(drop_unreferenced);
        assert_eq!(table.stats().entries, 0);
    }

    #[test]
    fn test_an_entry_a_page_refers_to_survives_a_sweep() {
        let table = DigestInterner::new();
        let data = digest_of(b"test_an_entry_a_page_refers_to_survives_a_sweep");
        let expiry = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(4);

        let handle = table.intern(data.dupe(), expiry.as_second());
        table.page_out(&data);
        drop(handle);
        assert_eq!(table.stats().paged, 1);

        // Swept as far as handles go, but the page still holds it, so its expiration is still
        // there to be refreshed and to answer the next construction.
        assert_eq!(table.sweep(drop_unreferenced).0.swept, 0);
        let entry = table.peek(&data).expect("the page holds it");
        assert!(entry.referenced, "the page counts as a holder");
        assert_eq!(entry.expires_secs, expiry.as_second());

        // Nothing undoes it: the row stays on disk, and a second page-out of the same digest
        // counts once.
        table.page_out(&data);
        assert_eq!(table.stats().paged, 1);
        assert_eq!(table.sweep(drop_unreferenced).0.swept, 0);
        assert!(table.get(&data).is_some());
    }

    #[test]
    fn test_a_stash_record_pins_the_entry_until_a_sweep_drops_it() {
        let table = DigestInterner::new();
        let data = digest_of(b"test_a_stash_record_pins_the_entry_until_a_sweep_drops_it");
        let record = StashRecord {
            nonce: 0x1234,
            executable: true,
        };
        let handle = table.intern(data.dupe(), 0);
        assert_eq!(handle.stash(), None);
        assert!(handle.register_stash(record));
        // A second record is a duplicate: the first stays.
        let duplicate = StashRecord {
            nonce: 0x5678,
            executable: false,
        };
        assert!(!handle.register_stash(duplicate));
        assert_eq!(handle.stash(), Some(record));
        assert!(!handle.clear_stash(duplicate));

        // Pinned: the entry outlives its last handle, and a sweep sees the record.
        drop(handle);
        let entry = table.peek(&data).expect("pinned by the stash record");
        assert!(!entry.referenced);
        assert_eq!(entry.stash, Some(record));
        assert_eq!(table.sweep(|_| Verdict::Keep).0.swept, 0);
        assert!(table.peek(&data).is_some());
        assert_eq!(table.sweep(drop_unreferenced).0.swept, 1);
        assert!(table.peek(&data).is_none());

        // Clearing the record unpins: the next last drop removes the entry. Drops go through the
        // global table, so this part runs there.
        let data = digest_of(b"test_a_stash_record_pins_the_entry_until_a_sweep_drops_it, cleared");
        let handle = TrackedFileDigest::new(data.dupe(), testing::sha1());
        assert!(handle.register_stash(record));
        assert!(handle.clear_stash(record));
        assert_eq!(handle.stash(), None);
        drop(handle);
        assert!(interner().peek(&data).is_none());
    }

    #[test]
    fn test_page_in_reuses_the_live_entry() -> pagable::Result<()> {
        use pagable::testing::TestingDeserializer;
        use pagable::testing::TestingSerializer;

        let config = testing::sha1();
        let content = b"test_page_in_reuses_the_live_entry";
        let data = digest_of(content);
        let expiry = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(4);
        let live = TrackedFileDigest::new_expires(data.dupe(), expiry, config);

        let mut serializer = TestingSerializer::new();
        (live.dupe(), live.dupe()).pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let (first, second): (TrackedFileDigest, TrackedFileDigest) =
            PagableDeserialize::pagable_deserialize(&mut deserializer)?;
        assert!(first.ptr_eq(&live));
        assert!(second.ptr_eq(&live));
        assert_eq!(first.expires().unwrap().as_second(), expiry.as_second());

        // The page is a holder of its own, so the entry is still there once the value that was
        // paged out is gone, and the digest that comes back is that same entry.
        let content = b"test_page_in_reuses_the_live_entry (no handle left)";
        let data = digest_of(content);
        let bytes = {
            let paged_out = TrackedFileDigest::new(data.dupe(), config);
            let mut serializer = TestingSerializer::new();
            paged_out.pagable_serialize(&mut serializer)?;
            serializer.finish()
        };
        let entry = interner()
            .get(&data)
            .expect("a page out of memory refers to it");
        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: TrackedFileDigest =
            PagableDeserialize::pagable_deserialize(&mut deserializer)?;
        assert!(restored.ptr_eq(&entry));
        Ok(())
    }

    #[test]
    fn test_interner_counts_hits_misses_and_pins() {
        // A private table, so the counts are not shared with the other tests in this binary.
        // Handles created here still report their drops to the global table, which does not
        // hold them, so that is a no-op; the entries live until this table is dropped.
        let local = DigestInterner::new();
        let data = digest_of(b"test_interner_counts_hits_misses_and_pins");
        let expiry = jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1);

        let a = local.intern(data.dupe(), 0);
        let b = local.intern(data.dupe(), 0);
        assert!(a.ptr_eq(&b));
        assert_eq!(
            local.stats(),
            DigestInternerStats {
                entries: 1,
                pinned: 0,
                paged: 0,
                hits: 1,
                misses: 1
            }
        );

        let c = local.intern(data.dupe(), expiry.as_second());
        assert!(c.ptr_eq(&a));
        assert_eq!(local.stats().pinned, 1);
        assert_eq!(local.stats().hits, 2);

        let other = digest_of(b"test_interner_counts_hits_misses_and_pins (other)");
        let _d = local.intern(other, expiry.as_second());
        assert_eq!(
            local.stats(),
            DigestInternerStats {
                entries: 2,
                pinned: 2,
                paged: 0,
                hits: 2,
                misses: 2
            }
        );
    }
}
