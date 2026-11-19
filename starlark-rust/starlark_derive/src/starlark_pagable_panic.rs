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

//! Derive macro generating panicking [`StarlarkSerialize`] and [`StarlarkDeserialize`]
//! impls — a Starlark analog of `pagable::PagablePanic`.
//!
//! Use on types that must satisfy a `StarlarkSerialize + StarlarkDeserialize` trait bound but
//! should never actually be serialized. Any accidental call will `unimplemented!()`.

use quote::quote_spanned;
use syn::DeriveInput;
use syn::spanned::Spanned;

use crate::pagable_brand::deserialize_brand;

pub fn derive_starlark_pagable_panic(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    match derive_starlark_pagable_panic_impl(input.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn derive_starlark_pagable_panic_impl(
    input: proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    let (brand, de_generics) = deserialize_brand(&input)?;
    let (de_impl_generics, _, _) = de_generics.split_for_impl();

    Ok(quote_spanned! { input.span() =>
        #[allow(unused)]
        impl #impl_generics starlark::pagable::StarlarkSerialize for #name #type_generics #where_clause {
            fn starlark_serialize(
                &self,
                _ctx: &mut dyn starlark::pagable::StarlarkSerializeContext,
            ) -> starlark::Result<()> {
                panic!(
                    "StarlarkSerialize is not supported for `{}`",
                    ::std::any::type_name::<Self>()
                )
            }
        }

        #[allow(unused)]
        impl #de_impl_generics starlark::pagable::StarlarkDeserialize<#brand> for #name #type_generics #where_clause {
            fn starlark_deserialize(
                _ctx: &mut dyn starlark::pagable::StarlarkDeserializeContext<'_, #brand>,
            ) -> starlark::Result<Self> {
                panic!(
                    "StarlarkDeserialize is not supported for `{}`",
                    ::std::any::type_name::<Self>()
                )
            }
        }
    })
}
