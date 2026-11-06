/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#[allow(unused_extern_crates)] // proc_macro is very special
extern crate proc_macro;

mod provider;

/// Generates the starlark implementation for an internal provider.
///
/// Implements `StarlarkValue` and `ProviderLike` for the provider type and
/// adds a `ProviderCallable` that's used in starlark as the constructor.
///
/// The struct must have one lifetime parameter, `ValueOf<'v, _>`/`ValueTyped<'v, _>`/`StringValue<'v>`
/// fields (any of them in an `Option`) and `#[derive(Freeze)]`; its frozen form is the `'static`
/// alias.
///
/// # Arguments
///
/// - `creator_func` (required): The name of the function to create the provider
/// - `methods = <func>` (optional): Custom methods function name. If not provided,
///   attribute accessors for all fields are auto-generated.
///
/// # Field Types
///
/// Return types can be specified directly by adding `#[provider(field_type=SomeIdentifier)]`
/// to a field. The type `SomeIdentifier` must be one that implements
/// `starlark::values::type_repr::StarlarkTypeRepr`
///
/// # Behavior
///
/// In starlark, a provider instance will have starlark-accessible attributes for
/// each of its fields by default. Field attribute accessors are auto-generated unless
/// you provide a custom `methods` function (see Arguments section).
///
/// In rust, a utility is added for getting the provider from a provider collection like
/// Foo::from_providers(collection).
///
/// To run validation at freeze time, put `#[freeze(validator = my_validate)]`
/// on the struct; that is an option of the `Freeze` derive, not of this
/// attribute.
///
/// # Examples
///
/// ## Auto-generate attribute accessors (default)
///
/// ```ignore
/// #[internal_provider(my_provider_creator)]
/// #[derive(Clone, Debug, Trace, Freeze, ProvidesStaticType, Allocative, StarlarkPagable)]
/// #[repr(C)]
/// struct MyProvider<'v> {
///     field1: StringValue<'v>,
/// }
/// ```
///
/// In Starlark: `provider.field1` (attribute access auto-generated)
///
/// ## Use custom methods
///
/// `methods = some_module` replaces the generated attribute lookup with a
/// `#[starlark_module]` of methods, which must then define the attributes itself:
///
/// ```ignore
/// #[internal_provider(my_provider_creator, methods = my_custom_methods)]
/// ```
///
///
/// In Starlark: `provider.field1` (custom attribute) and `provider.my_method("hello world")` (custom method)
#[proc_macro_attribute]
pub fn internal_provider(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input);
    let attr = syn::parse_macro_input!(attr);
    match provider::define_provider(attr, input) {
        Ok(tokens) => tokens,
        Err(e) => e.to_compile_error().into(),
    }
}
