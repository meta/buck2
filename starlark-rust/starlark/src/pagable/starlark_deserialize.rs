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

//! Trait for Starlark values that can be deserialized from pagable format.

use pagable::PagableDeserializer;

use crate::any::IsStaticType;
use crate::any::ProvidesStaticType;
use crate::values::Value;
use crate::values::deferred::DeferredReadContext;

/// Trait for Starlark values that can be deserialized at the brand `'fv`.
///
/// This trait is used during arena deserialization to reconstruct each value
/// in the heap. The deserialization is done via the vtable, allowing for
/// dynamic dispatch during heap reconstruction.
///
/// `'fv` is the brand of the heap the value is being deserialized into (see the `branding`
/// module), and the [`Value`]s the context hands out are `Value<'fv>`. A type that holds starlark
/// values therefore implements the trait at its own brand, `impl<'v> StarlarkDeserialize<'v> for
/// Foo<'v>`, and a type that holds none implements it at every brand, `impl<'fv>
/// StarlarkDeserialize<'fv> for Bar`; `#[derive(StarlarkPagable)]` picks the shape from the type's
/// lifetime parameters.
pub trait StarlarkDeserialize<'fv>: Sized {
    /// Deserialize this value using the provided context.
    ///
    /// The context provides access to the underlying pagable deserializer.
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self>;
}

/// Deserialize a single field of `T`, attaching the field name to any error
/// returned. Used by `#[derive(StarlarkPagable)]` to give blameable error
/// messages — `"deserializing field foo"` is much more useful than the raw
/// inner error alone.
pub fn starlark_deserialize_field<'fv, T: StarlarkDeserialize<'fv>>(
    ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    field: &'static str,
) -> crate::Result<T> {
    T::starlark_deserialize(ctx)
        .map_err(|e| e.with_context(format!("deserializing field {}", field)))
}

/// Context for deserialization - wraps PagableDeserializer.
///
/// This trait provides the interface that StarlarkDeserialize implementations
/// use to deserialize their data. It abstracts over the underlying deserialization
/// mechanism.
///
/// `'de` is the lifetime of the pagable deserializer's input. `'fv` is the brand of the heap
/// whose value is being deserialized: every value the context hands out lives in that heap or
/// in a heap it references, and the brand is introduced by the framework for one value's
/// deserialization only, the way [`Heap::temp`](crate::values::Heap::temp) introduces a brand, so
/// a [`StarlarkDeserialize`] implementation cannot put a value anywhere the brand does not reach.
pub trait StarlarkDeserializeContext<'de, 'fv> {
    /// Get mutable access to the underlying pagable deserializer.
    fn pagable(&mut self) -> &mut dyn PagableDeserializer<'de>;

    /// Deserialize a value reference. During cyclic heap construction a reference
    /// may name an unfinished slot: store such edges without inspecting their
    /// targets. Owning results wait for all construction dependencies to succeed
    /// before publishing the resulting graph.
    fn deserialize_value(&mut self) -> crate::Result<Value<'fv>>;

    /// Capture this heap's context for a deferred field, when enabled.
    /// Contexts without an owning heap keep reading fields eagerly.
    #[doc(hidden)]
    fn deferred_read_context(&mut self) -> Option<DeferredReadContext<'fv>> {
        None
    }
}

/// `Self`, a type at some brand `'v`, is deserializable at the brand `'fv`.
///
/// That is, `Self` with `'fv` in place of `'v` (its [`Reinfect`](IsStaticType::Reinfect))
/// implements [`StarlarkDeserialize<'fv>`]. Implemented for every type whose static-type impls
/// say so; it exists so that the heap deserializer, which names a value type through the vtable
/// of one instantiation, can ask for the type at the brand it is deserializing at.
pub trait StarlarkDeserializeAt<'v, 'fv>: ProvidesStaticType<'v> {
    /// `Self` at `'fv`.
    type Reinfected: StarlarkDeserialize<'fv>;
}

impl<'v, 'fv, T> StarlarkDeserializeAt<'v, 'fv> for T
where
    T: ProvidesStaticType<'v>,
    T::StaticType: IsStaticType,
    <T::StaticType as IsStaticType>::Reinfect<'fv>: StarlarkDeserialize<'fv>,
{
    type Reinfected = <T::StaticType as IsStaticType>::Reinfect<'fv>;
}
