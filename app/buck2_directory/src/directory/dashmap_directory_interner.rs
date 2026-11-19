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
use std::sync::Weak;

use allocative::Allocative;
use buck2_core::directory_digest::DirectoryDigest;
use buck2_hash::BuckDashMap;
use dashmap::mapref::entry::Entry;
use dupe::Clone_;
use dupe::Dupe;
use dupe::Dupe_;

use crate::directory::exhaustiveness::ExhaustivenessHash;
use crate::directory::shared_directory::SharedDirectory;
use crate::directory::shared_directory::SharedDirectoryData;
use crate::directory::shared_directory::SharedDirectoryInner;

/// Deduplicates directories by identity: the fingerprint plus the exhaustiveness hash.
///
/// The map is keyed by fingerprint alone and holds one directory per fingerprint; the
/// exhaustiveness half of the identity is checked against the stored directory on lookup.
/// Distinct exhaustiveness-variants of the same content are therefore never conflated, but only
/// the most recently interned variant is deduplicated — an older coexisting variant simply
/// stays alive unshared wherever it is already referenced. Variants are rare enough that
/// spending a wider key (and an owned-key lookup) on deduplicating them is a bad trade.
#[derive(Dupe_, Clone_, Allocative)]
pub struct DashMapDirectoryInterner<L, H>
where
    H: DirectoryDigest,
{
    inner: Arc<BuckDashMap<H, Weak<SharedDirectoryInner<L, H>>>>,
}

impl<L, H> DashMapDirectoryInterner<L, H>
where
    H: DirectoryDigest,
{
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BuckDashMap::default()),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Get an existing entry from the interner.
    pub fn get(
        &self,
        fingerprint: &H,
        exhaustiveness_hash: ExhaustivenessHash,
    ) -> Option<SharedDirectory<L, H>> {
        self.inner
            .get(fingerprint)
            .and_then(|inner| inner.upgrade())
            .filter(|inner| inner.data.exhaustiveness_hash == exhaustiveness_hash)
            .map(|inner| SharedDirectory { inner })
    }

    /// Insert a new entry into the interner. This may insert this data, or return an existing
    /// entry.
    pub fn intern(&self, data: SharedDirectoryData<L, H>) -> SharedDirectory<L, H> {
        // The self.inner.entry call below holds a lock on it's dashmap shard. If we take
        // the Occupied branch and the `inner = o.get().upgrade()` is the last strong
        // reference to inner, dropping inner at the end of the Occupied branch calls
        // `SharedDirectoryInner::drop`->`dropped()`, which locks the same shard, resulting
        // in a deadlock. `displaced` holds the inner strong reference until we're outside
        // the `self.inner.entry` lock.
        let mut displaced = None;

        let new_inner = match self.inner.entry(data.fingerprint.dupe()) {
            Entry::Occupied(mut o) => {
                if let Some(inner) = o.get().upgrade() {
                    if inner.data.exhaustiveness_hash == data.exhaustiveness_hash {
                        return SharedDirectory { inner };
                    }
                    // Same content, different exhaustiveness-variant: take over the slot
                    // (last-wins) so repeated interning of this variant deduplicates from here
                    // on; the previous variant stays alive unshared where it is referenced.
                    displaced = Some(inner);
                }

                // Constructing this here is a bit duplicative but it avoids creating a new
                // SharedDirectoryInner above just to drop it, and avoids an early return possibly
                // calling dropped() below.
                let new_inner = Arc::new(SharedDirectoryInner {
                    data,
                    interner: self.dupe(),
                });

                o.insert(Arc::downgrade(&new_inner));

                new_inner
            }
            Entry::Vacant(v) => {
                let new_inner = Arc::new(SharedDirectoryInner {
                    data,
                    interner: self.dupe(),
                });

                v.insert(Arc::downgrade(&new_inner));

                new_inner
            }
        };

        // The `self.inner.entry` lock is released; dropping this may now re-enter the map safely.
        drop(displaced);

        SharedDirectory { inner: new_inner }
    }

    /// Notify the interner that an entry has been removed.
    pub fn dropped(&self, data: &SharedDirectoryData<L, H>) {
        // Note: we still check the count here, since you could hypothetically have a race where
        // one SharedDirectory data gets released, but by the time the drop code for the
        // SharedDirectoryInner executes (which calls this), another instance has been created.
        // The same check also keeps a variant-replaced slot alive: the dying directory may not
        // be the one the slot currently points to.
        self.inner
            .remove_if(&data.fingerprint, |_, v| Weak::strong_count(v) == 0);
    }
}

