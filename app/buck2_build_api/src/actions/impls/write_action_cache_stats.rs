/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Counters for write actions served from the action cache (`buck2.write_action_cache`), read
//! into the daemon's snapshots.

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

pub struct WriteActionCacheStats {
    /// Lookups that found an entry.
    pub hits: AtomicU64,
    /// Lookups that found none.
    pub misses: AtomicU64,
    /// Lookups the cache did not answer; each one a miss to its write.
    pub lookup_failures: AtomicU64,
    /// Entries recorded.
    pub populated: AtomicU64,
    /// Entries that could not be recorded.
    pub populate_failures: AtomicU64,
    /// Writes uploaded to the CAS and declared from there, without an entry: a cache miss or a
    /// probe miss the machine had no room to make.
    pub uploaded: AtomicU64,
    /// Misses written to their output path.
    pub written: AtomicU64,
    /// Hits compared against what the write rendered, under `verify`.
    pub verified: AtomicU64,
    /// Of those, hits whose content was not what the write rendered.
    pub mismatches: AtomicU64,
}

/// A reading of the counters.
#[derive(Default, Debug, Clone, Copy)]
pub struct WriteActionCacheCounts {
    pub hits: u64,
    pub misses: u64,
    pub lookup_failures: u64,
    pub populated: u64,
    pub populate_failures: u64,
    pub uploaded: u64,
    pub written: u64,
    pub verified: u64,
    pub mismatches: u64,
}

pub static WRITE_ACTION_CACHE_STATS: WriteActionCacheStats = WriteActionCacheStats {
    hits: AtomicU64::new(0),
    misses: AtomicU64::new(0),
    lookup_failures: AtomicU64::new(0),
    populated: AtomicU64::new(0),
    populate_failures: AtomicU64::new(0),
    uploaded: AtomicU64::new(0),
    written: AtomicU64::new(0),
    verified: AtomicU64::new(0),
    mismatches: AtomicU64::new(0),
};

impl WriteActionCacheStats {
    pub fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn counts(&self) -> WriteActionCacheCounts {
        WriteActionCacheCounts {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            lookup_failures: self.lookup_failures.load(Ordering::Relaxed),
            populated: self.populated.load(Ordering::Relaxed),
            populate_failures: self.populate_failures.load(Ordering::Relaxed),
            uploaded: self.uploaded.load(Ordering::Relaxed),
            written: self.written.load(Ordering::Relaxed),
            verified: self.verified.load(Ordering::Relaxed),
            mismatches: self.mismatches.load(Ordering::Relaxed),
        }
    }
}
