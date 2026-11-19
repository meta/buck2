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

//! Derive macro generating `StarlarkSerialize` / `StarlarkDeserialize` impls
//! that bridge to the type's existing `pagable::PagableSerialize` /
//! `pagable::PagableDeserialize` impls — use on types that are `Pagable` but
//! don't reference any Starlark `Value`.

use quote::quote_spanned;
use syn::DeriveInput;
use syn::spanned::Spanned;

use crate::pagable_brand::deserialize_brand;

pub fn derive_starlark_pagable_via_pagable(
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    match derive_impl(input.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn derive_impl(input: proc_macro2::TokenStream) -> syn::Result<proc_macro2::TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    let (brand, de_generics) = deserialize_brand(&input)?;
    let (de_impl_generics, _, _) = de_generics.split_for_impl();

    Ok(quote_spanned! { input.span() =>
        impl #impl_generics starlark::pagable::StarlarkSerialize for #name #type_generics #where_clause {
            fn starlark_serialize(
                &self,
                ctx: &mut dyn starlark::pagable::StarlarkSerializeContext,
            ) -> starlark::Result<()> {
                pagable::PagableSerialize::pagable_serialize(self, ctx.pagable())?;
                Ok(())
            }
        }

        impl #de_impl_generics starlark::pagable::StarlarkDeserialize<#brand> for #name #type_generics #where_clause {
            fn starlark_deserialize(
                ctx: &mut dyn starlark::pagable::StarlarkDeserializeContext<'_, #brand>,
            ) -> starlark::Result<Self> {
                Ok(<Self as pagable::PagableDeserialize>::pagable_deserialize(ctx.pagable())?)
            }
        }
    })
}
