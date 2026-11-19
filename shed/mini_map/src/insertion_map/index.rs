/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The position index of an insertion map that has outgrown the hash scan.
//!
//! Our own table rather than hashbrown's because the map is one allocation placed by whoever owns
//! it, and hashbrown cannot build into memory it does not own.
//!
//! Two arrays over the same slots: a byte of tag per slot, and the position of the entry in that
//! slot. A tag is the low seven bits of the entry's hash, or [`EMPTY`]; the top bit tells the two
//! apart, so over a group of tags loaded at once one byte-wise comparison yields the candidates
//! for a hash and another the empty slots. Probing is linear by slot from a home taken from the
//! high bits of the hash by a multiply, so the slot count need not be a power of two, and moves a
//! group at a time; the tags array is followed by a copy of its first [`TAIL`] bytes so that a
//! group loaded near the end wraps around without a branch. Removal shifts later members of a
//! run back rather than leaving tombstones. The table is sized by
//! [`index_slots`](super::layout::index_slots), which keeps it at most two thirds full and so
//! guarantees every probe an empty slot to stop at. Positions of empty slots are kept at zero, so
//! that equal maps have equal bytes.

/// The tag of an empty slot. Hash tags have the top bit clear.
pub(crate) const EMPTY: u8 = 0x80;

/// Bytes of tags past the last slot, mirroring the first slots, so that a group loaded at any slot
/// is in bounds. Fixed rather than tied to `GROUP` so that the layout does not depend on the
/// toolchain.
pub(crate) const TAIL: usize = 16;

/// Tags examined per probe step.
#[cfg(rust_nightly)]
const GROUP: usize = 16;
#[cfg(not(rust_nightly))]
const GROUP: usize = 8;

const _: () = assert!(GROUP <= TAIL);

/// A position as the index stores it. Two widths: `u32` while every position fits in one, `u64`
/// beyond that.
pub(crate) trait Pos: Copy + Eq + 'static {
    /// The most entries a map can hold with positions of this width.
    const MAX_CAP: usize;

    fn from_usize(pos: usize) -> Self;

    fn to_usize(self) -> usize;

    /// The slot where probing for `hash` starts, in a table of `slots` slots.
    fn home(hash: u32, slots: usize) -> usize;
}

impl Pos for u32 {
    /// Chosen so that [`index_slots`](super::layout::index_slots) of it still fits in 32 bits,
    /// which is what lets `home` multiply in 64.
    const MAX_CAP: usize = 0xaaaa_aaaa;

    #[inline]
    fn from_usize(pos: usize) -> Self {
        debug_assert!(pos < Self::MAX_CAP);
        pos as u32
    }

    #[inline]
    fn to_usize(self) -> usize {
        self as usize
    }

    #[inline]
    fn home(hash: u32, slots: usize) -> usize {
        debug_assert!(slots <= u32::MAX as usize);
        ((hash as u64 * slots as u64) >> 32) as usize
    }
}

impl Pos for u64 {
    const MAX_CAP: usize = usize::MAX;

    #[inline]
    fn from_usize(pos: usize) -> Self {
        pos as u64
    }

    #[inline]
    fn to_usize(self) -> usize {
        self as usize
    }

    #[inline]
    fn home(hash: u32, slots: usize) -> usize {
        ((hash as u128 * slots as u128) >> 32) as usize
    }
}

/// The tag stored for a hash: its low bits, which `home` barely looks at.
#[inline]
fn tag(hash: u32) -> u8 {
    (hash & 0x7f) as u8
}

/// `i` taken back into the table if it ran past the end by less than a table.
#[inline]
fn wrap(i: usize, slots: usize) -> usize {
    if i >= slots { i - slots } else { i }
}

#[inline]
fn next(i: usize, slots: usize) -> usize {
    wrap(i + 1, slots)
}

/// Write a tag, and its copy in the tail if the slot has one there.
#[inline]
fn set_tag(tags: &mut [u8], slot: usize, tag: u8) {
    let slots = tags.len() - TAIL;
    tags[slot] = tag;
    if slot < TAIL {
        tags[slots + slot] = tag;
    }
}

#[cfg(rust_nightly)]
mod group {
    use std::simd::Simd;
    use std::simd::cmp::SimdPartialEq;
    use std::simd::cmp::SimdPartialOrd;

