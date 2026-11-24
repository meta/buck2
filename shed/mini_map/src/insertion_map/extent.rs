/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The first word of an insertion map's header, and the only code that knows how it is encoded.

/// Which slots of an allocation hold its entries, in one word.
///
/// Normally the word is the number of entries, which fill the slots from zero. While slots at
/// the front of a mutable map are vacated, it is the complement of the slot after the last
/// entry, and the number of vacated slots is in the allocation's `front`. The top bit tells the
/// two apart: every slot takes at least a `u32` hash and no allocation exceeds `isize::MAX`
/// bytes, so no slot number has it set.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct Extent(usize);

impl Extent {
    /// The largest slot number an allocation can have.
    const MAX_SLOT: usize = isize::MAX as usize / size_of::<u32>();

    /// `len` entries from slot zero.
    #[inline(always)]
    pub(crate) const fn new(len: usize) -> Self {
        debug_assert!(len <= Self::MAX_SLOT);
        Extent(len)
    }

    /// Entries ending before slot `end`, with slots vacated before the first of them.
    #[inline(always)]
    fn vacated(end: usize) -> Self {
        debug_assert!(end <= Self::MAX_SLOT);
        Extent(!end)
    }

    #[inline(always)]
    pub(crate) fn has_vacated(self) -> bool {
        (self.0 as isize) < 0
    }

    #[inline(always)]
    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The number of entries, of an allocation with no vacated slots.
    #[inline(always)]
    pub(crate) fn len(self) -> usize {
        debug_assert!(!self.has_vacated());
        self.0
    }

    /// Entries in slots `front..end`. `store_front` writes the allocation's `front`, and is
    /// called only when it has to hold a value.
    #[inline(always)]
    pub(crate) fn from_bounds(front: usize, end: usize, store_front: impl FnOnce(usize)) -> Self {
        if front == 0 {
            Extent::new(end)
        } else {
            store_front(front);
            Extent::vacated(end)
        }
    }

    /// `then` of the number of vacated slots before the first entry and the number of entries.
    /// `stored_front` reads the allocation's `front`, and is called only while it holds a value.
    #[inline(always)]
    pub(crate) fn front_and_len<R>(
        self,
        stored_front: impl FnOnce() -> usize,
        then: impl FnOnce(usize, usize) -> R,
    ) -> R {
        if self.has_vacated() {
            let front = stored_front();
            then(front, !self.0 - front)
        } else {
            then(0, self.0)
        }
    }

    /// The slot after the last entry.
    #[inline(always)]
    pub(crate) fn end(self) -> usize {
        self.0 ^ self.complemented()
    }

    /// The same, with the entries ending before slot `end` instead.
    #[inline(always)]
    pub(crate) fn with_end(self, end: usize) -> Self {
        debug_assert!(end <= Self::MAX_SLOT);
        Extent(end ^ self.complemented())
    }

    /// The slot after the last entry, and the same with one more entry there.
    #[inline(always)]
    pub(crate) fn extend(self) -> (usize, Self) {
        // A branch, not arithmetic on the word: consecutive insertions wait on this word, and
        // the usual case then costs them one addition.
        if self.has_vacated() {
            self.extend_vacated()
        } else {
            (self.0, Extent::new(self.0 + 1))
        }
    }

    #[cold]
    #[inline(never)]
    fn extend_vacated(self) -> (usize, Self) {
        let end = self.end();
        (end, self.with_end(end + 1))
    }

    /// All ones if the word is a complement, zero if not.
    #[inline(always)]
    fn complemented(self) -> usize {
        ((self.0 as isize) >> (usize::BITS - 1)) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENDS: [usize; 7] = [0, 1, 2, 33, 1 << 31, 1 << 32, Extent::MAX_SLOT];

    #[test]
    fn plain_is_the_length() {
        for len in ENDS {
            let extent = Extent::new(len);
            assert!(!extent.has_vacated());
            assert_eq!(len, extent.len());
            assert_eq!(len, extent.end());
            assert_eq!(len == 0, extent.is_empty());
        }
    }

    #[test]
    fn vacated_keeps_the_end() {
        for end in ENDS {
            let extent = Extent::vacated(end);
            assert!(extent.has_vacated());
            assert_eq!(end, extent.end());
            assert!(!extent.is_empty());
        }
    }

    #[test]
    fn front_is_stored_only_while_slots_are_vacated() {
        for end in ENDS {
            let plain = Extent::from_bounds(0, end, |_| panic!("no vacated slots"));
            assert!(!plain.has_vacated());
            let bounds =
                plain.front_and_len(|| panic!("no vacated slots"), |front, len| (front, len));
            assert_eq!((0, end), bounds);
            for front in [1, end / 2, end] {
                if (1..=end).contains(&front) {
                    let mut stored = None;
                    let vacated = Extent::from_bounds(front, end, |front| stored = Some(front));
                    assert!(vacated.has_vacated());
                    assert_eq!(Some(front), stored);
                    assert_eq!(end, vacated.end());
                    let bounds = vacated.front_and_len(|| front, |front, len| (front, len));
                    assert_eq!((front, end - front), bounds);
                }
            }
        }
    }

    #[test]
    fn with_end_keeps_which_kind_it_is() {
        for from in ENDS {
            for to in ENDS {
                let plain = Extent::new(from).with_end(to);
                assert!(!plain.has_vacated());
                assert_eq!(to, plain.len());
                let vacated = Extent::vacated(from).with_end(to);
                assert!(vacated.has_vacated());
                assert_eq!(to, vacated.end());
            }
        }
    }

    #[test]
    fn extend_adds_one_entry_at_the_end() {
        for end in ENDS {
            if end < Extent::MAX_SLOT {
                let (slot, plain) = Extent::new(end).extend();
                assert_eq!(end, slot);
                assert!(!plain.has_vacated());
                assert_eq!(end + 1, plain.len());
                let (slot, vacated) = Extent::vacated(end).extend();
                assert_eq!(end, slot);
                assert!(vacated.has_vacated());
                assert_eq!(end + 1, vacated.end());
            }
        }
    }

    #[test]
    fn a_header_word_of_zeros_is_an_empty_map() {
        // The static header an unallocated map points at relies on this.
        assert!(Extent::new(0).is_empty());
        assert_eq!(0, Extent::new(0).end());
    }
}
