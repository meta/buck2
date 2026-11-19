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

//! Tools for lifetime conversions.

mod any_lifetime;
mod impls;
mod provides_static_type;

pub use any_lifetime::AnyLifetime;
pub use provides_static_type::IsStaticType;
pub use provides_static_type::ProvidesStaticType;
pub use provides_static_type::ReinfectStatic;
pub use starlark_derive::ProvidesStaticType;
