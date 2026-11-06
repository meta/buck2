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

//! A type [`StarlarkAny`] which can cheaply wrap any Rust value into a [`Value`].
//!
//! This is intended to be a low cost way to quickly wrap Rust types without much boilerplate.
//! For more advanced uses you should define an instance of [`StarlarkValue`].
//!
//! To use this type, usually you will return a [`StarlarkAny`] from a module function,
//! and consume it in another. As an example, we can cheaply wrap the
//! [`Duration`](std::time::Duration) type.
//!
//! ```
//! #[macro_use]
//! extern crate starlark;
//! # fn main() {
//! use std::fmt;
//! use std::time::Instant;
//!
//! use starlark::assert::Assert;
//! use starlark::environment::GlobalsBuilder;
//! use starlark::values::StarlarkPagablePanic;
//! use starlark::values::Value;
//! use starlark::values::any::StarlarkAny;
//!
//! #[derive(Debug, StarlarkPagablePanic)]
//! struct MyInstant(Instant);
//!
//! starlark::register_starlark_any!(MyInstant);
//!
//! #[starlark_module]
//! fn globals(builder: &mut GlobalsBuilder) {
//!     fn start() -> anyhow::Result<StarlarkAny<MyInstant>> {
//!         Ok(StarlarkAny::new(MyInstant(Instant::now())))
//!     }
//!
//!     fn elapsed(x: Value) -> anyhow::Result<String> {
//!         Ok(StarlarkAny::<MyInstant>::get(x)
//!             .unwrap()
//!             .0
//!             .elapsed()
//!             .as_secs_f64()
//!             .to_string())
//!     }
//! }
//!
//! let mut a = Assert::new();
//! a.globals_add(globals);
//! a.pass(
//!     r#"
//! instant = start()
//! print(elapsed(instant))
//! "#,
//! );
//! # }
//! ```

use std::fmt;
use std::fmt::Debug;
use std::ops::Deref;

use allocative::Allocative;
use starlark_derive::NoSerialize;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::pagable::vtable_register::VtableRegistered;
use crate::typing::starlark_value::TyStarlarkValueVTable;
use crate::values::AllocValue;
use crate::values::Heap;
use crate::values::StarlarkValue;
use crate::values::Value;

/// A type that can be passed around as a Starlark [`Value`], but in most
/// ways is uninteresting/opaque to Starlark. Constructed with
/// [`new`](StarlarkAny::new) and decomposed with [`get`](StarlarkAny::get).
///
/// This is version for "simple" values (not requiring trace during GC).
/// For "complex" version check
/// [`StarlarkAnyComplex`](crate::values::types::any_complex::StarlarkAnyComplex).
#[derive(ProvidesStaticType, NoSerialize, Allocative, derive_more::Display)]
#[allocative(bound = "")]
#[display("{:?}", self)]
#[repr(transparent)]
pub struct StarlarkAny<T: Debug + Send + Sync + 'static>(
    #[allocative(skip)] // TODO(nga): do not skip.
    pub  T,
);

impl<T: StarlarkAnyRegistered> crate::pagable::StarlarkSerialize for StarlarkAny<T> {
    fn starlark_serialize(
        &self,
        ctx: &mut dyn crate::pagable::StarlarkSerializeContext,
    ) -> crate::Result<()> {
        <T as crate::pagable::StarlarkSerialize>::starlark_serialize(&self.0, ctx)
    }
}

impl<'fv, T: StarlarkAnyRegistered> crate::pagable::StarlarkDeserialize<'fv> for StarlarkAny<T> {
    fn starlark_deserialize(
        ctx: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        Ok(StarlarkAny(<T as crate::pagable::StarlarkDeserialize<
            'fv,
        >>::starlark_deserialize(ctx)?))
    }
}

/// Marker trait certifying that `T` has been registered for both the
/// heap vtable (`StarlarkAny<T>` usable as a `Value`) and the typing
/// vtable (`HasTyVTable` for `StarlarkAny<T>`).
///
/// This indirection exists to work around the orphan rule: downstream
/// crates can't `impl HasTyVTable for StarlarkAny<LocalT>` directly (both
/// trait and outer type are foreign), but they can impl this marker on
/// their local `T`.
///
/// # Safety
///
/// Implementors must also register the heap vtable entry for
/// `StarlarkAny<Self>` via
/// [`register_simple_vtable_entry!`](macro@crate::register_simple_vtable_entry).
/// Use the [`register_starlark_any!`](macro@crate::register_starlark_any) macro
/// instead of implementing this trait manually — it handles both the trait
/// impl and the vtable registration.
pub unsafe trait StarlarkAnyRegistered:
    Debug + Send + Sync + 'static + for<'fv> crate::pagable::StarlarkPagable<'fv>
{
    /// Typing vtable entry for `StarlarkAny<Self>`.
    const TY_VTABLE_STATIC: pagable::StaticValue<TyStarlarkValueVTable>;
}

#[cfg(feature = "pagable")]
impl<T> crate::typing::HasTyVTable for StarlarkAny<T>
where
    T: StarlarkAnyRegistered,
{
    const TY_VTABLE_STATIC: pagable::StaticValue<TyStarlarkValueVTable> =
        <T as StarlarkAnyRegistered>::TY_VTABLE_STATIC;
}

#[starlark_value(type = "any")]
impl<'v, T: StarlarkAnyRegistered> StarlarkValue<'v> for StarlarkAny<T> {}

