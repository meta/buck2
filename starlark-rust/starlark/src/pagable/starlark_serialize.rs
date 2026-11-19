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

//! Trait for Starlark values that can be serialized to pagable format.

use pagable::PagableSerializer;

use crate::values::Value;
use crate::values::layout::heap::sealed::FrozenHeapPtr;

/// Opaque context that must follow Starlark values across deferred pagable serialization.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct StarlarkSerializeScope {
    /// Root of the ownership graph containing values serialized by this context.
    ///
    /// Pagable-only boundaries carry this identity so pointer lookup recovery can revisit
    /// only the heaps reachable from the original Starlark source.
    pub(crate) root: Option<FrozenHeapPtr>,
}

impl StarlarkSerializeScope {
    pub(crate) const fn rootless() -> Self {
        Self { root: None }
    }
}

/// Trait for Starlark values that can be serialized.
///
/// This trait is used during arena serialization to serialize each value
/// in the heap. The serialization is done via the vtable, allowing for
/// dynamic dispatch during heap traversal.
pub trait StarlarkSerialize {
    /// Serialize this value using the provided context.
    ///
    /// The context provides access to the underlying pagable serializer.
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()>;
}

/// Context for serialization - wraps PagableSerializer.
///
/// This trait provides the interface that StarlarkSerialize implementations
/// use to serialize their data. It abstracts over the underlying serialization
/// mechanism.
pub trait StarlarkSerializeContext {
    /// Get mutable access to the underlying pagable serializer.
    fn pagable(&mut self) -> &mut dyn PagableSerializer;

    /// Serialize a reference to `v`.
    ///
    /// Only frozen heaps are serialized, so `v` is frozen; an unfrozen value is an internal error.
    fn serialize_value(&mut self, v: Value<'_>) -> crate::Result<()>;

    /// Capture the Starlark ownership context for serialization delegated to pagable code.
    #[doc(hidden)]
    fn serialization_scope(&self) -> StarlarkSerializeScope {
        StarlarkSerializeScope::rootless()
    }
}
