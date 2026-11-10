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

//! A proc-macro for writing functions in Rust that can be called from Starlark.

#[allow(unused_extern_crates)] // proc_macro is very special
extern crate proc_macro;

use proc_macro::TokenStream;

mod any_lifetime;
mod bc;
mod freeze;
mod module;
mod pagable_brand;
mod representations;
mod serde;
mod starlark_pagable;
mod starlark_pagable_panic;
mod starlark_pagable_unsupported;
mod starlark_pagable_via_pagable;
mod starlark_type_repr;
mod starlark_value;
mod trace;
mod type_matcher;
mod unpack_value;
mod util;
mod v_lifetime;
mod visit_span;
mod vtable;

/// Write Starlark modules concisely in Rust syntax.
///
/// For example:
///
/// ```ignore
/// #[starlark_module]
/// fn global(builder: &mut GlobalsBuilder) {
///     fn cc_binary(name: &str, srcs: Vec<&str>) -> String {
///         Ok(format!("{:?} {:?}", name, srcs))
///     }
/// }
/// ```
///
/// Parameters operate as named parameters of a given type. Each parameter will be unpacked with `UnpackValue`, unless:
///
/// * It is type `Option`, in which case it will be considered optional.
/// * It is a single argument of type `Arguments`, in which case all arguments will be passed together with minimal interpretation.
///
/// There are a number of attributes that can be add to each parameter by writing attributes before the
/// parameter name:
///
/// * `#[starlark(default = "a default")]` - provide a default for the parameter if it is omitted -- the value given here must
///    be the exact same Rust type as the Rust argument
/// * `#[starlark(require = pos)]` - require the parameter to be passed by position, not named.
/// * `#[starlark(require = named)]` - require the parameter to be passed by name, not by position.
/// * `#[starlark(args)]` - treat the argument as `*args` in Starlark, receiving all additional positional arguments as a tuple.
/// * `#[starlark(kwargs)]` - treat the argument as `**kwargs` in Starlark, receiving all additional named arguments as a dictionary.
/// * `#[starlark(doc_hidden)]` - omit the parameter from generated documentation while still accepting it.
///   Only allowed on named-only parameters.
///
/// There are a number of attributes that can be applied to the entire function by writing attributes
/// before the `fn` of the function:
///
/// * `#[starlark(attribute_type = "foo")]` - if the function has `.type` applied, return this string. Usually used on
///   constructor functions so that `ctor.type` can be used in Starlark code.
/// * `#[starlark(speculative_exec_safe)]` - the function
///   is considered safe to execute speculatively: the function should have
///   no global side effects, should not panic, and should finish in reasonable time.
///   The evaluator may invoke such functions early to generate more efficient code.
/// * `#[starlark(attribute)]` to turn the name into
///   an attribute on the value. Such a function must take exactly one argument, namely a value
///   of the type you have attached it to.
///
/// Multiple attributes can be specified either separately `#[starlark(require = named)] #[starlark(default = "")]` or
/// separated with a comman `#[starlark(require = named, default = "")]`.
///
/// A `const NAME: T = value;` item defines the global `NAME` as `value` converted through
/// `AllocFrozenValue`. With `#[starlark(representation)]` on the constant, `T` is a
/// representation of a Starlark type, put on the heap as it is through
/// `AllocFrozenRepresentation`.
///
/// There are two special arguments, distinguished by their type, which provides access to interpreter state:
///
/// * `heap: Heap<'v>` gives access to the Starlark heap, for allocating things.
/// * `eval: &mut Evaluator<'v, '_, '_>` gives access to the Starlark evaluator, which can be used to look at interpreter state.
///
/// A module can be used to define globals (with `GlobalsBuilder`) or methods on an object (with `MethodsBuilder`).
/// In the case of methods, the first argument to each function will be the object itself, typically named `this`.
///
/// All these functions interoperate properly with `dir()`, `getattr()` and `hasattr()`.
///
/// If a desired function name is also a Rust keyword, use the `r#` prefix, e.g. `r#type`.
///
/// As a more complex example:
///
/// ```ignore
/// #[starlark_module]
/// fn methods(builder: &mut MethodsBuilder) {
///     fn r#enum<'v>(
///         this: Value<'v>,
///         #[starlark(require = named, default = 3)] index: i32,
///         heap: Heap<'v>,
///     ) -> anyhow::Result<StringValue<'v>> {
///         Ok(heap.alloc_str(&format!("{this} {index}")))
///     }
/// }
/// ```
///
/// This defines a method such that when attached to an object `object.enum(index = 12)` will
/// return the string of the object and the index.
#[proc_macro_attribute]
pub fn starlark_module(attr: TokenStream, input: TokenStream) -> TokenStream {
    module::starlark_module(attr, input)
}

