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

use syn::spanned::Spanned;

use crate::starlark_type_repr::StarlarkTypeReprInput;
use crate::v_lifetime::find_v_lifetime;

pub(crate) fn derive_unpack_value(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    match derive_unpack_value_impl(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// A union of its variants' type expressions: `StarlarkTypeExpr` (the enum is its own `Get`),
/// `StarlarkTypeExprStable` when every variant's is, and the infallible `UnpackValue` that
/// binds a parameter of the union type.
fn derive_unpack_value_impl(input: syn::DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let span = input.ident.span();

    let input = StarlarkTypeReprInput::parse(input, "UnpackValue")?;

    let ident = input.ident;
    let lifetime = find_v_lifetime(&input.generics)?;

    let (_impl_generics, type_generics, where_clause) = input.generics.split_for_impl();

    let mut generics = input.generics.clone();
    if lifetime.is_none() {
        generics
            .params
            .push(syn::parse_quote_spanned! { span => 'v });
    }

    let (impl_generics, _type_generics, _where_clause) = generics.split_for_impl();

    let branches: Vec<syn::ExprIf> = input
        .enum_variants
        .iter()
        .map(|(n, t)| {
            let variant = match n {
                Some(variant) => quote::quote! {::#variant},
                None => quote::quote! {},
            };
            syn::parse_quote_spanned! { t.span() =>
                if let Some(x) = <#t as starlark::values::type_expr::UnionPayload<'v>>::from_value(value) {
                    return std::option::Option::Some(#ident #variant(x));
                }
            }
        })
        .collect::<Vec<_>>();

    let variant_types: Vec<&syn::Type> = input.enum_variants.iter().map(|(_, t)| t).collect();

    let expr_impl: syn::ItemImpl = syn::parse_quote_spanned! { span =>
        impl #impl_generics starlark::values::StarlarkTypeExpr<'v> for #ident #type_generics #where_clause {
            type Get = Self;

            fn get(value: starlark::values::Value<'v>) -> std::option::Option<Self> {
                #(#branches)*
                let _unused_when_enum_is_empty = value;
                std::option::Option::None
            }
        }
    };
    // Stable iff every variant is; the bounds decide per instantiation.
    let stable_impl: syn::ItemImpl = syn::parse_quote_spanned! { span =>
        impl #impl_generics starlark::values::StarlarkTypeExprStable<'v> for #ident #type_generics
        where
            #(<#variant_types as starlark::values::type_expr::UnionPayload<'v>>::Expr: starlark::values::StarlarkTypeExprStable<'v>,)*
        {
        }
    };
    // A union is its own handle, so it is a payload of an enclosing union as itself.
    let payload_impl: syn::ItemImpl = syn::parse_quote_spanned! { span =>
        impl #impl_generics starlark::values::type_expr::UnionPayload<'v> for #ident #type_generics #where_clause {
            type Expr = Self;

            #[inline]
            fn from_value(value: starlark::values::Value<'v>) -> std::option::Option<Self> {
                <Self as starlark::values::StarlarkTypeExpr<'v>>::get(value)
            }
        }
    };
    let trait_impl: syn::ItemImpl = syn::parse_quote_spanned! { span =>
        impl #impl_generics starlark::values::UnpackValue<'v> for #ident #type_generics #where_clause {
            type Type = Self;
            type Error = std::convert::Infallible;

            #[inline]
            fn unpack(
                _value: starlark::values::ValueOf<'v, Self>,
                get: Self,
            ) -> std::result::Result<Self, Self::Error> {
                std::result::Result::Ok(get)
            }
        }
    };
    Ok(quote::quote_spanned! { span => #expr_impl #stable_impl #payload_impl #trait_impl })
}
