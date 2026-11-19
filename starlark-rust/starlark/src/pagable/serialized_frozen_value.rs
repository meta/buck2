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

//! Wire format for serialized frozen value references.

use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;

use crate::pagable::heap_ref_id::HeapRefId;
use crate::pagable::static_value::StaticValueId;

/// Wire representation of a frozen value reference. Heap pointers always carry
/// an explicit `heap_id` so the encoding doesn't depend on which heap
/// is currently being (de)serialized.
///
/// `value_index` is the value's position in the heap's serialization order
/// (drop bump first, then non-drop bump). It resolves either through the
/// resident heap's chunk index or the heap's lazy-deserialization state.
#[derive(Debug, Clone)]
pub(crate) enum SerializedFrozenValue {
    HeapPtr {
        heap_id: HeapRefId,
        value_index: u32,
        is_str: bool,
    },
    InlineInt(i32),
    Static(StaticValueId),
}

const TAG_HEAP_PTR: u8 = 0;
const TAG_INLINE_INT: u8 = 1;
const TAG_STATIC: u8 = 2;

impl PagableSerialize for SerializedFrozenValue {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        match self {
            SerializedFrozenValue::HeapPtr {
                heap_id,
                value_index,
                is_str,
            } => {
                TAG_HEAP_PTR.pagable_serialize(serializer)?;
                heap_id.pagable_serialize(serializer)?;
                value_index.pagable_serialize(serializer)?;
                is_str.pagable_serialize(serializer)?;
            }
            SerializedFrozenValue::InlineInt(v) => {
                TAG_INLINE_INT.pagable_serialize(serializer)?;
                v.pagable_serialize(serializer)?;
            }
            SerializedFrozenValue::Static(id) => {
                TAG_STATIC.pagable_serialize(serializer)?;
                id.pagable_serialize(serializer)?;
            }
        }
        Ok(())
    }
}

impl<'de> PagableDeserialize<'de> for SerializedFrozenValue {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let tag = u8::pagable_deserialize(deserializer)?;
        match tag {
            TAG_HEAP_PTR => {
                let heap_id = HeapRefId::pagable_deserialize(deserializer)?;
                let value_index = u32::pagable_deserialize(deserializer)?;
                let is_str = bool::pagable_deserialize(deserializer)?;
                Ok(SerializedFrozenValue::HeapPtr {
                    heap_id,
                    value_index,
                    is_str,
                })
            }
            TAG_INLINE_INT => {
                let v = i32::pagable_deserialize(deserializer)?;
                Ok(SerializedFrozenValue::InlineInt(v))
            }
            TAG_STATIC => {
                let id = StaticValueId::pagable_deserialize(deserializer)?;
                Ok(SerializedFrozenValue::Static(id))
            }
            _ => Err(anyhow::anyhow!(
                "Invalid SerializedFrozenValue tag: {}",
                tag
            )),
        }
    }
}
