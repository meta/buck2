/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;

use allocative::Allocative;
use buck2_hash::BuckMutInsertionSet;
use dupe::Clone_;
use dupe::Dupe;
use pagable::Pagable;
use starlark_map::Equivalent;
use starlark_map::Hashed;
use starlark_map::small_set;

use crate::query::graph::node::LabeledNode;

#[derive(Debug, Clone, Dupe, Allocative, Pagable)]
pub struct LabelIndexed<T: LabeledNode>(pub T);

impl<T: LabeledNode> PartialEq for LabelIndexed<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.node_key() == other.0.node_key()
    }
}
impl<T: LabeledNode> Eq for LabelIndexed<T> {}
impl<T: LabeledNode> Hash for LabelIndexed<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hashed_node_key().hash().hash(state)
    }
}

impl<T: LabeledNode> Ord for LabelIndexed<T>
where
    T::Key: Ord,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.node_key().cmp(other.0.node_key())
    }
}

impl<T: LabeledNode> PartialOrd for LabelIndexed<T>
where
    T::Key: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.0.node_key().partial_cmp(other.0.node_key())
    }
}

struct LabelIndexer<'a, T: LabeledNode>(Hashed<&'a T::Key>);

impl<'a, T: LabeledNode> Equivalent<LabelIndexed<T>> for LabelIndexer<'a, T> {
    fn equivalent(&self, key: &LabelIndexed<T>) -> bool {
        *self.0.key() == key.0.node_key()
    }
}
impl<'a, T: LabeledNode> Hash for LabelIndexer<'a, T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash().hash(state)
    }
}

/// This is a Set that contains `LabeledNode`s, but for all trait operation like `Eq`, `Ord` all
/// operate only on the `NodeRef` of the `LabeledNode`.
#[derive(Debug, Clone, Allocative, Pagable)]
pub struct LabelIndexedSet<T: LabeledNode> {
    nodes: BuckMutInsertionSet<LabelIndexed<T>>,
}

impl<T: LabeledNode> PartialEq for LabelIndexedSet<T> {
    fn eq(&self, other: &Self) -> bool {
        self.nodes == other.nodes
    }
}

impl<T: LabeledNode> Eq for LabelIndexedSet<T> {}

impl<T: LabeledNode> Hash for LabelIndexedSet<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.nodes.hash(state)
    }
}

impl<T: LabeledNode> LabelIndexedSet<T> {
    pub fn new() -> Self {
        Self {
            nodes: BuckMutInsertionSet::new(),
        }
    }

    pub fn with_capacity(n: usize) -> Self {
        Self {
            nodes: BuckMutInsertionSet::with_capacity(n),
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn get(&self, value: &T::Key) -> Option<&T> {
        self.nodes
            .get(&LabelIndexer(Hashed::new(value)))
            .map(|e| &e.0)
    }

    pub fn take(&mut self, value: &T::Key) -> Option<T> {
        self.nodes
            .take(&LabelIndexer(Hashed::new(value)))
            .map(|e| e.0)
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            iter: self.nodes.iter(),
        }
    }

    #[allow(clippy::should_implement_trait)] // the std trait requires concrete or boxed iterator type
    pub fn into_iter(self) -> impl ExactSizeIterator<Item = T> {
        self.nodes.into_iter().map(|e| e.0)
    }

    pub fn insert(&mut self, value: T) -> bool {
        self.nodes.insert(LabelIndexed(value))
    }

    pub fn insert_unique_unchecked(&mut self, value: T) {
        self.nodes.insert_unique_unchecked(LabelIndexed(value));
    }

    pub fn contains(&self, value: &T::Key) -> bool {
        self.nodes.contains(&LabelIndexer(Hashed::new(value)))
    }

    pub fn get_index(&self, index: usize) -> Option<&T> {
        self.nodes.get_index(index).map(|e| &e.0)
    }

    pub fn last(&self) -> Option<&T> {
        self.nodes.last().map(|e| &e.0)
    }
}

#[derive(Clone_)]
pub struct Iter<'a, T: LabeledNode> {
    iter: small_set::Iter<'a, LabelIndexed<T>>,
}

impl<'a, T: LabeledNode> Iterator for Iter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|e| &e.0)
    }
}

impl<'a, T: LabeledNode> ExactSizeIterator for Iter<'a, T> {
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<'a, T: LabeledNode> Dupe for Iter<'a, T> {}

impl<'a, T: LabeledNode> IntoIterator for &'a LabelIndexedSet<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        Iter {
            iter: self.nodes.iter(),
        }
    }
}

impl<T: LabeledNode> FromIterator<T> for LabelIndexedSet<T> {
    fn from_iter<Iter: IntoIterator<Item = T>>(iter: Iter) -> Self {
        let iter = iter.into_iter();
        let mut res = Self::with_capacity(iter.size_hint().0);
        for item in iter {
            res.insert(item);
        }
        res
    }
}
