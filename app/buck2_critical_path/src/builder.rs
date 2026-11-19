/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;
use std::hash::Hash;

use buck2_hash::BuckMutInsertionMap;
use starlark_map::Hashed;

use crate::graph::Graph;
use crate::graph::GraphVertex;
use crate::types::VertexData;
use crate::types::VertexId;
use crate::types::VertexKeys;

#[derive(buck2_error::Error, Debug)]
#[buck2(tier0)]
pub enum PushError<K: Display> {
    #[error("duplicate key: {key}")]
    DuplicateKey { key: K },

    #[error("overflow")]
    Overflow,
}

pub struct GraphBuilder<K: Hash + Eq, D> {
    keys: BuckMutInsertionMap<K, VertexId>,
    data: Vec<D>,
    vertices: Vec<GraphVertex>,
    edges: Vec<VertexId>,
}

impl<K, D> GraphBuilder<K, D>
where
    K: Hash + Eq + Display,
{
    pub fn new() -> Self {
        Self {
            keys: Default::default(),
            data: Default::default(),
            vertices: Default::default(),
            edges: Default::default(),
        }
    }

    pub fn push(
        &mut self,
        key: K,
        deps: impl IntoIterator<Item = K>,
        data: D,
    ) -> Result<(), PushError<K>> {
        let idx: u32 = self
            .vertices
            .len()
            .try_into()
            .map_err(|_| PushError::Overflow)?;

        // We need to constrain ourselves to i32::MAX in order tosupport optionals.
        if idx > i32::MAX as u32 {
            return Err(PushError::Overflow);
        }

        let idx = VertexId::new(idx);

        let hashed = Hashed::new(key);
        if self.keys.contains_key_hashed(hashed.as_ref()) {
            return Err(PushError::DuplicateKey {
                key: hashed.into_key(),
            });
        }
        self.keys.insert_hashed(hashed, idx);

        self.data.push(data);

        let edges_idx = self
            .edges
            .len()
            .try_into()
            .map_err(|_| PushError::Overflow)?;

        let mut edges_count = 0;

        for dep in deps {
            let dep_idx = match self.keys.get(&dep) {
                Some(idx) => idx,
                None => continue,
            };
            self.edges.push(*dep_idx);
            edges_count += 1;
        }

        self.vertices.push(GraphVertex {
            edges_idx,
            edges_count,
        });

        Ok(())
    }

    pub fn finish(self) -> (Graph, VertexKeys<K>, VertexData<D>) {
        (
            Graph {
                edges: self.edges,
                vertices: VertexData::new(self.vertices),
            },
            VertexKeys::new(self.keys),
            VertexData::new(self.data),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_finish() {
        let mut builder = GraphBuilder::new();
        builder
            .push("foo", std::iter::empty(), "foo value")
            .unwrap();
        builder
            .push("bar", std::iter::empty(), "bar value")
            .unwrap();
        let (_graph, keys, data) = builder.finish();
        let foo_idx = keys.get(&"foo").unwrap();
        let bar_idx = keys.get(&"bar").unwrap();

        assert_eq!(keys[foo_idx], "foo");
        assert_eq!(keys[bar_idx], "bar");
        assert_eq!(data[foo_idx], "foo value");
        assert_eq!(data[bar_idx], "bar value");
    }

    #[test]
    fn test_missing_ok() {
        let mut builder = GraphBuilder::new();
        builder
            .push("foo", std::iter::once("bar"), "foo value")
            .unwrap();
    }

    #[test]
    fn test_duplicate_err() {
        let mut builder = GraphBuilder::new();
        builder
            .push("foo", std::iter::empty(), "foo value")
            .unwrap();
        assert!(
            builder
                .push("foo", std::iter::empty(), "foo value")
                .is_err()
        );
    }
}