/// Stubs for Starlark bytecode interpreter.
#[proc_macro_attribute]
pub fn starlark_internal_bc(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    bc::starlark_internal_bc(attr, input)
}

#[proc_macro_attribute]
pub fn starlark_internal_vtable(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    vtable::starlark_internal_vtable(attr, input)
}

#[proc_macro_derive(VisitSpanMut)]
pub fn derive_visit_span_mut(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    visit_span::derive_visit_span_mut(input)
}

/// Derive the `Trace` trait.
#[proc_macro_derive(Trace, attributes(trace))]
pub fn derive_trace(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    trace::derive_trace(input)
}

/// Derive the `Freeze` trait: the brand is the type's lifetime parameter (a type without
/// one implements the trait at every brand, named `'v` in the impl), `Frozen<'fv>` is the type
/// with the lifetime parameter replaced by `'fv`, and `freeze` freezes each field. A type with
/// several lifetime parameters implements the trait by hand.
///
/// Options, as `#[freeze(..)]`:
/// - `identity` on a field: keep the field as it is, for a type that holds no values and
///   implements neither `Freeze` nor needs to.
/// - `validator = f` on the type: call `f(&frozen)` after freezing and fail the freeze with its
///   error.
/// - `bounds = "..."` on the type: extra `where` predicates for the impl; the brand is in scope
///   under its name.
///
/// A type that is only ever allocated into a frozen heap needs no `Freeze` impl: hold it through
/// a `FrozenValueTyped`, which re-types under freeze on its own.
#[proc_macro_derive(Freeze, attributes(freeze))]
pub fn derive_freeze(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    freeze::derive_freeze(input)
}

/// Derive the `NoSerialize` trait for serde.
#[proc_macro_derive(NoSerialize)]
pub fn derive_no_serialize(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    serde::derive_no_serialize(input)
}

/// Derive the `StarlarkTypeRepr` trait.
#[proc_macro_derive(StarlarkTypeRepr)]
pub fn derive_starlark_type_repr(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    starlark_type_repr::derive_starlark_type_repr(input)
}

/// Derive the `UnpackValue` trait.
#[proc_macro_derive(UnpackValue)]
pub fn derive_unpack_value(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    unpack_value::derive_unpack_value(input)
}

/// Derive the glue of a Starlark type's view over its representations: an enum with one
/// `ValueTyped<'v, Rep>` variant per representation. Generates `ProvidesStaticType`, `Display`,
/// `From` of each variant's `ValueTyped` (which `#[starlark_value]` builds a representation's
/// `with_type` on), `StarlarkTypeRepr` forwarding to the view's `StarlarkType`, and an
/// `UnpackValue` that tries the representations in order. The first variant's representation is the canonical one, whose
/// type id is the type's.
#[proc_macro_derive(StarlarkRepresentations)]
pub fn derive_starlark_representations(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    representations::derive_starlark_representations(input)
}

/// Generate missing elements of `StarlarkValue` trait when this attribute
/// is applied to an impl block of `StarlarkValue`.
///
/// `#[starlark_value(type = "name", ...)]` accepts these flags after the required `type`:
/// - `StarlarkTypeRepr`, `UnpackValue` (only together): implement those traits for `&T`.
/// - `skip_vtable`: register no vtable at all, neither the pagable `AValue` one nor the typing
///   one. For types whose frozen canonical form is what gets serialized.
/// - `no_freeze`: the type is never frozen. Register only the typing vtable, so it need not be
///   `StarlarkPagable`, and allocate it with `alloc_complex_no_freeze`.
/// - `frozen_vtable`: `T<'v>` is its own frozen form, freezing to `T<'static>`; register the
///   frozen `AValue` vtable too, which implements `VtableRegistered` for every `T<'v>`.
///   Requires `T<'static>: StarlarkPagable`. Without the flag a lifetime-parameterized `T`
///   gets only the typing vtable, because that shape is also what the unfrozen half of a
///   frozen/unfrozen pair looks like.
/// - `alloc = simple | complex | frozen | by_hand`: how the representation is allocated, which
///   writes its `AllocRepresentation` and `AllocFrozenRepresentation` impls. Without it the
///   shape decides as it does for the vtables: no lifetimes is `simple` (both heaps), lifetimes
///   is `complex` (the unfrozen heap; both with `frozen_vtable`), `frozen_vtable` alone is
///   `frozen` (the frozen heap). `by_hand` writes nothing, for a representation that is built
///   from a recipe rather than by value.
///
/// At most one of `skip_vtable`, `no_freeze`, `frozen_vtable` may be given, and at most one
/// of `no_freeze` and `alloc`.
#[proc_macro_attribute]
pub fn starlark_value(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    starlark_value::derive_starlark_value(attr, input)
}