/// Register a type for use with [`StarlarkAny`].
///
/// Call once per concrete `T` that is wrapped in `StarlarkAny`. This macro:
/// 1. Implements [`StarlarkAnyRegistered`] for `T`, providing the typing
///    vtable entry for `StarlarkAny<T>`.
/// 2. Registers the heap vtable entry for `StarlarkAny<T>` via
///    [`register_simple_vtable_entry!`](macro@crate::register_simple_vtable_entry).
#[macro_export]
macro_rules! register_starlark_any {
    ($t:ty) => {
        const _: () = {
            $crate::__declare_ty_vtable_static!($crate::values::any::StarlarkAny<$t>);
            // SAFETY: the heap vtable entry is registered below via
            // `register_simple_vtable_entry!`.
            unsafe impl $crate::values::any::StarlarkAnyRegistered for $t {
                const TY_VTABLE_STATIC: pagable::StaticValue<
                    $crate::__derive_refs::TyStarlarkValueVTable,
                > = VTABLE_STATIC;
            }
        };

        $crate::register_simple_vtable_entry!($crate::values::any::StarlarkAny<$t>);
    };
}

impl<'v, T: StarlarkAnyRegistered> AllocValue<'v> for StarlarkAny<T> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_simple(self)
    }
}

impl<T: Debug + Send + Sync + 'static> Debug for StarlarkAny<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.0, f)
    }
}

impl<T: Debug + Send + Sync + 'static> Deref for StarlarkAny<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Debug + Send + Sync + 'static> StarlarkAny<T> {
    /// Create a new [`StarlarkAny`] value. Such a value can be allocated on a heap with
    /// `heap.alloc(StarlarkAny::new(x))`.
    pub const fn new(x: T) -> Self {
        StarlarkAny(x)
    }
}

impl<T: StarlarkAnyRegistered> StarlarkAny<T> {
    /// Extract from a [`Value`] that contains a [`StarlarkAny`] underneath. Returns [`None`] if
    /// the value does not match the expected type.
    pub fn get<'v>(x: Value<'v>) -> Option<&'v T> {
        let x: &StarlarkAny<T> = x.downcast_ref()?;
        Some(&x.0)
    }
}

/// Trait alias that captures the bounds required for types wrapped in `StarlarkAny`.
///
/// When the `pagable` feature is enabled, this requires `StarlarkAnyRegistered`
/// (the type must be registered via [`register_starlark_any!`]).
#[cfg(feature = "pagable")]
pub trait StarlarkAnyBound: StarlarkAnyRegistered {}

#[cfg(feature = "pagable")]
impl<T: StarlarkAnyRegistered> StarlarkAnyBound for T {}

/// Trait alias that captures the bounds required for types wrapped in `StarlarkAny`.
#[cfg(not(feature = "pagable"))]
pub trait StarlarkAnyBound: Debug + Send + Sync + 'static {}

#[cfg(not(feature = "pagable"))]
impl<T: Debug + Send + Sync + 'static> StarlarkAnyBound for T {}

#[cfg(feature = "pagable")]
unsafe impl<T: StarlarkAnyBound> VtableRegistered for StarlarkAny<T> {}

#[cfg(not(feature = "pagable"))]
unsafe impl<T: StarlarkAnyBound> VtableRegistered for StarlarkAny<T> {}

/// Declare a static `StarlarkAny<T>` value with automatic pagable registration.
///
/// This macro:
/// 1. Implements `StaticValueRegistered` for `StarlarkAny<T>` (first invocation for a type)
/// 2. Creates a `static AllocStaticSimple<StarlarkAny<T>>` variable
/// 3. Registers it with `inventory` for pagable serialization
///
/// Use `@no_impl` variant when `StaticValueRegistered` is already
/// implemented for `StarlarkAny<T>` (e.g., by a previous invocation for the same type).
///
/// # Syntax
///
/// ```ignore
/// // First static for this type
/// crate::static_starlark_any!(pub(crate) VALUE_EMPTY_CODEMAP: CodeMap = EMPTY_NATIVE_CODEMAP.to_codemap());
///
/// // Additional statics for an already-registered type
/// crate::static_starlark_any!(@no_impl ANOTHER_CODEMAP: CodeMap = other_codemap());
/// ```
#[macro_export]
macro_rules! static_starlark_any {
    ($vis:vis $name:ident : $T:ty = $value:expr) => {
        // SAFETY: This impl is generated by static_starlark_any! alongside the static
        // registration, ensuring the type is properly registered for pagable serialization.
        unsafe impl $crate::pagable::static_value::StaticValueRegistered
            for $crate::values::types::any::StarlarkAny<$T> {}

        $crate::static_starlark_any!(@no_impl $vis $name : $T = $value);
    };

    (@no_impl $vis:vis $name:ident : $T:ty = $value:expr) => {
        $vis static $name: $crate::values::AllocStaticSimple<
            $crate::values::types::any::StarlarkAny<$T>,
        > = $crate::values::AllocStaticSimple::alloc(
            $crate::values::types::any::StarlarkAny::new($value),
        );

        $crate::__derive_refs::inventory::submit! {
            $crate::__derive_refs::StaticValueEntry::new(
                file!(),
                line!(),
                || $name.unpack().to_value()
            )
        }
    };
}

// Note: `register_starlark_any!(T)` for concrete starlark-internal types lives
// alongside each type's definition (e.g. `DefInfo` in `eval/compiler/def.rs`,
// `LocalSlotId` in `eval/runtime/slots.rs`). External users register their own
// `T` the same way.
#[cfg(test)]
crate::register_starlark_any!(String);
