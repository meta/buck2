/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::hash_map::Entry;
use std::io::Write;

use anyhow::Context as _;
use serde::Serializer;
use serde::ser::SerializeSeq;

use crate::HashMap;
use crate::introspection::AnyKey;
use crate::introspection::graph::GraphIntrospectable;

pub fn serialize_graph(
    graph: &GraphIntrospectable,
    nodes: impl Write,
    mut edges: impl Write,
    mut nodes_currently_running: impl Write,
) -> anyhow::Result<()> {
    let mut reg = NodeRegistry::new();

    for (k, vs) in graph.edges() {
        let k = reg.map(k);

        for v in vs.into_iter() {
            let v = reg.map(v);
            edges
                .write_all(format!("{k}\t{v}\n").as_bytes())
                .context("Failed to write edge")?;
        }
    }

    for (k, v, s) in graph.keys_currently_running() {
        let k_short_type_name = k.short_type_name();
        let k_str = k.to_string();
        let k_n = reg.map(k);
        writeln!(
            nodes_currently_running,
            "{k_n}\t{v}\t{s:?}\t{k_short_type_name}\t{k_str}",
        )?;
    }

    reg.write(nodes)?;

    Ok(())
}

pub fn serialize_dense_graph<S>(graph: &GraphIntrospectable, writer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let num_nodes = graph.len_for_introspection();

    let mut seq = writer.serialize_seq(Some(num_nodes))?;
    for node in graph.nodes() {
        seq.serialize_element(&node)?;
    }
    seq.end()
}

struct NodeRegistry {
    keys: HashMap<AnyKey, u64>,
}

impl NodeRegistry {
    fn new() -> Self {
        Self {
            keys: HashMap::default(),
        }
    }

    fn map(&mut self, key: AnyKey) -> u64 {
        let next_idx = self.keys.len() as u64;
        match self.keys.entry(key) {
            Entry::Occupied(e) => *e.get(),
            Entry::Vacant(e) => *e.insert(next_idx),
        }
    }

    fn write(self, mut out: impl Write) -> anyhow::Result<()> {
        // NOTE: Writing out the node type every time is wasteful here, we could optimize, though
        // if we compress then that goes away, so optimizing *here* might not make the most sense.
        let mut keys = self
            .keys
            .into_iter()
            .map(|(key, idx)| (idx, key))
            .collect::<Vec<_>>();

        keys.sort_by_key(|(idx, _)| *idx);

        for (idx, key) in keys {
            out.write_all(format!("{}\t{}\t{}\n", idx, key.short_type_name(), key).as_bytes())
                .context("Failed to write node")?;
        }
        Ok(())
    }
}
