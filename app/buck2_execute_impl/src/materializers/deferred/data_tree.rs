/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Data structure akin to a map, but where the key is a sequence.
//!
//! Restrictions:
//! - Storing both a key A and a key B which is a prefix of A is not possible.
//!
//! Special operations:
//! - Using key A to search for a value at a key B when B is a prefix of A.
//!
//! This is useful when artifacts are directories and we need to query the map
//! to figure out which artifact a path belongs to. E.g. we have an artifact at
//! "foo/bar", and we need to find out which artifact "foo/bar/c" belongs to.

use std::borrow::Borrow;
use std::collections::hash_map::IntoIter;
use std::collections::hash_map::Iter;
use std::hash::Hash;
use std::marker::PhantomData;
use std::mem;

use allocative::Allocative;
use allocative::Key;
use allocative::Visitor;
use allocative::hashbrown_util::bucket_count_for_capacity;
use buck2_error::buck2_error;
use buck2_hash::StdBuckHashMap;

/// Tree that stores data in the leaves. Think of the key as the path to the
/// leaf containing the value. The data/value is of type `V`, and each edge
/// is of type `K` (making the key to a value a sequence of `K`).
/// TODO(scottcao): This trie is not implemented properly. It should be merged into the directory trie
/// we have at buck2_core/src/directory.
#[derive(Debug)]
pub enum DataTree<K, V> {
    /// Stores data of type `V` with key of type `Iterator<Item = K>`.
    Tree(DataTreeChildren<K, DataTree<K, V>>),
    Data(V),
}

/// Child storage that allocates a hash table only when a path branches.
#[derive(Debug)]
pub enum DataTreeChildren<K, V> {
    Empty,
    One(Box<(K, V)>),
    Many(Box<StdBuckHashMap<K, V>>),
}

impl<K, V> DataTreeChildren<K, V> {
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q> + Hash + Eq,
        Q: Hash + Eq + ?Sized,
    {
        match self {
            Self::Empty => None,
            Self::One(entry) => (entry.0.borrow() == key).then_some(&entry.1),
            Self::Many(entries) => entries.get(key),
        }
    }

    fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q> + Hash + Eq,
        Q: Hash + Eq + ?Sized,
    {
        match self {
            Self::Empty => None,
            Self::One(entry) => (entry.0.borrow() == key).then_some(&mut entry.1),
            Self::Many(entries) => entries.get_mut(key),
        }
    }

    fn get_or_insert_with(&mut self, key: K, value: impl FnOnce() -> V) -> &mut V
    where
        K: Hash + Eq,
    {
        if matches!(self, Self::One(entry) if entry.0 != key) {
            let Self::One(entry) = mem::replace(self, Self::Empty) else {
                unreachable!();
            };
            *self = Self::Many(Box::new(StdBuckHashMap::from([*entry])));
        }

        if matches!(self, Self::Empty) {
            *self = Self::One(Box::new((key, value())));
            let Self::One(entry) = self else {
                unreachable!();
            };
            return &mut entry.1;
        }

        match self {
            Self::Empty => unreachable!(),
            Self::One(entry) => &mut entry.1,
            Self::Many(entries) => entries.entry(key).or_insert_with(value),
        }
    }

    fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q> + Hash + Eq,
        Q: Hash + Eq + ?Sized,
    {
        match self {
            Self::Empty => None,
            Self::One(entry) if entry.0.borrow() != key => None,
            Self::One(_) => {
                let Self::One(entry) = mem::replace(self, Self::Empty) else {
                    unreachable!();
                };
                Some(entry.1)
            }
            Self::Many(entries) => {
                let removed = entries.remove(key);
                if entries.len() == 1 {
                    let Self::Many(entries) = mem::replace(self, Self::Empty) else {
                        unreachable!();
                    };
                    *self = Self::One(Box::new(
                        (*entries)
                            .into_iter()
                            .next()
                            .expect("len == 1 branch guarantees a single entry"),
                    ));
                }
                removed
            }
        }
    }

    fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    fn iter(&self) -> DataTreeChildrenIter<'_, K, V> {
        match self {
            Self::Empty => DataTreeChildrenIter::One(None.into_iter()),
            Self::One(entry) => DataTreeChildrenIter::One(Some((&entry.0, &entry.1)).into_iter()),
            Self::Many(entries) => DataTreeChildrenIter::Many(entries.iter()),
        }
    }

    fn into_iter(self) -> DataTreeChildrenIntoIter<K, V> {
        match self {
            Self::Empty => DataTreeChildrenIntoIter::One(None.into_iter()),
            Self::One(entry) => DataTreeChildrenIntoIter::One(Some(*entry).into_iter()),
            Self::Many(entries) => DataTreeChildrenIntoIter::Many((*entries).into_iter()),
        }
    }
}

pub enum DataTreeChildrenIter<'a, K, V> {
    One(std::option::IntoIter<(&'a K, &'a V)>),
    Many(Iter<'a, K, V>),
}

impl<'a, K, V> Iterator for DataTreeChildrenIter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::One(iter) => iter.next(),
            Self::Many(iter) => iter.next(),
        }
    }
}

pub enum DataTreeChildrenIntoIter<K, V> {
    One(std::option::IntoIter<(K, V)>),
    Many(IntoIter<K, V>),
}

impl<K, V> Iterator for DataTreeChildrenIntoIter<K, V> {
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::One(iter) => iter.next(),
            Self::Many(iter) => iter.next(),
        }
    }
}

/// Visits a whole `DataTree` without making the flamegraph stack follow the
/// tree's path depth.
pub(crate) struct DataTreeAllocativeDfs<'a, K, V> {
    tree: &'a DataTree<K, V>,
}

impl<K: Allocative, V: Allocative> Allocative for DataTree<K, V> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        match self {
            Self::Tree(children) => {
                visitor.visit_field_with(Key::new("Tree"), mem::size_of_val(children), |visitor| {
                    match children {
                        DataTreeChildren::Empty => {}
                        DataTreeChildren::One(entry) => {
                            let mut visitor =
                                visitor.enter_unique(Key::new("One"), mem::size_of::<*const ()>());
                            visitor.visit_field_with(
                                Key::new("entry"),
                                mem::size_of_val(&**entry) - mem::size_of_val(&entry.1),
                                |visitor| visitor.visit_field(Key::new("key"), &entry.0),
                            );
                            visitor.exit();
                        }
                        DataTreeChildren::Many(entries) => {
                            let mut visitor =
                                visitor.enter_unique(Key::new("Many"), mem::size_of::<*const ()>());
                            visitor.visit_field_with(
                                Key::new("map"),
                                mem::size_of_val(&**entries),
                                |visitor| {
                                    visit_hash_map_keys_and_skipped_values(visitor, entries);
                                },
                            );
                            visitor.exit();
                        }
                    }
                });
            }
            Self::Data(data) => visitor.visit_field(Key::new("Data"), data),
        }
        visitor.exit();
    }
}

/// Account for hashmap overhead while skipping values.
///
/// The values (`DataTree` children) are not visited here — they are walked
/// separately by `DataTreeAllocativeDfs`. This means the standard `HashMap`
/// `Allocative` impl can't be used because it visits both keys and values.
///
/// Instead we report:
///   - each key individually (via `visit_field`)
///   - the key-portion of occupied slots (`occupied_key_slot_bytes`)
///   - empty bucket slots (`unused_bucket_bytes`)
///   - one control byte per bucket (`control_bytes`)
///
/// The total (`occupied_key_slot_bytes + unused_bucket_bytes + control_bytes`)
/// equals `raw_table_alloc_size_for_capacity::<(K, V)>() - len * size_of::<V>()`
/// — the full hashmap allocation minus the value slots that the DFS accounts for.
fn visit_hash_map_keys_and_skipped_values<K: Allocative, V: Allocative>(
    visitor: &mut Visitor<'_>,
    map: &StdBuckHashMap<K, DataTree<K, V>>,
) {
    let bucket_count = bucket_count_for_capacity(map.capacity());
    let occupied_key_slot_bytes = map.len()
        * mem::size_of::<(K, DataTree<K, V>)>().saturating_sub(mem::size_of::<DataTree<K, V>>());
    let unused_bucket_bytes =
        bucket_count.saturating_sub(map.len()) * mem::size_of::<(K, DataTree<K, V>)>();
    let control_bytes = bucket_count;

    let mut visitor = visitor.enter_unique(Key::new("data"), mem::size_of::<*const ()>());
    visitor.visit_field_with(
        Key::new("capacity"),
        occupied_key_slot_bytes + unused_bucket_bytes + control_bytes,
        |visitor| {
            for key in map.keys() {
                visitor.visit_field(Key::new("key"), key);
            }
            visitor.visit_simple(Key::new("unused_capacity"), unused_bucket_bytes);
            visitor.visit_simple(Key::new("control_bytes"), control_bytes);
        },
    );
    visitor.exit();
}

