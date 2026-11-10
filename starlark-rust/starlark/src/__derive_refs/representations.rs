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

//! What `#[derive(StarlarkRepresentations)]` writes for `#[starlark_value]` to read.

use crate::values::Value;

/// A view over a type's representations, from a value: the representations tried in order.
/// `#[starlark_value]` on the view's `StarlarkType` impl writes `get` through it.
pub trait ViewOfRepresentations<'v>: Sized {
    fn from_value(value: Value<'v>) -> Option<Self>;
}