    pub(super) type Repr = Simd<u8, { super::GROUP }>;

    /// Lane `i` of a mask is bit `i * LANE_BITS`.
    pub(super) const LANE_BITS: u32 = 1;

    /// # Safety
    ///
    /// `GROUP` bytes must be readable at `ptr`.
    #[inline]
    pub(super) unsafe fn load(ptr: *const u8) -> Repr {
        unsafe { ptr.cast::<Repr>().read_unaligned() }
    }

    #[inline]
    pub(super) fn matches(group: Repr, tag: u8) -> u64 {
        group.simd_eq(Repr::splat(tag)).to_bitmask()
    }

    #[inline]
    pub(super) fn empties(group: Repr) -> u64 {
        group.simd_ge(Repr::splat(super::EMPTY)).to_bitmask()
    }
}

#[cfg(not(rust_nightly))]
mod group {
    pub(super) type Repr = u64;

    /// Lane `i` of a mask is bit `i * LANE_BITS`.
    pub(super) const LANE_BITS: u32 = 8;

    const LOW_BITS: u64 = 0x0101_0101_0101_0101;
    const HIGH_BITS: u64 = 0x8080_8080_8080_8080;

    /// # Safety
    ///
    /// `GROUP` bytes must be readable at `ptr`.
    #[inline]
    pub(super) unsafe fn load(ptr: *const u8) -> Repr {
        // Little-endian order puts the tag at the lowest address in the lowest byte, which is
        // what counting lanes from the bottom relies on.
        u64::from_le(unsafe { ptr.cast::<u64>().read_unaligned() })
    }

    /// Bytes equal to `tag`, as the top bit of each. Exact up to and including the first match;
    /// a borrow out of a matching byte can flag the byte above it too, which costs the probe a
    /// verification and nothing else.
    #[inline]
    pub(super) fn matches(group: Repr, tag: u8) -> u64 {
        let x = group ^ (LOW_BITS * tag as u64);
        x.wrapping_sub(LOW_BITS) & !x & HIGH_BITS
    }

    #[inline]
    pub(super) fn empties(group: Repr) -> u64 {
        group & HIGH_BITS
    }
}

/// Lanes of a group, picked out by bit.
#[derive(Clone, Copy)]
struct Lanes(u64);

impl Lanes {
    #[inline]
    fn any(self) -> bool {
        self.0 != 0
    }

    /// The lowest lane set; the mask must not be empty.
    #[inline]
    fn lowest(self) -> usize {
        (self.0.trailing_zeros() / group::LANE_BITS) as usize
    }

    #[inline]
    fn without_lowest(self) -> Lanes {
        Lanes(self.0 & (self.0 - 1))
    }

    /// Only the lanes below `lane`, which must be within the group.
    #[inline]
    fn below(self, lane: usize) -> Lanes {
        Lanes(self.0 & ((1u64 << (lane as u32 * group::LANE_BITS)) - 1))
    }
}

/// The group of tags starting at slot `i`.
///
/// # Safety
///
/// `i + GROUP` must not exceed `tags.len()`, which holds for every slot of a table with its tail.
#[inline]
unsafe fn load(tags: &[u8], i: usize) -> group::Repr {
    debug_assert!(i + GROUP <= tags.len());
    unsafe { group::load(tags.as_ptr().add(i)) }
}

/// Position of the entry with this hash for which `eq` holds, if any. `hashes` are the entries'
/// stored hashes, indexed by position; `tags` include the tail.
#[inline(always)]
pub(crate) fn find<P: Pos>(
    tags: &[u8],
    positions: &[P],
    hashes: &[u32],
    hash: u32,
    mut eq: impl FnMut(usize) -> bool,
) -> Option<usize> {
    let slots = positions.len();
    debug_assert_eq!(tags.len(), slots + TAIL);
    let tag = tag(hash);
    let mut i = P::home(hash, slots);
    loop {
        // SAFETY: `i < slots`, and at least `GROUP` tags follow the last slot.
        let group = unsafe { load(tags, i) };
        let empties = Lanes(group::empties(group));
        let mut candidates = Lanes(group::matches(group, tag));
        if empties.any() {
            candidates = candidates.below(empties.lowest());
        }
        while candidates.any() {
            let slot = wrap(i + candidates.lowest(), slots);
            // SAFETY: the slot comes before the first empty one of its group, so it is occupied,
            // and an occupied slot holds the position of a live entry.
            let pos = unsafe { positions.get_unchecked(slot) }.to_usize();
            debug_assert!(pos < hashes.len());
            if unsafe { *hashes.get_unchecked(pos) } == hash && eq(pos) {
                return Some(pos);
            }
            candidates = candidates.without_lowest();
        }
        if empties.any() {
            return None;
        }
        i = wrap(i + GROUP, slots);
    }
}

