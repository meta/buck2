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

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark_derive::type_matcher;

use crate as starlark;
use crate::values::Value;
use crate::values::record::Record;
use crate::values::types::type_instance_id::TypeInstanceId;
use crate::values::typing::type_compiled::matcher::TypeMatcher;
use crate::values::typing::type_compiled::matcher::TypeMatcherDyn;

/// The runtime [`TypeMatcher`] for a specific `record` type.
///
/// Matches exactly the [`Record`]s produced by one `record()` definition, and no
/// others. Matching is by the record type's identity, not its field shape, so two
/// structurally identical record types (for example, the same fields defined in
/// two different files) never match each other.
#[derive(Hash, Debug, Eq, PartialEq, Clone, Dupe, Allocative, Pagable)]
#[pagable_typetag(TypeMatcherDyn)]
pub struct RecordTypeMatcher {
    pub(crate) id: TypeInstanceId,
}

#[type_matcher]
impl TypeMatcher for RecordTypeMatcher {
    fn matches(&self, value: Value) -> bool {
        match Record::from_value(value) {
            None => false,
            Some(record) => record.record_type_id() == self.id,
        }
    }
}
