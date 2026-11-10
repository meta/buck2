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

#![doc(hidden)]

//! __derive_refs allows us to reference other crates in starlark_derive without users needing to be
//!  aware of those dependencies. We make them public here and then can reference them like
//!  `starlark::__derive_refs::foo`.

pub mod serde {
    pub use serde::Serialize;
    pub use serde::Serializer;
    pub use serde::ser::Error;
}
pub use anyhow;
pub use inventory;
// `pagable` re-exports so macros can reach them via `$crate::__derive_refs`,
// without consumer crates needing a direct `pagable` dependency.
pub use pagable::StaticValue;
pub use pagable::static_str;
pub use pagable::static_value;
pub mod components;
pub mod invoke_macro_error;
pub mod param_spec;
pub mod parse_args;
pub mod representations;
pub mod sig;

// Re-exports for the methods_static!/globals_static! macros.
pub use representations::ViewOfRepresentations;

pub use crate::environment::GlobalsStatic;
pub use crate::environment::MethodsStatic;
// Re-export for const_frozen_string!, static_starlark_value!, and
// declare_starlark_value_as_type! macro registry.
pub use crate::pagable::StaticHeapEntry;
pub use crate::pagable::StaticValueEntry;
// Re-exports for error types.
pub use crate::pagable::error::PagableError;
// Re-exports for vtable registration macro.
pub use crate::pagable::vtable_register::VtableRegistered;
#[cfg(feature = "pagable")]
pub use crate::pagable::vtable_registry::DeserTypeId;
#[cfg(feature = "pagable")]
pub use crate::pagable::vtable_registry::VTableRegistryEntry;
// Re-exports for TyStarlarkValue registration macro.
pub use crate::typing::starlark_value::HasTyVTable;
pub use crate::typing::starlark_value::TyStarlarkValueVTable;
pub use crate::typing::starlark_value::TyStarlarkValueVTableGet;
pub use crate::typing::starlark_value::TyStarlarkValueVTableStaticEntry;
pub use crate::values::layout::avalues::array::AValueAnyArray;
pub use crate::values::layout::avalues::simple::AValueFrozen;
pub use crate::values::layout::vtable::AValueVTable;
pub use crate::values::types::none::NoneType;
// Re-export for declare_starlark_value_as_type! and #[starlark_types] proc macro output.
pub use crate::values::types::starlark_value_as_type::StarlarkValueAsType;
// Re-export for TypeMatcher pagable typetag registration macro.
pub use crate::values::typing::type_compiled::matcher::TypeMatcherDyn;
