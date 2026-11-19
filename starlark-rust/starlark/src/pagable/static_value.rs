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

//! Static value registry for pagable serialization.
//!
//! This module provides a mapping from static value addresses (for static strings
//! like empty string and single ASCII characters) to their identifiers.

mod get_static_value_id;
mod registry;
mod static_heaps;
mod static_starlark_value;
mod static_string;
mod static_type_compiled;
mod static_value_as_type;

pub(crate) use get_static_value_id::StaticValueId;
pub(crate) use get_static_value_id::get_static_value_by_id;
pub(crate) use get_static_value_id::get_static_value_id;
pub use registry::StaticHeapEntry;
pub use registry::StaticValueEntry;
pub(crate) use static_heaps::StaticHeapId;
pub(crate) use static_heaps::get_static_heap_by_id;
pub(crate) use static_heaps::get_static_heap_id;
pub use static_starlark_value::StaticValueRegistered;
pub(crate) use static_type_compiled::TypeCompiledStaticRegistered;
pub(crate) use static_type_compiled::static_type_compiled;
