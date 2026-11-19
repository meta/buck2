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

//! Ordered map optimized for starlark-rust use cases.

// Hints we disagree with
#![allow(clippy::missing_safety_doc)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod ordered_map;
pub mod ordered_set;
pub mod small_map;
pub mod small_set;
pub mod sorted_vec;
pub mod unordered_map;
pub mod unordered_set;

pub use equivalent::Equivalent;
// The hashing layer lives in `mini_map` now; these are the names starlark has always used for it.
pub use mini_map::FastHasher as StarlarkHasher;
pub use mini_map::FastHasherBuilder as StarlarkHasherBuilder;
pub use mini_map::Hashed;
pub use mini_map::ShortHash as StarlarkHashValue;