/// Record that the entry at `pos` has this hash. The table must have an empty slot.
#[inline]
pub(crate) fn insert<P: Pos>(tags: &mut [u8], positions: &mut [P], hash: u32, pos: usize) {
    let slots = positions.len();
    let mut i = P::home(hash, slots);
    loop {
        // SAFETY: as in `find`.
        let empties = Lanes(group::empties(unsafe { load(tags, i) }));
        if empties.any() {
            let slot = wrap(i + empties.lowest(), slots);
            set_tag(tags, slot, tag(hash));
            positions[slot] = P::from_usize(pos);
            return;
        }
        i = wrap(i + GROUP, slots);
    }
}

/// Forget the entry at `pos`, which has this hash and is still in `hashes`.
pub(crate) fn remove<P: Pos>(
    tags: &mut [u8],
    positions: &mut [P],
    hashes: &[u32],
    hash: u32,
    pos: usize,
) {
    let slots = positions.len();
    let mut hole = P::home(hash, slots);
    loop {
        debug_assert!(tags[hole] != EMPTY, "entry {pos} is not in the index");
        if tags[hole] == EMPTY || positions[hole].to_usize() == pos {
            break;
        }
        hole = next(hole, slots);
    }
    // Backward-shift deletion: an entry later in the same run can move into the hole unless its
    // own home lies strictly after the hole, in which case a probe from that home would never
    // have reached the hole and must not find it there.
    let mut j = hole;
    loop {
        j = next(j, slots);
        if tags[j] == EMPTY {
            break;
        }
        let home = P::home(hashes[positions[j].to_usize()], slots);
        let home_after_hole = if hole <= j {
            hole < home && home <= j
        } else {
            hole < home || home <= j
        };
        if !home_after_hole {
            set_tag(tags, hole, tags[j]);
            positions[hole] = positions[j];
            hole = j;
        }
    }
    set_tag(tags, hole, EMPTY);
    positions[hole] = P::from_usize(0);
}

/// Empty the table.
pub(crate) fn clear<P: Pos>(tags: &mut [u8], positions: &mut [P]) {
    tags.fill(EMPTY);
    positions.fill(P::from_usize(0));
}

/// Rebuild the whole table from the stored hashes.
pub(crate) fn rebuild<P: Pos>(tags: &mut [u8], positions: &mut [P], hashes: &[u32]) {
    clear(tags, positions);
    for (pos, &hash) in hashes.iter().enumerate() {
        insert(tags, positions, hash, pos);
    }
}

/// Apply `f` to every stored position, after the entries were reordered in a way that moves no
/// entry to a different hash.
pub(crate) fn map_positions<P: Pos>(tags: &[u8], positions: &mut [P], f: impl Fn(usize) -> usize) {
    for (slot, position) in positions.iter_mut().enumerate() {
        if tags[slot] != EMPTY {
            *position = P::from_usize(f(position.to_usize()));
        }
    }
}

/// The two arrays of an index, borrowed for writing.
pub(crate) struct Table<'a, P> {
    pub(crate) tags: &'a mut [u8],
    pub(crate) positions: &'a mut [P],
}

