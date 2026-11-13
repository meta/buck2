/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! An optimised string HashMap which goes even faster when the keys can
//! be pre-hashed or otherwise precomputed.
//!
//! The two bottlenecks in our use of these hash tables are computing the hashes and comparing
//! the resulting keys for equality. We precompute the hashes. We also use `[usize]` to do faster
//! comparison when possible. We use the Starlark SmallHash hashes, promoted by IdHasher,
//! so we can reuse a SmallMap hash.
//!
//! Benchmarks on which the `[usize]` choice was made (mac/linux, all in ns):
//!                           8 bytes       32 bytes      64 bytes
//! slice equality (memcmp)   3.5/3.8       3.5/ 3.0      4.5/ 4.7
//! usize equality loop       1.0/1.4       2.7/ 3.5      3.5/ 6.0
//! u8 equality loop          3.4/5.7      13.7/19.7     22.6/44.8
//!
//! Measuring some sample strings, the P50 = 21 bytes, P75 = 27, P95 = 35,
//! so we can reasonably expect to hit the smaller cases most often.

use std::fmt;
use std::fmt::Debug;
use std::mem;

use allocative::Allocative;
use hashbrown::HashTable;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use starlark_derive::Trace;
use starlark_map::Hashed;

use crate as starlark;
use crate::collections::symbol::symbol::Symbol;
use crate::pagable::StarlarkDeserialize;
use crate::pagable::StarlarkDeserializeContext;
use crate::pagable::StarlarkSerialize;
use crate::pagable::StarlarkSerializeContext;
use crate::values::string::StarlarkStr;

// We use a RawTable (the thing that underlies HashMap) so we can look up efficiently
// and easily by Symbol and str, without being limited by `Borrow` traits.
#[derive(Clone, Trace, Allocative)]
pub(crate) struct SymbolMap<T>(HashTable<(Symbol, T)>);

impl<T: Debug> Debug for SymbolMap<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.iter().map(|x| (&x.0, &x.1)))
            .finish()
    }
}

impl<T> SymbolMap<T> {
    pub(crate) fn new() -> Self {
        SymbolMap::with_capacity(0)
    }

    pub(crate) fn with_capacity(capacity: usize) -> Self {
        SymbolMap(HashTable::with_capacity(capacity))
    }

    pub(crate) fn insert(&mut self, key: &str, value: T) -> Option<T> {
        let s = Symbol::new(key);
        if let Some((_, item)) = self.0.find_mut(s.hash(), |x| s == x.0) {
            Some(mem::replace(item, value))
        } else {
            // This insert doesn't remove old values, so do that manually first
            self.0.insert_unique(s.hash(), (s, value), |x| x.0.hash());
            None
        }
    }

    #[inline]
    pub(crate) fn get(&self, key: &Symbol) -> Option<&T> {
        self.0.find(key.hash(), |x| key == &x.0).map(|x| &x.1)
    }

    pub(crate) fn get_str(&self, key: &str) -> Option<&T> {
        self.get_hashed_str(Hashed::new(key))
    }

    pub(crate) fn get_hashed_str(&self, key: Hashed<&str>) -> Option<&T> {
        self.0
            .find(key.hash().promote(), |x| x.0.as_str() == *key.key())
            .map(|x| &x.1)
    }

    pub(crate) fn get_hashed_starlark_str(&self, key: Hashed<&StarlarkStr>) -> Option<&T> {
        self.0
            .find(key.hash().promote(), |x| {
                x.0.as_aligned_padded_str() == key.key().as_aligned_padded_str()
            })
            .map(|x| &x.1)
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    /// Transform the values, keeping the keys.
    pub(crate) fn map_values<U>(self, mut f: impl FnMut(T) -> U) -> SymbolMap<U> {
        let mut r = SymbolMap::with_capacity(self.len());
        for (k, v) in self.0 {
            let hash = k.hash();
            r.0.insert_unique(hash, (k, f(v)), |x| x.0.hash());
        }
        r
    }

    pub(crate) fn iter<'a>(&'a self) -> impl ExactSizeIterator<Item = &'a (Symbol, T)> + 'a {
        self.0.iter()
    }

    pub(crate) fn keys<'a>(&'a self) -> impl ExactSizeIterator<Item = &'a Symbol> + 'a {
        self.iter().map(|x| &x.0)
    }

    pub(crate) fn values<'a>(&'a self) -> impl ExactSizeIterator<Item = &'a T> + 'a {
        self.iter().map(|x| &x.1)
    }
}

impl<T: PagableSerialize> PagableSerialize for SymbolMap<T> {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        self.len().pagable_serialize(serializer)?;
        for (symbol, value) in self.iter() {
            symbol.pagable_serialize(serializer)?;
            value.pagable_serialize(serializer)?;
        }
        Ok(())
    }
}

impl<'de, T: PagableDeserialize<'de>> PagableDeserialize<'de> for SymbolMap<T> {
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let len = usize::pagable_deserialize(deserializer)?;
        let mut map = SymbolMap::with_capacity(len);
        for _ in 0..len {
            let symbol = Symbol::pagable_deserialize(deserializer)?;
            let value = T::pagable_deserialize(deserializer)?;
            map.insert(symbol.as_str(), value);
        }
        Ok(map)
    }
}

impl<T: StarlarkSerialize> StarlarkSerialize for SymbolMap<T> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.len().pagable_serialize(ctx.pagable())?;
        for (symbol, value) in self.iter() {
            symbol.pagable_serialize(ctx.pagable())?;
            value.starlark_serialize(ctx)?;
        }
        Ok(())
    }
}

impl<'fv, T: StarlarkDeserialize<'fv>> StarlarkDeserialize<'fv> for SymbolMap<T> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        let len = usize::pagable_deserialize(ctx.pagable())?;
        let mut map = SymbolMap::with_capacity(len);
        for _ in 0..len {
            let symbol = Symbol::pagable_deserialize(ctx.pagable())?;
            let value = T::starlark_deserialize(ctx)?;
            map.insert(symbol.as_str(), value);
        }
        Ok(map)
    }
}
