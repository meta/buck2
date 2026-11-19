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
use std::fmt::Display;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;

use cmp_any::PartialEqAny;
use dupe::Dupe;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use serde::de::Error;
use serde::de::Unexpected;
use serde::de::Visitor;

use crate::HashMap;
use crate::HashSet;
use crate::core::graph::introspection::VersionedGraphIntrospectable;
use crate::core::versions::introspection::VersionIntrospectable;
use crate::introspection::DiceTaskState;
use crate::introspection::serialize_dense_graph;
use crate::key::DiceKey;

pub struct GraphIntrospectable {
    pub(crate) graph: VersionedGraphIntrospectable,
    pub(crate) version_data: VersionIntrospectable,
    pub(crate) key_map: HashMap<DiceKey, AnyKey>,
}

impl GraphIntrospectable {
    pub fn keys<'a>(&'a self) -> Box<dyn Iterator<Item = AnyKey> + 'a> {
        let keys: HashSet<DiceKey> = self.graph.nodes.keys().map(|(_, key)| *key).collect();
        Box::new(
            keys.into_iter()
                .map(|k| self.key_map.get(&k).expect("key should be present").clone()),
        )
    }

    pub fn edges<'a>(&'a self) -> Box<dyn Iterator<Item = (AnyKey, Vec<AnyKey>)> + 'a> {
        Box::new(self.graph.edges.iter().map(|(k, deps)| {
            (
                self.key_map.get(k).expect("key should be present").clone(),
                deps.iter()
                    .map(|k| self.key_map.get(k).expect("key should be present").clone())
                    .collect(),
            )
        }))
    }

    pub fn keys_currently_running(&self) -> Vec<(AnyKey, VersionNumber, DiceTaskState)> {
        self.version_data.keys_currently_running(&self.key_map)
    }

    pub fn versions_currently_running(&self) -> Vec<VersionNumber> {
        self.version_data.versions_currently_running()
    }

    pub fn nodes<'a>(&'a self) -> Box<dyn Iterator<Item = SerializedGraphNodeForKey> + 'a> {
        Box::new(self.graph.nodes.iter().map(|((branch, key), node)| {
            let any_k = self.key_map.get(key).expect("key should be present");
            SerializedGraphNodeForKey {
                id: KeyID(node.node_id.0),
                branch: branch.as_u32(),
                key: any_k.to_string(),
                type_name: any_k.type_name().to_owned(),
                node: node.clone(),
            }
        }))
    }

    pub fn len_for_introspection(&self) -> usize {
        self.graph.nodes.len()
    }
}

impl Serialize for GraphIntrospectable {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_dense_graph(self, serializer)
    }
}

#[derive(PartialEq, Eq, Hash, Serialize, Deserialize, Clone, Dupe, Copy)]
#[serde(transparent)]
pub struct KeyID(pub usize);

#[derive(
    PartialEq,
    Eq,
    Hash,
    Clone,
    Dupe,
    Copy,
    Ord,
    PartialOrd,
    derive_more::Display
)]
pub struct VersionNumber(pub usize);

impl Serialize for VersionNumber {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("v{}", self.0))
    }
}

struct VersionNumberVisitor;

impl<'de> Visitor<'de> for VersionNumberVisitor {
    type Value = VersionNumber;

    fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
        formatter.write_str("string of format `vX` where X is a usize, like `v2`")
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E>
    where
        E: Error,
    {
        let mut chars = v.chars();

        if chars.next() != Some('v') {
            Err(Error::invalid_value(Unexpected::Str(v), &self))
        } else {
            chars
                .as_str()
                .parse::<usize>()
                .map_err(|_| Error::invalid_value(Unexpected::Str(v), &self))
                .map(VersionNumber)
        }
    }
}

impl<'de> Deserialize<'de> for VersionNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(VersionNumberVisitor)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum GraphNodeKind {
    Occupied,
    Transient,
    Vacant,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CellHistory {
    pub valid_ranges: Vec<(VersionNumber, Option<VersionNumber>)>,
    pub force_dirtied_at: Vec<VersionNumber>,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum HistoryState {
    Verified,
    Dirty,
    ForceDirty,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SerializedGraphNode {
    pub node_id: KeyID,
    pub kind: GraphNodeKind,
    pub history: CellHistory,
    pub deps: HashSet<KeyID>,
    pub rdeps: Vec<KeyID>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SerializedGraphNodeForKey {
    pub id: KeyID,
    /// The branch the node describes. A key has one node per live branch it is known on, all
    /// with the same `id`.
    pub branch: u32,
    pub key: String,
    pub type_name: String,
    pub node: SerializedGraphNode,
}

pub trait KeyForIntrospection: Display + Send + 'static {
    fn get_key_equality(&self) -> PartialEqAny<'_>;

    fn hash(&self, state: &mut dyn Hasher);

    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn box_clone(&self) -> Box<dyn KeyForIntrospection>;
}

impl<K> KeyForIntrospection for K
where
    K: Clone + Display + Hash + Eq + Send + 'static,
{
    fn get_key_equality(&self) -> PartialEqAny<'_> {
        PartialEqAny::new(self)
    }

    fn hash(&self, mut state: &mut dyn Hasher) {
        K::hash(self, &mut state)
    }

    fn box_clone(&self) -> Box<dyn KeyForIntrospection> {
        Box::new(self.clone())
    }
}

pub struct AnyKey {
    pub inner: Box<dyn KeyForIntrospection>,
}

/// Shorten a type name like
/// ```ignore
/// <dice::ctx::DiceComputations as buck2_interpreter::extra::buckconfig::HasLegacyBuckConfigForStarlark>
///     ::get_legacy_buck_config_for_starlark::{{closure}}::LegacyBuckConfigForStarlarkKey
/// ```
/// to
/// `LegacyBuckConfigForStarlarkKey`.
pub fn short_type_name(type_name: &str) -> &str {
    type_name
        .rsplit("::")
        .next()
        .filter(|s| {
            // TODO(nga): shorten types like `Vec<String>`.
            //   Ignore them for now.
            !s.contains('>')
        })
        .unwrap_or(type_name)
}

impl AnyKey {
    pub fn new(k: impl KeyForIntrospection) -> Self {
        Self { inner: Box::new(k) }
    }

    pub fn type_name(&self) -> &'static str {
        self.inner.type_name()
    }

    pub fn short_type_name(&self) -> &'static str {
        short_type_name(self.type_name())
    }
}

impl Clone for AnyKey {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.box_clone(),
        }
    }
}

impl Display for AnyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl PartialEq for AnyKey {
    fn eq(&self, other: &Self) -> bool {
        self.inner.get_key_equality() == other.inner.get_key_equality()
    }
}

impl Eq for AnyKey {}

impl Hash for AnyKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state)
    }
}

#[cfg(test)]
mod tests {
    use std::any::type_name;

    use crate::introspection::graph::short_type_name;

    #[test]
    fn test_short_type_name() {
        assert_eq!("String", short_type_name(type_name::<String>()));

        let closure = || {
            struct MyLocalTypeInClosure;
            short_type_name(type_name::<MyLocalTypeInClosure>())
        };
        assert_eq!("MyLocalTypeInClosure", closure());

        // We do not shorten generic types yet.
        assert_eq!(
            type_name::<Vec<String>>(),
            short_type_name(type_name::<Vec<String>>())
        );
    }
}