#[cfg(test)]
mod tests {
    use buck2_fs::paths::file_name::FileNameBuf;

    use crate::directory::dashmap_directory_interner::DashMapDirectoryInterner;
    use crate::directory::directory_data::DirectoryData;
    use crate::directory::entry::DirectoryEntry;
    use crate::directory::exhaustiveness::Exhaustiveness;
    use crate::directory::shared_directory::SharedDirectoryData;
    use crate::directory::sorted_slice_map::SortedSliceMap;
    use crate::directory::test::NopEntry;
    use crate::directory::test::TestDigest;
    use crate::directory::test::TestDirectoryBuilder;
    use crate::directory::test::TestHasher;
    use crate::directory::test::path;

    #[test]
    fn test_directory_interner() -> buck2_error::Result<()> {
        let interner = DashMapDirectoryInterner::new();

        let d1 = {
            let mut b = TestDirectoryBuilder::empty_non_exhaustive();
            b.insert(path("a/b"), DirectoryEntry::Leaf(NopEntry))?;
            b.fingerprint(&TestHasher).shared(&interner)
        };

        let d2 = {
            let mut b = TestDirectoryBuilder::empty_non_exhaustive();
            b.insert(path("a/b"), DirectoryEntry::Leaf(NopEntry))?;
            b.fingerprint(&TestHasher).shared(&interner)
        };

        assert!(d1.ptr_eq(&d2));

        assert_eq!(interner.len(), 2);

        drop(d1);
        assert_eq!(interner.len(), 2);

        drop(d2);
        assert_eq!(interner.len(), 0);

        Ok(())
    }

    #[test]
    fn test_directory_interner_distinguishes_exhaustiveness() {
        let interner = DashMapDirectoryInterner::new();

        let entries = || {
            SortedSliceMap::from_iter([(
                FileNameBuf::try_from("a".to_owned()).unwrap(),
                DirectoryEntry::Leaf(NopEntry),
            )])
        };

        let scaffold = || -> SharedDirectoryData<NopEntry, TestDigest> {
            DirectoryData::new(entries(), &TestHasher, Exhaustiveness::NonExhaustive)
        };
        let exhaustive = || -> SharedDirectoryData<NopEntry, TestDigest> {
            DirectoryData::new(entries(), &TestHasher, Exhaustiveness::Exhaustive)
        };
        assert_eq!(scaffold().fingerprint(), exhaustive().fingerprint());

        // Coexisting variants of the same fingerprint are never conflated...
        let d1 = interner.intern(scaffold());
        let d2 = interner.intern(exhaustive());
        assert!(!d1.ptr_eq(&d2));
        assert_ne!(d1, d2);

        // ...but they share one slot: the most recently interned variant deduplicates, the
        // older one no longer does.
        assert_eq!(interner.len(), 1);
        let d3 = interner.intern(exhaustive());
        assert!(d2.ptr_eq(&d3));
        let d4 = interner.intern(scaffold());
        assert!(!d1.ptr_eq(&d4));

        assert!(
            interner
                .get(d2.fingerprint(), d2.exhaustiveness_hash())
                .is_none()
        );
        assert!(
            interner
                .get(d4.fingerprint(), d4.exhaustiveness_hash())
                .is_some_and(|d| d.ptr_eq(&d4))
        );

        // The slot survives drops of unmapped variants and dies with the mapped one.
        drop(d1);
        drop(d2);
        drop(d3);
        assert_eq!(interner.len(), 1);
        drop(d4);
        assert_eq!(interner.len(), 0);
    }

    #[test]
    fn test_directory_interner_deep() -> buck2_error::Result<()> {
        let interner = DashMapDirectoryInterner::new();

        let d1 = {
            let mut b = TestDirectoryBuilder::empty_non_exhaustive();
            b.insert(path("a/b"), DirectoryEntry::Leaf(NopEntry))?;
            b.fingerprint(&TestHasher).shared(&interner)
        };

        let _d2 = {
            let mut b = TestDirectoryBuilder::empty_non_exhaustive();
            b.insert(path("b"), DirectoryEntry::Leaf(NopEntry))?;
            b.fingerprint(&TestHasher).shared(&interner)
        };

        assert_eq!(interner.len(), 2);

        drop(d1);

        // Now we only have d2.
        assert_eq!(interner.len(), 1);

        Ok(())
    }
}
