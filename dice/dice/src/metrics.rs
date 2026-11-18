/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::OnceLock;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::thread;
use std::thread::ThreadId;

/// Dice metrics.
#[derive(Debug)]
pub struct Metrics {
    pub key_count: usize,
    pub active_transaction_count: u32,
    /// Live branches, roots included.
    pub branch_count: usize,
}

/// Page-in counters for one DICE key type.
#[derive(Debug, Default, Clone, Copy)]
pub struct PageInKeyTypeMetrics {
    /// Values paged in.
    pub count: u64,
    /// Time fetching the top-level serialized blob. Nested `PagableArc`
    /// sub-values are fetched lazily during deserialize, so their I/O is
    /// counted under `deser_us`, not here.
    pub fetch_us: u64,
    /// Deserialize time, including the lazy fetch of nested `PagableArc`
    /// sub-values — dominates for arc-heavy values.
    pub deser_us: u64,
    /// Top-level blob bytes only; excludes nested `PagableArc` sub-values, so
    /// near-zero for arc-heavy values whose payload lives in those arcs.
    pub bytes: u64,
    /// Memory the rebuilt value added, measured from the allocator. Unlike
    /// `bytes` this covers nested `PagableArc` sub-values, and is zero when the
    /// arc came back from the cache. Signed for the same reason as
    /// [`AllocWindow::net_allocated`].
    pub restored_bytes: i64,
}

/// Reads the calling thread's cumulative `(allocated, deallocated)` byte totals.
///
/// Installed by the embedder, because the counters come from the allocator and
/// dice stays allocator-agnostic. Without one, paging memory accounting is off and
/// the totals stay zero.
pub type ThreadAllocCountersFn = fn() -> Option<(u64, u64)>;

static THREAD_ALLOC_COUNTERS: OnceLock<ThreadAllocCountersFn> = OnceLock::new();

/// Enables paging memory accounting. Later calls are ignored.
pub fn set_thread_alloc_counters(f: ThreadAllocCountersFn) {
    let _ignored = THREAD_ALLOC_COUNTERS.set(f);
}

/// Whether the counters can actually be read, which is what separates a real
/// measurement of zero from never having measured. Calls the reader rather than
/// just checking one is installed, since an installed reader still yields
/// nothing when the allocator does not supply the counters.
fn thread_alloc_counters_available() -> bool {
    THREAD_ALLOC_COUNTERS.get().and_then(|f| f()).is_some()
}

/// Net memory movement on one thread across a window of work.
///
/// Only the thread performing the work is measured, which is what makes this
/// attributable: unrelated allocation on other threads cannot leak in, unlike a
/// process-wide RSS or allocator reading. A window that opens and closes on
/// different threads measures zero rather than nonsense, so callers do not have
/// to keep the no-`await`-inside rule in their heads.
///
/// Results are signed. A window can legitimately net an allocation where a
/// freeing one is expected — evicting a key allocates while mutating the graph,
/// and a page-in that reuses a cached arc allocates only scratch — and clamping
/// those to zero would bias the accumulated totals in one direction with no
/// opportunity to cancel.
pub(crate) struct AllocWindow(Option<(ThreadId, u64, u64)>);

impl AllocWindow {
    pub(crate) fn open() -> AllocWindow {
        AllocWindow(
            THREAD_ALLOC_COUNTERS
                .get()
                .and_then(|f| f())
                .map(|(allocated, deallocated)| (thread::current().id(), allocated, deallocated)),
        )
    }

    /// Bytes released since [`AllocWindow::open`], net of anything the window
    /// allocated, and so negative if it allocated more than it freed. Zero when
    /// accounting is off.
    pub(crate) fn net_freed(self) -> i64 {
        self.net(|freed, allocated| freed.saturating_sub(allocated))
    }

    /// Bytes retained since [`AllocWindow::open`], net of anything it freed —
    /// the mirror of [`AllocWindow::net_freed`].
    pub(crate) fn net_allocated(self) -> i64 {
        self.net(|freed, allocated| allocated.saturating_sub(freed))
    }

    fn net(self, f: impl Fn(i64, i64) -> i64) -> i64 {
        let Some((start_thread, start_alloc, start_dealloc)) = self.0 else {
            return 0;
        };
        // Enforced rather than assumed: the counters are per-thread, so a window
        // closed on a different thread would difference two unrelated series.
        // Measuring nothing is better than measuring noise.
        if start_thread != thread::current().id() {
            return 0;
        }
        let Some((end_alloc, end_dealloc)) = THREAD_ALLOC_COUNTERS.get().and_then(|f| f()) else {
            return 0;
        };
        // Same thread, so the allocator's counters have only grown. The casts fit
        // in i64 for any process that has not moved more than 8 EiB.
        f(
            end_dealloc.saturating_sub(start_dealloc) as i64,
            end_alloc.saturating_sub(start_alloc) as i64,
        )
    }
}

