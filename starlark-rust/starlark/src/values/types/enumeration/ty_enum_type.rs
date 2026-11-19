/*
 * Copyright 2018 The Starlark in Rust Authors.
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

use allocative::Allocative;
use pagable::Pagable;

use crate::typing::Ty;
use crate::values::types::type_instance_id::TypeInstanceId;

#[doc(hidden)]
#[derive(Allocative, Ord, PartialOrd, Debug, Pagable)]
pub(crate) struct TyEnumData {
    /// Name of the enum type.
    pub(super) name: String,
    /// Globally unique id of the enum type.
    // Id must be last so `Ord` is deterministic.
    pub(super) id: TypeInstanceId,
    /// Type of enum variant.
    pub(super) ty_enum_value: Ty,
    /// Type of enum type value.
    pub(super) ty_enum_type: Ty,
}

impl PartialEq for TyEnumData {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for TyEnumData {}

impl std::hash::Hash for TyEnumData {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Do not hash `id` because hashing should be deterministic.
        self.name.hash(state);
    }
}
