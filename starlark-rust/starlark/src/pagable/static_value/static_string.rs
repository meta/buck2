/*
 * Copyright 2026 The Starlark in Rust Authors.
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

//! Static string value registry.
//!
//! This module provides bidirectional mappings between static string value
//! addresses/IDs (empty string and single ASCII characters).

use crate::values::Value;
use crate::values::layout::static_string::VALUE_BYTE_STRINGS;
use crate::values::layout::static_string::VALUE_EMPTY_STRING;

pub(super) fn get_static_strings() -> impl Iterator<Item = Value<'static>> {
    std::iter::once(VALUE_EMPTY_STRING.erase().to_value()).chain(
        VALUE_BYTE_STRINGS
            .iter()
            .map(|repr| repr.erase().to_value()),
    )
}
