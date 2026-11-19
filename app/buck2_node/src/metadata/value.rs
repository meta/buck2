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
use std::hash::Hasher;
use std::sync::Arc;

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;
use strong_hash::StrongHash;

#[derive(
    Debug,
    // serde_json's equality ignores map ordering, but we should have
    // equality with ordering preserved here.
    // TODO: implement our own equality here that preserves ordering
    Eq,
    PartialEq,
    Clone,
    Dupe,
    Allocative,
    Default,
    Serialize,
    Deserialize,
    Pagable
)]
pub struct MetadataValue(pub Arc<serde_json::Value>);

impl MetadataValue {
    pub fn new(value: serde_json::Value) -> Self {
        let value = Arc::new(value);
        Self(value)
    }

    pub fn as_json(&self) -> &serde_json::Value {
        self.0.as_ref()
    }
}

impl Hash for MetadataValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        hash_json_value(&self.0, state);
    }
}

/// The JSON walk feeds every value byte plus a tag per node, so it satisfies
/// `StrongHash`'s completeness contract; there is no cached hash to leak.
impl StrongHash for MetadataValue {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        hash_json_value(&self.0, state);
    }
}

// Objects are hashed in map iteration order while JSON equality ignores key
// order, so equal values hash equally only while iteration is canonical
// (sorted). That holds for serde_json's default `BTreeMap` map backing and
// breaks under its `preserve_order` feature (insertion-ordered `IndexMap`).
pub(crate) fn hash_json_value<H: Hasher>(v: &serde_json::Value, state: &mut H) {
    use serde_json::Value;

    match v {
        Value::Null => {
            state.write_u8(0);
        }
        Value::Bool(v) => {
            state.write_u8(1);
            v.hash(state);
        }
        Value::Number(v) => {
            state.write_u8(2);
            if let Some(v) = v.as_u64() {
                state.write_u8(1);
                v.hash(state);
            } else if let Some(v) = v.as_i64() {
                state.write_u8(2);
                v.hash(state);
            } else {
                state.write_u8(3);
                v.to_string().hash(state);
            }
        }
        Value::String(v) => {
            state.write_u8(3);
            v.hash(state);
        }
        Value::Array(vals) => {
            state.write_u8(4);
            state.write_usize(vals.len());
            for v in vals {
                hash_json_value(v, state);
            }
        }
        Value::Object(vals) => {
            state.write_u8(5);
            state.write_usize(vals.len());
            for (k, v) in vals {
                k.hash(state);
                hash_json_value(v, state);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_json_object_iteration_is_sorted() {
        let v: serde_json::Value = serde_json::from_str(r#"{"b":1,"a":2}"#).unwrap();
        let keys: Vec<_> = v.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            ["a", "b"],
            "hash_json_value hashes objects in iteration order, so iteration must be canonical (sorted)"
        );
    }
}