/// Cumulative bytes paging has moved out of and back into memory, measured from
/// the allocator rather than inferred from serialized sizes.
///
/// `bytes_offloaded` counts the memory paging frees: values dropped from the graph, and the
/// storage and arc-cache memory released after them. `bytes_restored` counts the memory values
/// take up again when read back. An arc shared with a value that stays resident is never freed,
/// so it contributes nothing — the case a serialized-byte count gets wrong.
#[derive(Default)]
pub(crate) struct PagingMemoryMetrics {
    bytes_offloaded: AtomicI64,
    bytes_restored: AtomicI64,
    /// Graph nodes whose value was actually dropped for a `DataKey`.
    ///
    /// Not part of [`PagingMemorySnapshot`]: that is `None` when the allocator
    /// counters are unreadable, and this count does not depend on them.
    nodes_paged_out: AtomicU64,
    /// Calls to `evict_backed`, and the values they dropped and memory that freed. A subset of
    /// `nodes_paged_out` and `bytes_offloaded`, which also count what page-out evicts.
    evictions: AtomicU64,
    values_evicted: AtomicU64,
    bytes_evicted: AtomicI64,
}

impl PagingMemoryMetrics {
    pub(crate) fn record_offloaded(&self, bytes: i64) {
        self.bytes_offloaded.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn record_restored(&self, bytes: i64) {
        self.bytes_restored.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn record_nodes_paged_out(&self, nodes: u64) {
        self.nodes_paged_out.fetch_add(nodes, Ordering::Relaxed);
    }

    pub(crate) fn record_eviction(&self) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_evicted(&self, values: u64, bytes: i64) {
        self.values_evicted.fetch_add(values, Ordering::Relaxed);
        self.bytes_evicted.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Cumulative graph nodes evicted since daemon start.
    pub(crate) fn nodes_paged_out(&self) -> u64 {
        self.nodes_paged_out.load(Ordering::Relaxed)
    }

    pub(crate) fn eviction_snapshot(&self) -> EvictionSnapshot {
        EvictionSnapshot {
            evictions: self.evictions.load(Ordering::Relaxed),
            values_evicted: self.values_evicted.load(Ordering::Relaxed),
            bytes_evicted: thread_alloc_counters_available()
                .then(|| self.bytes_evicted.load(Ordering::Relaxed)),
        }
    }

    /// `None` when the allocator counters cannot be read. The totals would be
    /// zero in that case, which reads as "paging moved nothing" rather than
    /// "nothing was measured".
    pub(crate) fn snapshot(&self) -> Option<PagingMemorySnapshot> {
        if !thread_alloc_counters_available() {
            return None;
        }
        Some(PagingMemorySnapshot {
            bytes_offloaded: self.bytes_offloaded.load(Ordering::Relaxed),
            bytes_restored: self.bytes_restored.load(Ordering::Relaxed),
        })
    }
}

/// Cumulative memory moved by paging since daemon start. `bytes_offloaded -
/// bytes_restored` is an upper bound on the memory paging is currently saving: it
/// does not subtract values that were paged out and later invalidated, whose
/// saving would have been reclaimed anyway.
///
/// Signed, because the per-window measurements are: a window that allocated more
/// than it freed moves a total backwards rather than contributing zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagingMemorySnapshot {
    pub bytes_offloaded: i64,
    pub bytes_restored: i64,
}

impl PagingMemorySnapshot {
    /// The memory moved between `earlier` and this snapshot. Either total may be
    /// lower than in `earlier` — the underlying measurements are signed, so a
    /// window that netted an allocation moves a total backwards — and a negative
    /// result is a real one, not a sign of mismatched snapshots.
    pub fn since(self, earlier: PagingMemorySnapshot) -> PagingMemorySnapshot {
        PagingMemorySnapshot {
            bytes_offloaded: self.bytes_offloaded.saturating_sub(earlier.bytes_offloaded),
            bytes_restored: self.bytes_restored.saturating_sub(earlier.bytes_restored),
        }
    }
}

/// Cumulative evictions of backed values (see `Dice::evict_backed`) since daemon start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvictionSnapshot {
    /// Eviction passes, counted when they start, including ones that found nothing to evict.
    pub evictions: u64,
    pub values_evicted: u64,
    /// Memory the evictions freed, including the storage and arc-cache memory released after
    /// them, measured from the allocator like [`PagingMemorySnapshot::bytes_offloaded`] and
    /// signed for the same reason. Excludes what returning freed pages to the OS saves. `None`
    /// when the allocator counters cannot be read.
    pub bytes_evicted: Option<i64>,
}

impl EvictionSnapshot {
    /// The evictions between `earlier` and this snapshot.
    pub fn since(self, earlier: EvictionSnapshot) -> EvictionSnapshot {
        EvictionSnapshot {
            evictions: self.evictions.saturating_sub(earlier.evictions),
            values_evicted: self.values_evicted.saturating_sub(earlier.values_evicted),
            bytes_evicted: self
                .bytes_evicted
                .zip(earlier.bytes_evicted)
                .map(|(now, then)| now.saturating_sub(then)),
        }
    }
}