impl<K: Allocative, V: Allocative> Allocative for DataTreeAllocativeDfs<'_, K, V> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        let mut visitor = visitor.enter_unique(Key::new("nodes"), 0);
        let mut stack = vec![self.tree];
        while let Some(tree) = stack.pop() {
            tree.visit(&mut visitor);
            if let DataTree::Tree(children) = tree {
                stack.extend(children.iter().map(|(_, child)| child));
            }
        }
        visitor.exit();
    }
}

impl<K, V> DataTree<K, V> {
    pub(crate) fn allocative_dfs(&self) -> DataTreeAllocativeDfs<'_, K, V> {
        DataTreeAllocativeDfs { tree: self }
    }
}

impl<K: 'static + Eq + Hash + Clone, V: 'static> DataTree<K, V> {
    pub fn new() -> Self {
        Self::Tree(DataTreeChildren::Empty)
    }

    /// Gets the value at `key` or one of its prefixes, and returns it.
    ///
    /// When a value is found and [`Some`] is returned, it's guaranteed that
    /// only enough of `key` to find the returned value was consumed.
    /// E.g. if `key` is (A, B, C, D) and there's a value present at (A, B),
    /// then after this method returns (C, D) can still be consumed from `key`.
    ///
    /// There are no guarantees on how much is consumed from `key` when
    /// [`None`] is returned.
    pub fn prefix_get<'a, I, Q>(&self, key: &mut I) -> Option<&V>
    where
        K: 'a + Borrow<Q>,
        Q: 'a + Hash + Eq + ?Sized,
        I: Iterator<Item = &'a Q>,
    {
        if let Self::Data(data) = self {
            // return early so we don't consume from key_iter unnecessarily
            return Some(data);
        }
        let mut node = self;
        for k in key {
            node = match node.children().unwrap().get(k) {
                None => return None,
                Some(node) => match node {
                    Self::Tree(_) => node,
                    Self::Data(data) => return Some(data),
                },
            };
        }
        None
    }

    /// Similar to `prefix_get`, but takes and returns `&mut`.
    pub fn prefix_get_mut<'a, I, Q>(&mut self, key: &mut I) -> Option<&mut V>
    where
        K: 'a + Borrow<Q>,
        Q: 'a + Hash + Eq + ?Sized,
        I: Iterator<Item = &'a Q>,
    {
        if let Self::Data(data) = self {
            // return early so we don't consume from key_iter unnecessarily
            return Some(data);
        }
        let mut node = self;
        for k in key {
            node = match node.children_mut().unwrap().get_mut(k) {
                None => return None,
                Some(node) => match node {
                    Self::Tree(_) => node,
                    Self::Data(data) => return Some(data),
                },
            };
        }
        None
    }

