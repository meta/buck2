/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The byte layout of an insertion map's single allocation.
//!
//! ```text
//! mutable, any capacity:            Header [u32; cap] [K; cap] [V; cap] [index]
//! fixed, 1 ..= NO_INDEX_THRESHOLD:         [u32; n]   [K; n]   [V; n]
//! fixed, more than that:            Header [u32; n]   [K; n]   [V; n]   [index]
//! empty:                            no allocation at all
//! ```
//!
//! The index is present exactly when the capacity exceeds [`NO_INDEX_THRESHOLD`], and is two
//! arrays: a tag byte per slot followed by a [`TAIL`]-byte mirror of the first slots, then a
//! position per slot, 32 or 64 bits wide by the capacity ([`index`](super::index)). A large fixed
//! map has the same layout as a mutable map whose capacity equals its length, which is what lets
//! freezing hand the allocation over instead of copying it. Padding between the regions is what
//! `Layout::extend` would insert, computed in closed form here (a test holds the two to agree)
//! because the offsets are on the path of every lookup; every offset comes from here rather than
//! from arithmetic at the use site.

use std::alloc::Layout;
use std::marker::PhantomData;

use crate::insertion_map::index::Pos;
use crate::insertion_map::index::TAIL;

/// Entry count at or below which no index is kept and lookup scans the hashes.
///
/// 32 is where `buck2 cquery` was fastest and used the least memory when this was measured for
/// starlark_map, with the SIMD scan; the scalar scan on stable gets half that.
#[cfg(rust_nightly)]
pub(crate) const NO_INDEX_THRESHOLD: usize = 32;
#[cfg(not(rust_nightly))]
pub(crate) const NO_INDEX_THRESHOLD: usize = 16;

/// Written at the start of every mutable map's allocation and of a large fixed map's.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Header {
    pub(crate) len: usize,
    pub(crate) cap: usize,
}

/// Whether a map with room for `cap` entries keeps an index.
#[inline]
pub(crate) fn has_index(cap: usize) -> bool {
    cap > NO_INDEX_THRESHOLD
}

/// Slots in the index of a map with room for `cap` entries: half again as many as entries, so the
/// table is never more than two thirds full and a probe always reaches an empty slot.
#[inline]
pub(crate) fn index_slots(cap: usize) -> usize {
    if has_index(cap) {
        cap + cap.div_ceil(2)
    } else {
        0
    }
}

/// Whether positions in the index of a map with room for `cap` entries need 64 bits.
#[inline]
pub(crate) fn wide_index(cap: usize) -> bool {
    cap > <u32 as Pos>::MAX_CAP
}

/// Alignment of the positions array at either width, so that where it starts does not depend on
/// which.
const POSITION_ALIGN: usize = align_of::<u64>();

#[inline(always)]
const fn position_size(wide: bool) -> usize {
    if wide {
        size_of::<u64>()
    } else {
        size_of::<u32>()
    }
}

/// The whole allocation's layout, for the code that allocates, frees or accounts for it. Lookups
/// go through [`array_offsets`] and [`index_region`] instead.
///
/// Typed by the key and value, so that the arrays read out of an allocation cannot be of a
/// different type than the layout was computed for.
pub(crate) struct Offsets<K, V> {
    pub(crate) layout: Layout,
    /// Where the entry arrays end and the index, if any, begins.
    pub(crate) index: usize,
    _marker: PhantomData<(K, V)>,
}

impl<K, V> Clone for Offsets<K, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K, V> Copy for Offsets<K, V> {}

/// Round `x` up to a multiple of `align`, which must be a power of two.
#[inline(always)]
const fn align_up(x: usize, align: usize) -> usize {
    (x + align - 1) & !(align - 1)
}

