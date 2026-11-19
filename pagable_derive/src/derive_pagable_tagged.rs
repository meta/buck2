/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use proc_macro::TokenStream;
use quote::quote;
use syn::DeriveInput;
use syn::parse_macro_input;

/// `#[pagable_tagged(TraitName)]` — generates a `PagableTypeTag` impl that
/// requires `Self: PagableRegisteredFor<dyn TraitName>`, where `Self` is the
/// full wrapper type (e.g. `Wrapper<F>`).
pub fn pagable_tagged_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let trait_path =
        syn::parse::<syn::Path>(attr).expect("pagable_tagged requires a trait path argument");
    let input = parse_macro_input!(item as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();

    let mut generics = input.generics.clone();
    if input.generics.type_params().next().is_some() {
        generics.make_where_clause().predicates.push(
            syn::parse_quote! { Self: pagable::typetag::PagableRegisteredFor<dyn #trait_path> },
        );
    }
    generics
        .make_where_clause()
        .predicates
        .push(syn::parse_quote! { Self: pagable::typetag::PagableStableName });
    let where_clause = &generics.where_clause;

    let expanded = quote! {
        #input

        impl #impl_generics pagable::typetag::PagableTypeTag for #name #ty_generics #where_clause {
            fn pagable_type_tag_static() -> &'static str {
                <Self as pagable::typetag::PagableStableName>::pagable_stable_name()
            }
        }
    };

    expanded.into()
}
