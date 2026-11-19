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

//! Stub implementations of vtable registry types when the `pagable` feature is disabled.
//!
//! These stubs allow code that references `DeserTypeId` and `lookup_vtable` to compile
//! without feature gates, while panicking at runtime if actually called.

use derive_more::Display;

use crate::values::layout::vtable::AValueVTable;

/// Stub `DeserTypeId` when the `pagable` feature is disabled.
#[derive(Debug, Display, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeserTypeId;

impl DeserTypeId {
    #[allow(clippy::extra_unused_type_parameters)]
    pub const fn of<T: ?Sized>() -> Self {
        DeserTypeId
    }
}

impl pagable::PagableSerialize for DeserTypeId {
    fn pagable_serialize(
        &self,
        _serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        unimplemented!("DeserTypeId serialization requires the `pagable` feature")
    }
}

impl<'de> pagable::PagableDeserialize<'de> for DeserTypeId {
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        _deserializer: &mut D,
    ) -> pagable::Result<Self> {
        unimplemented!("DeserTypeId deserialization requires the `pagable` feature")
    }
}

/// Stub `lookup_vtable` when the `pagable` feature is disabled.
pub(crate) fn lookup_vtable(_deser_type_id: DeserTypeId) -> crate::Result<&'static AValueVTable> {
    unimplemented!("lookup_vtable requires the `pagable` feature")
}