impl<P: Pos> Table<'_, P> {
    pub(crate) fn insert(&mut self, hash: u32, pos: usize) {
        insert(self.tags, self.positions, hash, pos)
    }

    pub(crate) fn remove(&mut self, hashes: &[u32], hash: u32, pos: usize) {
        remove(self.tags, self.positions, hashes, hash, pos)
    }

    pub(crate) fn clear(&mut self) {
        clear(self.tags, self.positions)
    }

    pub(crate) fn rebuild(&mut self, hashes: &[u32]) {
        rebuild(self.tags, self.positions, hashes)
    }

    pub(crate) fn map_positions(&mut self, f: impl Fn(usize) -> usize) {
        map_positions(self.tags, self.positions, f)
    }

    /// Everything the table stores, for tests that two tables are identical.
    #[cfg(test)]
    pub(crate) fn contents(&self) -> (Vec<u8>, Vec<usize>) {
        (
            self.tags.to_vec(),
            self.positions.iter().map(|p| p.to_usize()).collect(),
        )
    }

    /// Check that the table agrees with the stored hashes, slot by slot.
    #[cfg(test)]
    pub(crate) fn check(&self, hashes: &[u32]) {
        let slots = self.positions.len();
        let occupied = self.tags[..slots].iter().filter(|&&t| t != EMPTY).count();
        assert_eq!(
            hashes.len(),
            occupied,
            "index holds a different number of entries"
        );
        assert_eq!(
            self.tags[..TAIL],
            self.tags[slots..],
            "tail does not mirror the first slots"
        );
        for (slot, &tag) in self.tags[..slots].iter().enumerate() {
            if tag == EMPTY {
                assert_eq!(
                    0,
                    self.positions[slot].to_usize(),
                    "empty slot {slot} keeps a stale position"
                );
            }
        }
        for (pos, &hash) in hashes.iter().enumerate() {
            assert_eq!(
                Some(pos),
                find(self.tags, self.positions, hashes, hash, |p| p == pos),
                "entry {pos} with hash {hash:#x} is not reachable through the index"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::insertion_map::layout::index_slots;

    struct Owned<P> {
        tags: Vec<u8>,
        positions: Vec<P>,
    }

    impl<P: Pos> Owned<P> {
        fn for_capacity(cap: usize) -> Self {
            let slots = index_slots(cap);
            assert!(slots > 0);
            Owned {
                tags: vec![EMPTY; slots + TAIL],
                positions: vec![P::from_usize(0); slots],
            }
        }

        fn table(&mut self) -> Table<'_, P> {
            Table {
                tags: &mut self.tags,
                positions: &mut self.positions,
            }
        }

        fn find(&self, hashes: &[u32], hash: u32, eq: impl FnMut(usize) -> bool) -> Option<usize> {
            find(&self.tags, &self.positions, hashes, hash, eq)
        }

        fn check(&mut self, hashes: &[u32]) {
            self.table().check(hashes);
        }
    }

    fn lcg(seed: u64) -> impl FnMut() -> u32 {
        let mut state = seed | 1;
        move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 32) as u32
        }
    }

    fn exercise<P: Pos>() {
        // Smaller under Miri, where the group operations are interpreted lane by lane.
        let (entries, cap) = if cfg!(miri) { (60, 80) } else { (300, 400) };
        // Many colliding tags and homes: hashes drawn from a tiny range of bits.
        for (seed, mask) in [
            (1u64, u32::MAX),
            (2, 0xff00_00ff),
            (3, 0x0000_ffff),
            (4, 0x7),
            (5, 0xffff_0000),
        ] {
            let mut random = lcg(seed);
            let mut hashes: Vec<u32> = (0..entries).map(|_| random() & mask).collect();
            let mut t = Owned::<P>::for_capacity(cap);
            for (pos, &hash) in hashes.iter().enumerate() {
                t.table().insert(hash, pos);
            }
            t.check(&hashes);

            // Remove every third entry from the back, keeping positions dense, as the map does.
            let mut pos = hashes.len();
            while pos >= 3 {
                pos -= 3;
                t.table().remove(&hashes, hashes[pos], pos);
                hashes.remove(pos);
                t.table().map_positions(|p| if p > pos { p - 1 } else { p });
                t.check(&hashes);
            }

            // A miss must stop at an empty slot rather than loop.
            assert_eq!(None, t.find(&hashes, random(), |_| false));

            t.table().rebuild(&hashes);
            t.check(&hashes);
            t.table().clear();
            t.check(&[]);
        }
    }

    #[test]
    fn narrow_positions() {
        exercise::<u32>();
    }

    #[test]
    fn wide_positions() {
        exercise::<u64>();
    }

    /// A hash whose home is `home`, in a table of `slots` slots.
    fn hash_with_home(home: usize, slots: usize) -> u32 {
        let hash = ((home as u64) << 32).div_ceil(slots as u64) as u32;
        assert_eq!(home, <u32 as Pos>::home(hash, slots));
        hash
    }

    #[test]
    fn a_run_longer_than_a_group_is_searched_in_order() {
        // Every entry has the same hash, so the same home and tag, and the run of them is several
        // groups long. Each is found, whichever `eq` singles out.
        let mut t = Owned::<u32>::for_capacity(400);
        let hash = hash_with_home(100, t.positions.len());
        let hashes = vec![hash; 3 * TAIL];
        for (pos, &h) in hashes.iter().enumerate() {
            t.table().insert(h, pos);
        }
        t.check(&hashes);
        assert_eq!(None, t.find(&hashes, hash, |_| false));
        // Remove from the front of the run and the rest shift back into place.
        let mut hashes = hashes;
        while !hashes.is_empty() {
            t.table().remove(&hashes, hash, 0);
            hashes.remove(0);
            t.table().map_positions(|p| p - 1);
            t.check(&hashes);
        }
    }

    #[test]
    fn probing_wraps_around_the_end() {
        let mut t = Owned::<u32>::for_capacity(400);
        let slots = t.positions.len();
        // A run that starts in the last few slots and spills into the first ones, so the group
        // loaded at its home reads the tail, and the slots it names wrap.
        let hash = hash_with_home(slots - 3, slots);
        let mut hashes = vec![hash; 2 * TAIL];
        for (pos, &h) in hashes.iter().enumerate() {
            t.table().insert(h, pos);
        }
        t.check(&hashes);
        assert_eq!(None, t.find(&hashes, hash_with_home(0, slots), |_| false));
        // A different hash whose home is inside the run is still found past the wrap.
        let later = hash_with_home(2, slots);
        hashes.push(later);
        t.table().insert(later, hashes.len() - 1);
        t.check(&hashes);
        for pos in (0..hashes.len() - 1).rev() {
            t.table().remove(&hashes, hashes[pos], pos);
            hashes.remove(pos);
            t.table().map_positions(|p| if p > pos { p - 1 } else { p });
            t.check(&hashes);
        }
    }

    #[test]
    fn home_uses_the_high_bits_and_stays_in_range() {
        for slots in [25usize, 49, 1000, u32::MAX as usize] {
            assert_eq!(0, <u32 as Pos>::home(0, slots));
            assert!(<u32 as Pos>::home(u32::MAX, slots) < slots);
            assert_eq!(slots / 2, <u32 as Pos>::home(1 << 31, slots));
        }
        assert!(<u64 as Pos>::home(u32::MAX, 1 << 40) < 1 << 40);
        assert_eq!(1 << 39, <u64 as Pos>::home(1 << 31, 1 << 40));
    }

    #[test]
    fn narrow_positions_keep_the_slot_count_in_32_bits() {
        assert!(index_slots(<u32 as Pos>::MAX_CAP) <= u32::MAX as usize);
        assert!(index_slots(<u32 as Pos>::MAX_CAP + 1) > u32::MAX as usize);
    }

    #[test]
    fn group_masks_find_tags_and_empties() {
        let mut tags = vec![EMPTY; 4 * TAIL];
        tags[1] = 0x12;
        tags[3] = 0x12;
        tags[4] = 0x7f;
        let group = unsafe { load(&tags, 0) };
        let matches = Lanes(group::matches(group, 0x12));
        assert_eq!(1, matches.lowest());
        assert_eq!(3, matches.without_lowest().lowest());
        let empties = Lanes(group::empties(group));
        assert_eq!(0, empties.lowest());
        assert!(!matches.below(1).any());
        assert_eq!(1, matches.below(2).lowest());
        assert!(!Lanes(group::matches(group, 0x13)).below(4).any());
        // Everything below the first empty is exact; the matches among the tags that follow
        // it are what the probe ignores.
        let group = unsafe { load(&tags, 1) };
        let empties = Lanes(group::empties(group));
        assert_eq!(1, empties.lowest());
        assert_eq!(0, Lanes(group::matches(group, 0x12)).below(1).lowest());
    }
}