const fn max(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

/// The most entries a layout can be computed for without its arithmetic overflowing.
///
/// Every allocation checks its capacity against this, which is what lets [`offsets`] run on every
/// lookup with plain arithmetic instead of checked `Layout` building.
pub(crate) const fn max_cap<K, V>() -> usize {
    // Bytes per entry, over-estimating the index (never more than 1.5 slots of 9 bytes each per
    // entry), and room to spare for the header, the tail and alignment padding.
    let per_entry = size_of::<u32>() + size_of::<K>() + size_of::<V>() + 2 * size_of::<u64>();
    (isize::MAX as usize - 4096) / per_entry
}

/// Layout of an allocation with room for `cap` entries, led by a [`Header`] or not.
///
/// `cap` must not exceed [`max_cap`]; [`checked_offsets`] is the entry point that enforces that,
/// for the places that allocate.
#[inline]
pub(crate) fn offsets<K, V>(cap: usize, header: bool) -> Offsets<K, V> {
    let (hashes, keys, values) = array_offsets::<K, V>(cap, header);
    let region = index_region::<K, V>(cap, header);
    debug_assert!(hashes <= keys && keys <= values && values <= region.tags);
    let header_align = if header { align_of::<Header>() } else { 1 };
    let mut align = max(
        max(header_align, align_of::<u32>()),
        max(align_of::<K>(), align_of::<V>()),
    );
    let end = if region.slots == 0 {
        region.tags
    } else {
        align = max(align, POSITION_ALIGN);
        region.positions + region.slots * position_size(region.wide)
    };
    // SAFETY: `align` is a power of two, being the maximum of several, and `end` is below
    // `isize::MAX` with room to round up, by the bound on `cap`.
    let layout = unsafe { Layout::from_size_align_unchecked(align_up(end, align), align) };

    Offsets {
        layout,
        index: region.tags,
        _marker: PhantomData,
    }
}

/// Where the hash, key and value arrays start: all a lookup needs to know about the layout.
#[inline(always)]
pub(crate) fn array_offsets<K, V>(cap: usize, header: bool) -> (usize, usize, usize) {
    debug_assert!(cap <= max_cap::<K, V>());
    let header_size = if header { size_of::<Header>() } else { 0 };
    let hashes = align_up(header_size, align_of::<u32>());
    let keys = align_up(hashes + size_of::<u32>() * cap, align_of::<K>());
    let values = align_up(keys + size_of::<K>() * cap, align_of::<V>());
    (hashes, keys, values)
}

/// Where the index lives, if the capacity calls for one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct IndexRegion {
    /// Byte offset of the tags, which the [`TAIL`] follows; also where the entry arrays end.
    pub(crate) tags: usize,
    /// Byte offset of the positions.
    pub(crate) positions: usize,
    /// Slots in the index: zero when there is none.
    pub(crate) slots: usize,
    /// Whether positions are 64 bits wide.
    pub(crate) wide: bool,
}

#[inline(always)]
pub(crate) fn index_region<K, V>(cap: usize, header: bool) -> IndexRegion {
    let (_, _, values) = array_offsets::<K, V>(cap, header);
    let tags = values + size_of::<V>() * cap;
    let slots = index_slots(cap);
    let tag_bytes = if slots == 0 { 0 } else { slots + TAIL };
    IndexRegion {
        tags,
        positions: align_up(tags + tag_bytes, POSITION_ALIGN),
        slots,
        wide: wide_index(cap),
    }
}

