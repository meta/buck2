/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//!
//! Interfaces for introspection of the DICE graph

use crate::introspection::graph::AnyKey;

pub mod graph;
pub(crate) mod introspect;

pub use crate::introspection::introspect::serialize_dense_graph;
pub use crate::introspection::introspect::serialize_graph;

/// A point-in-time snapshot of a `DiceTask`'s state, surfaced for graph introspection (e.g.
/// `graph::GraphIntrospectable::keys_currently_running`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiceTaskState {
    /// Task is in progress (not yet completed)
    InProgress,
    /// Value is ready to be used
    Ready,
    /// Task will never become Ready
    Terminated,
}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use anyhow::Context as _;
    use async_trait::async_trait;
    use derive_more::Display;
    use dice_futures::cancellation::CancellationContext;
    use dupe::Dupe;
    use pagable::Pagable;
    use pagable::pagable_typetag;

    use crate::DiceKeyDyn;
    use crate::HashMap;
    use crate::api::computations::DiceComputations;
    use crate::api::cycles::DetectCycles;
    use crate::api::key::Key;
    use crate::api::key::NoValueSerialize;
    use crate::api::key::ValueSerialize;
    use crate::dice::Dice;
    use crate::introspection::graph::SerializedGraphNodeForKey;
    use crate::introspection::serialize_graph;

    #[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct KeyA(usize);

    #[async_trait]
    impl Key for KeyA {
        type Value = ();

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            if self.0 > 0 {
                ctx.compute(&KeyA(self.0 - 1)).await.unwrap();
            } else {
                ctx.compute(&KeyB).await.unwrap();
            }
        }

        fn equality_behavior() -> crate::EqualityBehavior<Self::Value> {
            crate::EqualityBehavior::Compare(|_, _| unimplemented!())
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    #[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct KeyB;

    #[async_trait]
    impl Key for KeyB {
        type Value = ();

        async fn compute(
            &self,
            _: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            // Noop
        }

        fn equality_behavior() -> crate::EqualityBehavior<Self::Value> {
            crate::EqualityBehavior::Compare(|_, _| unimplemented!())
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_serialization() -> anyhow::Result<()> {
        let dice = Dice::builder().build(DetectCycles::Disabled);
        let root = dice.new_root().await;
        let ctx = dice.updater(root).commit().await;
        ctx.compute(&KeyA(3)).await?;

        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut nodes_currently_running = Vec::new();

        serialize_graph(
            &dice.to_introspectable(),
            &mut nodes,
            &mut edges,
            &mut nodes_currently_running,
        )
        .unwrap();
        let nodes = String::from_utf8(nodes)?;
        let edges = String::from_utf8(edges)?;

        let mut node_map = HashMap::<String, u64>::default();
        let mut edge_list = Vec::<(u64, u64)>::new();

        for line in nodes.lines() {
            let mut it = line.trim().split('\t');
            let idx = it.next().context("No idx")?.parse()?;
            let _key_type = it.next().context("No key type")?;
            let key = it.next().context("No key")?;
            node_map.insert(key.into(), idx);
        }

        for line in edges.lines() {
            let mut it = line.trim().split('\t');
            let from = it.next().context("No idx")?.parse()?;
            let to = it.next().context("No key")?.parse()?;
            edge_list.push((from, to));
        }

        let a3 = *node_map.get("KeyA(3)").context("Missing key")?;
        let a2 = *node_map.get("KeyA(2)").context("Missing key")?;
        let a1 = *node_map.get("KeyA(1)").context("Missing key")?;
        let a0 = *node_map.get("KeyA(0)").context("Missing key")?;
        let b = *node_map.get("KeyB").context("Missing key")?;

        let mut expected_edge_list = vec![(a3, a2), (a2, a1), (a1, a0), (a0, b)];
        expected_edge_list.sort_unstable();
        edge_list.sort_unstable();
        assert_eq!(expected_edge_list, edge_list);

        // TODO(cjhopman): fix this
        // assert!(nodes_currently_running.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_serialization_dense() -> anyhow::Result<()> {
        let dice = Dice::builder().build(DetectCycles::Disabled);
        let root = dice.new_root().await;
        let ctx = dice.updater(root).commit().await;
        ctx.compute(&KeyA(3)).await?;

        let node =
            bincode::serde::encode_to_vec(dice.to_introspectable(), bincode::config::legacy())?;

        let (_out, _): (Vec<SerializedGraphNodeForKey>, _) =
            bincode::serde::decode_from_slice(&node, bincode::config::legacy())?;
        Ok(())
    }
}
