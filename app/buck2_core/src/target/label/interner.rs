/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use lock_free_hashtable::sharded::ShardedLockFreeRawTable;

use crate::package::PackageLabel;
use crate::target::label::label::OwnedTargetLabel;
use crate::target::label::label::TargetLabel;
use crate::target::name::TargetNameRef;

/// Every `TargetLabel` ever created lives here, forever.
///
/// Entries are never removed: `TargetLabel` is a non-refcounted `Copy` handle
/// whose soundness relies on this table never dropping an entry.
#[allocative::root]
static GLOBAL_TARGET_LABEL_INTERNER: ShardedLockFreeRawTable<OwnedTargetLabel, 64> =
    ShardedLockFreeRawTable::new();

/// Backing storage for every label allocation. Reported to allocative as
/// slack only; payload bytes are accounted per entry via `label_data`.
#[allocative::root]
static LABEL_ARENAS: crate::target::label::arena::LabelArenas =
    crate::target::label::arena::LabelArenas::new();

pub(in crate::target::label) fn label_arenas() -> &'static crate::target::label::arena::LabelArenas
{
    &LABEL_ARENAS
}

pub(crate) fn global_intern(pkg: PackageLabel, name: &TargetNameRef) -> TargetLabel {
    let hash = OwnedTargetLabel::label_hash(pkg, name);
    if let Some(label) = GLOBAL_TARGET_LABEL_INTERNER
        .lookup(hash, |label| label.pkg() == pkg && label.name() == name)
    {
        return label;
    }
    let owned = OwnedTargetLabel::alloc(pkg, name, hash);
    let (label, loser) = GLOBAL_TARGET_LABEL_INTERNER.insert(
        hash,
        owned,
        // `a` may be a handle to a not-yet-inserted candidate that is
        // abandoned if it loses the insert race: compare by value only, and
        // never let these handles escape (`==` on them would also be
        // wrong: it is pointer equality, always false here).
        |a, b| a.pkg() == b.pkg() && a.name() == b.name(),
        |label| OwnedTargetLabel::label_hash(label.pkg(), label.name()),
    );
    if let Some(loser) = loser {
        // The losing candidate's carve is never reused; reclassify its bytes
        // as slack so allocative accounting stays exact.
        label_arenas().abandon(loser.arena_size());
    }
    label
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use buck2_hash::BuckMutMap;

    use crate::target::label::label::TargetLabel;

    #[test]
    #[cfg_attr(
        all(miri, windows),
        ignore = "a contended `parking_lot` lock calls `GetModuleHandleA`, which Miri does not implement"
    )]
    fn test_concurrent_interning_is_canonical() {
        // Race many threads interning the same small label set to exercise
        // the losing side of concurrent inserts.
        const DISTINCT: usize = if cfg!(miri) { 10 } else { 100 };
        let labels: Vec<TargetLabel> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    s.spawn(|| {
                        (0..10 * DISTINCT)
                            .map(|i| {
                                TargetLabel::testing_parse(&format!("foo//pkg:t{}", i % DISTINCT))
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        });

        let mut canonical: BuckMutMap<String, *const ()> = BuckMutMap::default();
        for label in &labels {
            let ptr = canonical.entry(label.to_string()).or_insert(label.as_raw());
            assert!(
                ptr::eq(*ptr, label.as_raw()),
                "equal labels must share one allocation"
            );
        }
        assert_eq!(DISTINCT, canonical.len());
    }

    #[test]
    fn test_global_interning_canonicalizes() {
        let label1 = TargetLabel::testing_parse("foo//:bar");
        let label2 = TargetLabel::testing_parse("foo//:bar");
        assert!(
            ptr::eq(label1.as_raw(), label2.as_raw()),
            "same label must be the same allocation"
        );
        assert_eq!(label1, label2);

        let other = TargetLabel::testing_parse("foo//:baz");
        assert!(
            !ptr::eq(label1.as_raw(), other.as_raw()),
            "different labels must be different allocations"
        );
        assert_ne!(label1, other);
    }
}