/// [`offsets`] for an allocation about to be made; panics if `cap` is more than can be laid out.
pub(crate) fn checked_offsets<K, V>(cap: usize, header: bool) -> Offsets<K, V> {
    assert!(cap <= max_cap::<K, V>(), "insertion map capacity overflow");
    offsets::<K, V>(cap, header)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_capacity_has_no_index() {
        assert_eq!((0, 16, 48), array_offsets::<u64, u64>(4, false));
        assert_eq!(0, index_region::<u64, u64>(4, false).slots);
        assert_eq!(80, offsets::<u64, u64>(4, false).layout.size());
        assert_eq!(20, offsets::<u8, u8>(3, false).layout.size());
    }

    #[test]
    fn header_goes_first() {
        assert_eq!(size_of::<Header>(), array_offsets::<u64, u64>(4, true).0);
        assert_eq!(
            80 + size_of::<Header>(),
            offsets::<u64, u64>(4, true).layout.size()
        );
    }

    #[test]
    fn index_is_never_more_than_two_thirds_full() {
        for cap in [NO_INDEX_THRESHOLD + 1, 50, 100, 1000, 1 << 20] {
            let slots = index_slots(cap);
            assert!(cap * 3 <= slots * 2, "cap={cap} slots={slots}");
            assert!(slots < cap * 2, "cap={cap} slots={slots}");
            assert!(slots >= TAIL, "cap={cap} slots={slots}");
        }
    }

    #[test]
    fn position_width_follows_capacity() {
        let region = index_region::<u8, u8>(1000, true);
        assert!(!region.wide);
        assert_eq!(index_slots(1000), region.slots);
        let size = offsets::<u8, u8>(1000, true).layout.size();
        let positions_end = region.positions + 4 * region.slots;
        assert!(positions_end <= size && size < positions_end + POSITION_ALIGN);
        let big = <u32 as Pos>::MAX_CAP + 1;
        assert!(index_region::<u8, u8>(big, true).wide);
    }

    #[test]
    fn empty_fixed_layout_is_zero_sized() {
        assert_eq!(0, offsets::<u64, u64>(0, false).layout.size());
    }

    /// The arithmetic in `offsets` must agree with what `Layout::extend` would build, which is
    /// the definition of the layout.
    fn check_against_layout_extend<K, V>() {
        for cap in (0..300).chain([1000, 4095, 4096, 100_000, <u32 as Pos>::MAX_CAP + 7]) {
            for header in [false, true] {
                let start = if header {
                    Layout::new::<Header>()
                } else {
                    Layout::from_size_align(0, 1).unwrap()
                };
                let (acc, hashes) = start.extend(Layout::array::<u32>(cap).unwrap()).unwrap();
                let (acc, keys) = acc.extend(Layout::array::<K>(cap).unwrap()).unwrap();
                let (acc, values) = acc.extend(Layout::array::<V>(cap).unwrap()).unwrap();
                let slots = index_slots(cap);
                let tag_bytes = if slots == 0 { 0 } else { slots + TAIL };
                let (acc, tags) = acc.extend(Layout::array::<u8>(tag_bytes).unwrap()).unwrap();
                let (acc, positions) = if slots == 0 {
                    (acc, None)
                } else {
                    let positions = Layout::from_size_align(
                        slots * position_size(wide_index(cap)),
                        POSITION_ALIGN,
                    )
                    .unwrap();
                    let (acc, positions) = acc.extend(positions).unwrap();
                    (acc, Some(positions))
                };
                let expected = acc.pad_to_align();

                let name = std::any::type_name::<(K, V)>();
                let o = offsets::<K, V>(cap, header);
                assert_eq!(expected, o.layout, "{name} cap={cap} header={header}");
                assert_eq!(
                    (hashes, keys, values),
                    array_offsets::<K, V>(cap, header),
                    "{name} cap={cap} header={header}"
                );
                let region = index_region::<K, V>(cap, header);
                assert_eq!(tags, region.tags, "{name} cap={cap} header={header}");
                assert_eq!(slots, region.slots, "{name} cap={cap} header={header}");
                assert_eq!(
                    wide_index(cap),
                    region.wide,
                    "{name} cap={cap} header={header}"
                );
                if let Some(positions) = positions {
                    assert_eq!(
                        positions, region.positions,
                        "{name} cap={cap} header={header}"
                    );
                }
            }
        }
    }

    #[test]
    fn offsets_match_layout_extend() {
        check_against_layout_extend::<u8, u8>();
        check_against_layout_extend::<u64, u64>();
        check_against_layout_extend::<u128, u8>();
        check_against_layout_extend::<[u8; 3], u16>();
        check_against_layout_extend::<(), ()>();
        check_against_layout_extend::<String, Vec<u8>>();
    }

    #[test]
    fn max_cap_fits_an_allocation() {
        let cap = max_cap::<u64, u64>();
        assert!(offsets::<u64, u64>(cap, true).layout.size() <= isize::MAX as usize);
        let cap = max_cap::<(), ()>();
        assert!(offsets::<(), ()>(cap, true).layout.size() <= isize::MAX as usize);
    }
}