    /// Get the subtree at `key`, if the entry exists and is a tree.
    /// Return an error if there is an entry but it is not a tree.
    pub fn get_subtree<'a, I, Q>(
        &self,
        key: &mut I,
    ) -> buck2_error::Result<Option<&DataTreeChildren<K, Self>>>
    where
        K: 'a + Borrow<Q>,
        Q: 'a + Hash + Eq + ?Sized,
        I: Iterator<Item = &'a Q>,
    {
        let mut entries = match self {
            Self::Tree(t) => t,
            Self::Data(..) => {
                return Err(buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "Data found where tree expected"
                ));
            }
        };

        for k in key {
            let node = match entries.get(k) {
                None => return Ok(None),
                Some(v) => v,
            };

            entries = match node {
                Self::Tree(t) => t,
                Self::Data(..) => {
                    return Err(buck2_error!(
                        buck2_error::ErrorTag::Tier0,
                        "Data found where tree expected"
                    ));
                }
            };
        }

        Ok(Some(entries))
    }

    /// Inserts a key-value pair into the tree.
    ///
    /// If there is already a key in the map that is a prefix of the inserted
    /// key, that key is removed.
    ///
    /// If the inserted key is a prefix of one or more keys in the map, all
    /// those keys are removed.
    pub fn insert<I: Iterator<Item = K>>(&mut self, mut key: I, value: V) {
        if let Some(k) = key.next() {
            if matches!(self, Self::Data(_)) {
                *self = Self::new();
            }
            let child = self
                .children_mut()
                .unwrap()
                .get_or_insert_with(k, Self::new);
            child.insert(key, value);
        } else {
            *self = Self::Data(value);
        }
    }

    /// Removes a key from the tree, returning the subtree at the key if the key
    /// was previously in the tree.
    /// If the prefix of `key` exists as a leaf on the tree, that leaf is removed
    /// and the value of that leaf is returned. We guarantee that only enough of
    /// `key` to find the returned value was consumed.
    pub fn remove<'a, I, Q>(&mut self, mut key: I) -> Option<DataTree<K, V>>
    where
        K: 'a + Borrow<Q>,
        Q: 'a + Hash + Eq + ?Sized,
        I: Iterator<Item = &'a Q>,
    {
        if matches!(self, Self::Data(_)) {
            return Some(std::mem::replace(self, Self::new()));
        }
        if let Some(k) = key.next() {
            if let Some(node) = self.children_mut().unwrap().get_mut(k) {
                let data = node.remove(key);
                let remove_node = match node {
                    Self::Tree(children) => children.is_empty(),
                    Self::Data(_) => true,
                };
                if remove_node {
                    self.children_mut().unwrap().remove(k);
                }
                data
            } else {
                None
            }
        } else {
            Some(std::mem::replace(self, Self::new()))
        }
    }

    pub fn children(&self) -> Option<&DataTreeChildren<K, Self>> {
        match self {
            Self::Tree(children) => Some(children),
            Self::Data(_) => None,
        }
    }

    fn children_mut(&mut self) -> Option<&mut DataTreeChildren<K, Self>> {
        match self {
            Self::Tree(children) => Some(children),
            Self::Data(_) => None,
        }
    }

    /// Returns an iterator over DataTree<K, V>.
    pub fn iter<T>(&self) -> DataTreeIterator<'_, K, V, T> {
        match self {
            Self::Tree(t) => DataTreeIterator::Stack(vec![(None, t.iter())], PhantomData),
            Self::Data(v) => DataTreeIterator::Entry(Some(v)),
        }
    }

    /// Take ownership of the values in DataTree<K, V> and iterate.
    pub fn into_iter<T>(self) -> DataTreeIntoIterator<K, V, T> {
        match self {
            Self::Tree(t) => DataTreeIntoIterator::Stack(vec![(None, t.into_iter())], PhantomData),
            Self::Data(v) => DataTreeIntoIterator::Entry(Some(v)),
        }
    }
}

pub enum DataTreeIterator<'a, K, V, T> {
    Stack(
        Vec<(Option<&'a K>, DataTreeChildrenIter<'a, K, DataTree<K, V>>)>,
        PhantomData<T>,
    ),
    Entry(Option<&'a V>),
}

