/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use proc_macro2::Span;
use quote::quote;
use syn::DeriveInput;
use syn::GenericParam;
use syn::Lifetime;
use syn::LifetimeParam;

pub(crate) fn derive_pagable_panic(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    match derive_pagable_panic_impl(input.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn derive_pagable_panic_impl(
    input: proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    let name = &input.ident;
    let (ser_impl_generics, type_generics, where_clause) = input.generics.split_for_impl();

    let mut generics_for_de = input.generics.clone();
    generics_for_de
        .params
        .push(GenericParam::Lifetime(LifetimeParam::new(Lifetime::new(
            "'de",
            Span::call_site(),
        ))));

    let (de_impl_generics, _, _) = generics_for_de.split_for_impl();

    let serialize_body = quote! {
        #[allow(unused)]
        impl #ser_impl_generics pagable::PagableSerialize for #name #type_generics #where_clause {
                fn pagable_serialize(&self, serializer: &mut dyn pagable::PagableSerializer) -> pagable::__internal::anyhow::Result<()> {
                    unimplemented!()
                }
        }
    };

    let deserialize_body = quote! {
        #[allow(unused)]
        impl #de_impl_generics pagable::PagableDeserialize<'de> for #name #type_generics #where_clause {
            fn pagable_deserialize<De: pagable::PagableDeserializer<'de> + ?Sized>(deserializer: &mut De) -> pagable::Result<Self> {
                unimplemented!()
            }
        }
    };

    Ok(quote! {
        #serialize_body
        #deserialize_body
    })
}
