/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A map from [`Key`] stored at each key's index.

use std::iter;

use allocative::Allocative;

use crate::ids::Key;

/// `log2` of a page's length. Miri interprets each page's fill, so pages are smaller under it.
const PAGE_BITS: u32 = if cfg!(miri) { 6 } else { 12 };
const PAGE_LEN: usize = 1 << PAGE_BITS;

type Page<V> = Box<[Option<V>; PAGE_LEN]>;

/// A map from [`Key`] to `V`, stored at each key's index.
///
/// For maps that come to hold most keys: key indices are dense, so a slot per index costs about
/// what a hash map's spare capacity does, and a lookup needs no hashing or probing. Storage is
/// fixed-length pages allocated on first use, so growing never moves an entry and a sparse map
/// pays only for the pages it touches; a lookup is one load from the page table, which stays in
/// cache, and one from the page.
#[derive(Allocative)]
pub struct KeyVec<V> {
    /// Page `p` holds indices `[p * PAGE_LEN, (p + 1) * PAGE_LEN)`.
    pages: Vec<Option<Page<V>>>,
    len: usize,
}

impl<V> Default for KeyVec<V> {
    fn default() -> Self {
        Self {
            pages: Vec::new(),
            len: 0,
        }
    }
}

#[inline]
fn locate(key: Key) -> (usize, usize) {
    let index = key.index as usize;
    (index >> PAGE_BITS, index & (PAGE_LEN - 1))
}

fn new_page<V>() -> Page<V> {
    iter::repeat_with(|| None)
        .take(PAGE_LEN)
        .collect::<Box<[_]>>()
        .try_into()
        .ok()
        .expect("should have collected exactly one page")
}

impl<V> KeyVec<V> {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn get(&self, key: &Key) -> Option<&V> {
        let (page, offset) = locate(*key);
        self.pages.get(page)?.as_ref()?[offset].as_ref()
    }

    #[inline]
    pub fn get_mut(&mut self, key: &Key) -> Option<&mut V> {
        self.get_slot_mut(*key)?.as_mut()
    }

    pub fn contains_key(&self, key: &Key) -> bool {
        self.get(key).is_some()
    }

    /// The value at `key`, inserting `f()` first if there is none.
    #[inline]
    pub fn get_or_insert_with(&mut self, key: Key, f: impl FnOnce() -> V) -> &mut V {
        let (page, offset) = locate(key);
        if page >= self.pages.len() {
            self.pages.resize_with(page + 1, || None);
        }
        let slot = &mut self.pages[page].get_or_insert_with(new_page)[offset];
        if slot.is_none() {
            self.len += 1;
        }
        slot.get_or_insert_with(f)
    }

    pub fn remove(&mut self, key: &Key) -> Option<V> {
        let removed = self.get_slot_mut(*key)?.take();
        if removed.is_some() {
            self.len -= 1;
        }
        removed
    }

    #[inline]
    fn get_slot_mut(&mut self, key: Key) -> Option<&mut Option<V>> {
        let (page, offset) = locate(key);
        Some(&mut self.pages.get_mut(page)?.as_mut()?[offset])
    }

    /// Entries in key order.
    pub fn iter(&self) -> impl Iterator<Item = (Key, &V)> {
        self.pages
            .iter()
            .enumerate()
            .filter_map(|(p, page)| Some((p, page.as_ref()?)))
            .flat_map(|(p, page)| {
                page.iter()
                    .enumerate()
                    .filter_map(move |(offset, v)| Some((key_at(p, offset), v.as_ref()?)))
            })
    }

    /// Entries in key order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Key, &mut V)> {
        self.pages
            .iter_mut()
            .enumerate()
            .filter_map(|(p, page)| Some((p, page.as_mut()?)))
            .flat_map(|(p, page)| {
                page.iter_mut()
                    .enumerate()
                    .filter_map(move |(offset, v)| Some((key_at(p, offset), v.as_mut()?)))
            })
    }

    pub fn keys(&self) -> impl Iterator<Item = Key> {
        self.iter().map(|(key, _)| key)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }
}

fn key_at(page: usize, offset: usize) -> Key {
    Key {
        index: ((page << PAGE_BITS) | offset) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(index: u32) -> Key {
        Key { index }
    }

    #[test]
    fn locate_covers_page_boundaries() {
        let p = PAGE_LEN as u32;
        assert_eq!(locate(k(0)), (0, 0));
        assert_eq!(locate(k(p - 1)), (0, PAGE_LEN - 1));
        assert_eq!(locate(k(p)), (1, 0), "page 1 starts at PAGE_LEN");
        assert_eq!(
            locate(k(u32::MAX)),
            (u32::MAX as usize >> PAGE_BITS, PAGE_LEN - 1)
        );
        for index in [0, p - 1, p, 1 << 20, u32::MAX] {
            let (page, offset) = locate(k(index));
            assert_eq!(key_at(page, offset), k(index));
        }
    }

    #[test]
    fn insert_get_remove_and_iterate_in_key_order() {
        let p = PAGE_LEN as u32;
        let mut map = KeyVec::default();
        let keys = [p + 1, 3, p, 2 * p, 0];
        for key in keys {
            *map.get_or_insert_with(k(key), || 0) += key;
        }
        *map.get_or_insert_with(k(3), || 100) += 1;
        assert_eq!(
            map.len(),
            keys.len(),
            "a second insert at a key adds nothing"
        );
        assert_eq!(
            map.get(&k(3)),
            Some(&4),
            "the existing value is kept and updated"
        );
        assert_eq!(map.get(&k(4)), None);
        assert!(
            !map.contains_key(&k(6_000_000)),
            "a page past the table holds nothing"
        );

        assert_eq!(
            map.keys().map(|key| key.index).collect::<Vec<_>>(),
            vec![0, 3, p, p + 1, 2 * p]
        );

        assert_eq!(map.remove(&k(p)), Some(p));
        assert_eq!(map.remove(&k(p)), None);
        assert_eq!(map.remove(&k(9_000_000)), None);
        assert_eq!(map.len(), keys.len() - 1);

        for (_, v) in map.iter_mut() {
            *v += 1;
        }
        assert_eq!(
            map.values().copied().collect::<Vec<_>>(),
            vec![1, 5, p + 2, 2 * p + 1]
        );
    }

    #[test]
    fn sparse_map_allocates_only_touched_pages() {
        let mut map = KeyVec::default();
        map.get_or_insert_with(k(3 * PAGE_LEN as u32 + 7), || ());
        assert_eq!(
            map.pages.iter().filter(|p| p.is_some()).count(),
            1,
            "pages below the touched one stay unallocated"
        );
        assert!(!map.contains_key(&k(7)));
    }
}
