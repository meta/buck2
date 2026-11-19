/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::hash::Hash;
use std::marker::PhantomData;
use std::ops::Index;
use std::ops::IndexMut;

use buck2_hash::BuckMutInsertionMap;
use derive_more::Display;

#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
pub struct GraphVertexKind;

#[derive(Copy, Clone, Default, Ord, PartialOrd, PartialEq, Eq, Debug)]
pub struct CriticalPathIndexKind;

pub trait VertexKind: Copy + Clone + Default {}

impl VertexKind for GraphVertexKind {}
impl VertexKind for CriticalPathIndexKind {}

/// The ID of a Vertex. This can be used to index into AbstractVertexData. Those IDs are given a
/// kind so we don't confuse indices in a critical path with vertex indices in a graph.
#[derive(Copy, Clone, Default, Ord, PartialOrd, PartialEq, Eq, Display, Hash)]
#[display("{}", self.0)]
pub struct AbstractVertexId<Kind: VertexKind>(u32, PhantomData<Kind>);

impl<Kind> AbstractVertexId<Kind>
where
    Kind: VertexKind,
{
    pub(crate) fn new(v: u32) -> Self {
        Self(v, PhantomData)
    }

    #[cfg(test)]
    pub(crate) fn into_inner(self) -> u32 {
        self.0
    }
}

impl<T, Kind> Index<AbstractVertexId<Kind>> for AbstractVertexData<T, Kind>
where
    Kind: VertexKind,
{
    type Output = T;

    #[inline]
    fn index(&self, index: AbstractVertexId<Kind>) -> &Self::Output {
        &self.0[index.0 as usize]
    }
}

impl<T, Kind> IndexMut<AbstractVertexId<Kind>> for AbstractVertexData<T, Kind>
where
    Kind: VertexKind,
{
    #[inline]
    fn index_mut(&mut self, index: AbstractVertexId<Kind>) -> &mut Self::Output {
        &mut self.0[index.0 as usize]
    }
}

impl<K, Kind> Index<AbstractVertexId<Kind>> for AbstractKeys<K, Kind>
where
    Kind: VertexKind,
{
    type Output = K;

    fn index(&self, index: AbstractVertexId<Kind>) -> &Self::Output {
        // NOTE: Unwrap is par for the course in [] access.
        self.0.get_index(index.0 as usize).unwrap().0
    }
}

/// An Optional AbstractVertexId.
#[derive(Copy, Clone)]
pub struct AbstractOptionalVertexId<Kind: VertexKind>(
    /// -1 means None.
    i32,
    PhantomData<Kind>,
);

impl<Kind> From<AbstractVertexId<Kind>> for AbstractOptionalVertexId<Kind>
where
    Kind: VertexKind,
{
    #[inline]
    fn from(node: AbstractVertexId<Kind>) -> Self {
        Self(node.0 as _, PhantomData)
    }
}

impl<Kind> AbstractOptionalVertexId<Kind>
where
    Kind: VertexKind,
{
    #[inline]
    pub fn none() -> Self {
        Self(-1, PhantomData)
    }

    #[inline]
    pub fn is_some(self) -> bool {
        self.0 >= 0
    }

    #[inline]
    pub fn into_option(self) -> Option<AbstractVertexId<Kind>> {
        if self.0 >= 0 {
            return Some(AbstractVertexId(self.0 as _, PhantomData));
        }

        None
    }
}

/// A Vec that stores data indexed by Vertex.
#[derive(Clone)]
pub struct AbstractVertexData<T, Kind: VertexKind>(Vec<T>, PhantomData<Kind>);

impl<T, Kind> AbstractVertexData<T, Kind>
where
    Kind: VertexKind,
{
    pub(crate) fn new(v: Vec<T>) -> Self {
        Self(v, PhantomData)
    }

    pub fn keys(&self) -> impl DoubleEndedIterator<Item = AbstractVertexId<Kind>> + use<T, Kind> {
        // By construction the length of this is always less than the maximum vertex id.
        let len: u32 = self.0.len().try_into().unwrap();
        (0..len).map(AbstractVertexId::new)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (AbstractVertexId<Kind>, &T)> {
        self.keys().map(|k| (k, &self.0[k.0 as usize]))
    }

    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.0.iter_mut()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn get(&self, index: AbstractVertexId<Kind>) -> Option<&T> {
        self.0.get(index.0 as usize)
    }

    pub fn into_inner(self) -> Vec<T> {
        self.0
    }

    pub fn map_ref<TT>(&self, f: impl FnMut(&T) -> TT) -> AbstractVertexData<TT, Kind> {
        AbstractVertexData(self.values().map(f).collect(), PhantomData)
    }

    pub fn try_map_ref<TT, E>(
        &self,
        f: impl FnMut(&T) -> Result<TT, E>,
    ) -> Result<AbstractVertexData<TT, Kind>, E> {
        Ok(AbstractVertexData(
            self.values().map(f).collect::<Result<_, E>>()?,
            PhantomData,
        ))
    }
}

impl<Kind> fmt::Debug for AbstractVertexId<Kind>
where
    Kind: VertexKind,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl<Kind> fmt::Debug for AbstractOptionalVertexId<Kind>
where
    Kind: VertexKind,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.into_option(), f)
    }
}

/// The keys for a graph built using GraphBuilder. This can be accessed using both VertexId
/// (because the IDs are assigned in order), or K.
#[derive(Clone)]
pub struct AbstractKeys<K, Kind: VertexKind>(BuckMutInsertionMap<K, AbstractVertexId<Kind>>);

impl<K, Kind> AbstractKeys<K, Kind>
where
    Kind: VertexKind,
    K: Hash + Eq,
{
    pub(crate) fn new(v: BuckMutInsertionMap<K, AbstractVertexId<Kind>>) -> Self {
        Self(v)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (AbstractVertexId<Kind>, &K)> {
        self.0.iter().map(|(key, idx)| (*idx, key))
    }

    pub fn get(&self, k: &K) -> Option<AbstractVertexId<Kind>> {
        self.0.get(k).copied()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

pub type VertexId = AbstractVertexId<GraphVertexKind>;
pub type OptionalVertexId = AbstractOptionalVertexId<GraphVertexKind>;
pub type VertexData<T> = AbstractVertexData<T, GraphVertexKind>;
pub type VertexKeys<T> = AbstractKeys<T, GraphVertexKind>;

pub type CriticalPathIndex = AbstractVertexId<CriticalPathIndexKind>;
pub type OptionalCriticalPathIndex = AbstractOptionalVertexId<CriticalPathIndexKind>;
pub type CriticalPathVertexData<T> = AbstractVertexData<T, CriticalPathIndexKind>;

impl CriticalPathIndex {
    pub(crate) fn zero() -> Self {
        Self::new(0)
    }

    pub(crate) fn successor(self) -> Self {
        Self::new(self.0 + 1)
    }
}