impl<'a, K, V, T> Iterator for DataTreeIterator<'a, K, V, T>
where
    T: for<'x> FromIterator<&'x K>,
{
    type Item = (T, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Stack(stack, _) => loop {
                let (_, last) = stack.last_mut()?;

                match last.next() {
                    Some((k, DataTree::Tree(t))) => {
                        stack.push((Some(k), t.iter()));
                    }
                    Some((k, DataTree::Data(v))) => {
                        let it = stack
                            .iter()
                            .filter_map(|(k, _)| k.as_deref())
                            .chain(std::iter::once(k));
                        return Some((it.collect(), v));
                    }
                    None => {
                        stack.pop();
                    }
                }
            },
            Self::Entry(v) => v.take().map(|v| (std::iter::empty().collect(), v)),
        }
    }
}

pub enum DataTreeIntoIterator<K, V, T> {
    Stack(
        Vec<(Option<K>, DataTreeChildrenIntoIter<K, DataTree<K, V>>)>,
        PhantomData<T>,
    ),
    Entry(Option<V>),
}

impl<K, V, T> Iterator for DataTreeIntoIterator<K, V, T>
where
    T: for<'x> FromIterator<&'x K>,
{
    type Item = (T, V);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Stack(stack, _) => loop {
                let (_, last) = stack.last_mut()?;

                match last.next() {
                    Some((k, DataTree::Tree(t))) => {
                        stack.push((Some(k), t.into_iter()));
                    }
                    Some((k, DataTree::Data(v))) => {
                        let it = stack
                            .iter()
                            .filter_map(|(k, _)| k.as_ref())
                            .chain(std::iter::once(&k));
                        return Some((it.collect(), v));
                    }
                    None => {
                        stack.pop();
                    }
                }
            },
            Self::Entry(v) => v.take().map(|v| (std::iter::empty().collect(), v)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use assert_matches::assert_matches;
    use buck2_fs::paths::file_name::FileNameBuf;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;

    use super::*;

    #[derive(Debug)]
    struct CopyCollector<T>(Vec<T>);

    impl<'a, T> FromIterator<&'a T> for CopyCollector<T>
    where
        T: Copy + 'static,
    {
        fn from_iter<I>(iter: I) -> Self
        where
            I: IntoIterator<Item = &'a T>,
        {
            Self(iter.into_iter().copied().collect())
        }
    }

    #[test]
    fn test_child_storage_transitions() {
        let mut tree = DataTree::<i32, i32>::new();
        assert_matches!(tree.children(), Some(DataTreeChildren::Empty));
        tree.insert([1].into_iter(), 10);
        assert_matches!(tree.children(), Some(DataTreeChildren::One(_)));
        tree.insert([1].into_iter(), 11);
        assert_eq!(tree.prefix_get(&mut [1].iter()), Some(&11));
        assert_matches!(tree.children(), Some(DataTreeChildren::One(_)));
        assert!(tree.remove([2].iter()).is_none());
        tree.insert([2].into_iter(), 20);
        tree.insert([3].into_iter(), 30);
        assert_matches!(tree.children(), Some(DataTreeChildren::Many(_)));
        assert_matches!(tree.remove([2].iter()), Some(DataTree::Data(20)));
        assert_matches!(tree.children(), Some(DataTreeChildren::Many(_)));
        assert!(tree.remove([4].iter()).is_none());
        assert_matches!(tree.remove([3].iter()), Some(DataTree::Data(30)));
        assert_matches!(tree.children(), Some(DataTreeChildren::One(_)));
        assert_eq!(tree.prefix_get(&mut [1].iter()), Some(&11));
        assert_matches!(tree.remove([1].iter()), Some(DataTree::Data(11)));
        assert_matches!(tree.children(), Some(DataTreeChildren::Empty));
    }

    #[test]
    fn test_prefix_lookup_preserves_suffix() {
        let mut tree = DataTree::<i32, i32>::new();
        tree.insert([1, 2].into_iter(), 12);
        tree.insert([1, 3].into_iter(), 13);
        let path = [1, 2, 4, 5];
        let mut key = path.iter();
        assert_eq!(tree.prefix_get(&mut key), Some(&12));
        assert_eq!(key.copied().collect::<Vec<_>>(), [4, 5]);
        let mut key = path.iter();
        *tree.prefix_get_mut(&mut key).unwrap() = 120;
        assert_eq!(key.copied().collect::<Vec<_>>(), [4, 5]);
        assert_eq!(tree.prefix_get(&mut path.iter()), Some(&120));
    }

    #[test]
    fn test_overwrite_ancestor_and_descendants() {
        let mut tree = DataTree::<i32, i32>::new();
        tree.insert([1, 2].into_iter(), 12);
        tree.insert([1, 3].into_iter(), 13);
        tree.insert([1].into_iter(), 1);
        assert_eq!(tree.iter::<CopyCollector<_>>().count(), 1);
        assert_eq!(tree.prefix_get(&mut [1, 2].iter()), Some(&1));
        tree.insert([1, 4].into_iter(), 14);
        assert!(tree.prefix_get(&mut [1].iter()).is_none());
        assert!(tree.prefix_get(&mut [1, 2].iter()).is_none());
        assert_eq!(tree.prefix_get(&mut [1, 4].iter()), Some(&14));
        assert_eq!(tree.into_iter::<CopyCollector<_>>().count(), 1);
    }

    #[test]
    fn test_subtree_and_borrowed_keys() {
        let mut tree = DataTree::<String, i32>::new();
        tree.insert(["foo", "bar"].map(str::to_owned).into_iter(), 1);
        tree.insert(["foo", "baz"].map(str::to_owned).into_iter(), 2);
        let subtree = tree.get_subtree(&mut ["foo"].into_iter()).unwrap().unwrap();
        assert_eq!(subtree.iter().count(), 2);
        assert!(subtree.get("bar").is_some());
        assert!(
            tree.get_subtree(&mut ["missing"].into_iter())
                .unwrap()
                .is_none()
        );
        assert!(tree.get_subtree(&mut ["foo", "bar"].into_iter()).is_err());
        tree.remove(["foo", "bar"].into_iter());
        assert_eq!(tree.prefix_get(&mut ["foo", "baz"].into_iter()), Some(&2));
    }

    #[test]
    fn test_root_value() {
        let mut tree = DataTree::<i32, i32>::new();
        tree.insert([1, 2].into_iter(), 12);
        tree.insert(std::iter::empty(), 0);
        let mut key = [1, 2].iter();
        assert_eq!(tree.prefix_get(&mut key), Some(&0));
        assert_eq!(key.next(), Some(&1));
        assert!(tree.get_subtree(&mut std::iter::empty::<&i32>()).is_err());
        let entries = tree.iter::<CopyCollector<_>>().collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].0.0.is_empty());
        let mut key = [1, 2].iter();
        let removed = tree.remove(&mut key).unwrap();
        assert_eq!(key.next(), Some(&1));
        let entries = removed.into_iter::<CopyCollector<_>>().collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].0.0.is_empty());
        assert_eq!(entries[0].1, 0);
        assert_matches!(tree.children(), Some(DataTreeChildren::Empty));
    }

    #[test]
    fn test_allocative_singleton_storage() {
        let mut tree = DataTree::<u64, u64>::new();
        tree.insert([1, 2, 3].into_iter(), 123);
        let mut graph = allocative::FlameGraphBuilder::default();
        graph.visit_root(&tree.allocative_dfs());
        let output = graph.finish();
        assert_eq!(output.warnings(), "");
        let bytes: usize = output
            .flamegraph()
            .write()
            .lines()
            .map(|line| line.rsplit_once(' ').unwrap().1.parse::<usize>().unwrap())
            .sum();
        let expected = mem::size_of::<DataTreeAllocativeDfs<'_, u64, u64>>()
            + mem::size_of::<DataTree<u64, u64>>()
            + 3 * mem::size_of::<(u64, DataTree<u64, u64>)>();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_iter() {
        let expected: BTreeMap<Vec<i32>, String> = [
            (vec![1, 2, 3], "123".to_owned()),
            (vec![1, 2, 4], "124".to_owned()),
            (vec![1, 2, 5, 6], "1256".to_owned()),
            (vec![1, 3, 4], "134".to_owned()),
            (vec![1, 3, 5], "135".to_owned()),
        ]
        .into_iter()
        .collect();

        let mut tree = DataTree::<i32, String>::new();
        for (k, v) in expected.iter() {
            tree.insert(k.clone().into_iter(), v.clone());
        }
        let actual = tree
            .iter::<CopyCollector<_>>()
            .map(|(k, v)| (k.0.into_iter().collect::<Vec<_>>(), v.to_owned()))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(expected, actual);
    }

    #[test]
    fn test_allocative_dfs_does_not_recurse_in_node_stack() {
        let mut tree = DataTree::<i32, String>::new();
        tree.insert(vec![1, 2, 3, 4, 5].into_iter(), "12345".to_owned());
        tree.insert(vec![1, 2, 6].into_iter(), "126".to_owned());

        let mut graph = allocative::FlameGraphBuilder::default();
        graph.visit_root(&tree.allocative_dfs());
        let output = graph.finish();
        let source = output.flamegraph().write();

        assert_eq!("", output.warnings());
        assert!(
            source.contains(";nodes;"),
            "flamegraph source should contain flattened nodes: {source}"
        );

        for line in source.lines() {
            let data_tree_count = line.matches("::DataTree<").count();
            assert!(
                data_tree_count <= 1,
                "DataTree nodes should be visited as siblings instead of recursive stacks: {line}"
            );
        }
    }

    #[test]
    fn test_into_iter() {
        let expected: BTreeMap<Vec<i32>, String> = [
            (vec![1, 2, 3], "123".to_owned()),
            (vec![1, 2, 4], "124".to_owned()),
            (vec![1, 2, 5, 6], "1256".to_owned()),
            (vec![1, 3, 4], "134".to_owned()),
            (vec![1, 3, 5], "135".to_owned()),
        ]
        .into_iter()
        .collect();

        let mut tree = DataTree::<i32, String>::new();
        for (k, v) in expected.iter() {
            tree.insert(k.clone().into_iter(), v.clone());
        }
        let actual = tree
            .into_iter::<CopyCollector<_>>()
            .map(|(k, v)| (k.0.into_iter().collect::<Vec<_>>(), v))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(expected, actual);
    }

    #[test]
    fn test_get() {
        let path = ForwardRelativePathBuf::unchecked_new("foo/bar".to_owned());
        let contents = "contents_of_foobar".to_owned();

        let mut file_path_to_contents: DataTree<FileNameBuf, String> = DataTree::new();
        file_path_to_contents.insert(path.iter().map(|f| f.to_owned()), contents.clone());

        assert_eq!(
            file_path_to_contents
                .prefix_get_mut(&mut path.iter())
                .as_deref(),
            Some(&contents)
        );
    }

    #[test]
    fn test_prefix_remove() {
        // We need remove any artifacts at leaves when we invalidate
        // and declare artifacts at a subdirectory of that leaf path.
        let mut tree = DataTree::<i32, String>::new();
        tree.insert(vec![1, 2, 3].into_iter(), "123".to_owned());
        let key = [1, 2, 3, 4];
        let mut key_iter = key.iter();
        let removed = tree.remove(&mut key_iter);
        assert_matches!(removed, Some(DataTree::Data(val)) if val == *"123");
        // Test that only enough of key_iter to find the returned value was consumed.
        assert_eq!(key_iter.next(), Some(&4));
        assert_eq!(key_iter.next(), None);

        // Check tree is empty
        assert_matches!(tree.iter::<CopyCollector<_>>().next(), None);
    }

    #[test]
    fn test_suffix_remove() {
        let mut tree = DataTree::<i32, String>::new();
        tree.insert(vec![1, 2, 3].into_iter(), "123".to_owned());
        tree.remove([1, 2].iter());

        assert_matches!(tree.iter::<CopyCollector<_>>().next(), None);
    }

    #[test]
    fn test_remove_empty_dirs() {
        let mut tree = DataTree::<i32, String>::new();
        tree.insert(vec![1, 2, 3].into_iter(), "123".to_owned());
        tree.remove([1, 2, 3].iter());
        assert_matches!(tree, DataTree::Tree(m) if m.is_empty());
    }
}