/// Derive the `ProvidesStaticType` trait. Requires the type has no type arguments, no constant arguments,
/// and at most one lifetime argument.
#[proc_macro_derive(ProvidesStaticType)]
pub fn derive_provides_static_type(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    any_lifetime::derive_provides_static_type(input)
}

/// Derive both `StarlarkSerialize` and `StarlarkDeserialize` traits.
///
/// By default, each field is serialized/deserialized via the starlark context.
/// Fields annotated with `#[starlark_pagable(pagable)]` use the pagable bridge instead.
///
/// `StarlarkDeserialize` is implemented at the type's brand: its one lifetime parameter, or a
/// fresh `'fv` (every brand) for a type without lifetime parameters. A type with several
/// lifetime parameters implements the trait by hand.
#[proc_macro_derive(StarlarkPagable, attributes(starlark_pagable))]
pub fn derive_starlark_pagable(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    starlark_pagable::derive_starlark_pagable(input)
}

/// Derive panicking `StarlarkSerialize` and `StarlarkDeserialize` impls.
///
/// Use on types that must satisfy a `StarlarkSerialize + StarlarkDeserialize` bound but are
/// never actually round-tripped.
/// Any call to the generated methods triggers `unimplemented!()`.
///
/// The brand of the `StarlarkDeserialize` impl is chosen as for `StarlarkPagable`.
#[proc_macro_derive(StarlarkPagablePanic, attributes(starlark_pagable))]
pub fn derive_starlark_pagable_panic(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    starlark_pagable_panic::derive_starlark_pagable_panic(input)
}

/// Derive `StarlarkSerialize` and `StarlarkDeserialize` impls that fail with an error naming
/// the type.
///
/// Use on types that must satisfy a `StarlarkSerialize + StarlarkDeserialize` bound but cannot
/// be paged. Unlike `StarlarkPagablePanic`, reaching one during page-out fails that page-out
/// and leaves the heap resident, rather than aborting the process.
///
/// The brand of the `StarlarkDeserialize` impl is chosen as for `StarlarkPagable`.
#[proc_macro_derive(StarlarkPagableUnsupported, attributes(starlark_pagable))]
pub fn derive_starlark_pagable_unsupported(
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    starlark_pagable_unsupported::derive_starlark_pagable_unsupported(input)
}

/// Derive `StarlarkSerialize` / `StarlarkDeserialize` impls that bridge to the
/// type's `pagable::PagableSerialize` / `pagable::PagableDeserialize` impls.
///
/// Use on types that are `pagable::Pagable` and don't reference Starlark values.
/// The type must already implement `PagableSerialize` and `PagableDeserialize`
/// (typically via `#[derive(pagable::Pagable)]`).
///
/// The brand of the `StarlarkDeserialize` impl is chosen as for `StarlarkPagable`.
#[proc_macro_derive(StarlarkPagableViaPagable, attributes(starlark_pagable))]
pub fn derive_starlark_pagable_via_pagable(
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    starlark_pagable_via_pagable::derive_starlark_pagable_via_pagable(input)
}

/// Derive the `StarlarkSerialize` trait.
///
/// By default, each field is serialized via `StarlarkSerialize::starlark_serialize`.
/// Fields annotated with `#[starlark_pagable(pagable)]` use
/// `PagableSerialize::pagable_serialize(ctx.pagable())` instead.
#[proc_macro_derive(StarlarkSerialize, attributes(starlark_pagable))]
pub fn derive_starlark_serialize(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    starlark_pagable::derive_starlark_serialize(input)
}

/// Derive the `StarlarkDeserialize` trait.
///
/// By default, each field is deserialized via `StarlarkDeserialize::starlark_deserialize`.
/// Fields annotated with `#[starlark_pagable(pagable)]` use
/// `PagableDeserialize::pagable_deserialize(ctx.pagable())` instead.
///
/// The brand of the impl is chosen as for `StarlarkPagable`.
#[proc_macro_derive(StarlarkDeserialize, attributes(starlark_pagable))]
pub fn derive_starlark_deserialize(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    starlark_pagable::derive_starlark_deserialize(input)
}

/// Attribute macro for `impl TypeMatcher for X` blocks.
///
/// This macro generates:
/// 1. `impl TypeMatcherRegistered for X {}` - marks the type as registered
/// 2. For non-generic types: vtable registration via `register_avalue_simple_frozen!`
///
/// Example:
/// ```ignore
/// #[type_matcher]
/// impl TypeMatcher for IsAny {
///     fn matches(&self, _value: Value) -> bool { true }
/// }
/// ```
#[proc_macro_attribute]
pub fn type_matcher(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    type_matcher::derive_type_matcher(attr, input)
}
