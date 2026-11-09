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

pub(crate) fn derive_starlark_type_repr(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    match derive_starlark_type_repr_impl(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// `StarlarkTypeRepr` can be derived only for enums or structs with single field variants.
pub(crate) struct StarlarkTypeReprInput {
    pub(crate) ident: syn::Ident,
    pub(crate) generics: syn::Generics,
    pub(crate) enum_variants: Vec<(Option<syn::Ident>, syn::Type)>,
}

impl StarlarkTypeReprInput {
    pub(crate) fn parse(
        input: syn::DeriveInput,
        trait_name: &'static str,
    ) -> syn::Result<StarlarkTypeReprInput> {
        let variants = match input.data {
            syn::Data::Enum(data) => data
                .variants
                .into_iter()
                .map(|variant| (Some(variant.ident), variant.fields))
                .collect(),
            syn::Data::Struct(data) => {
                vec![(None, data.fields)]
            }
            syn::Data::Union(_) => {
                return Err(syn::Error::new_spanned(
                    input,
                    format!("`{trait_name}` cannot be derived for unions"),
                ));
            }
        };
        let enum_variants = variants
            .into_iter()
            .map(|(ident, fields)| {
                let mk_error = || {
                    syn::Error::new_spanned(
                        &fields,
                        format!(
                            "`{}` can be derived only for enums with single tuple field variants",
                            trait_name
                        ),
                    )
                };
                match &fields {
                    syn::Fields::Unnamed(fields) => {
                        match fields.unnamed.iter().collect::<Vec<_>>().as_slice() {
                            [field] => Ok((ident, field.ty.clone())),
                            _ => Err(mk_error()),
                        }
                    }
                    _ => Err(mk_error()),
                }
            })
            .collect::<syn::Result<Vec<_>>>()?;
        Ok(StarlarkTypeReprInput {
            ident: input.ident,
            generics: input.generics,
            enum_variants,
        })
    }
}

pub(crate) fn union_type(input: &StarlarkTypeReprInput) -> syn::Result<syn::Type> {
    fn recurse(
        (_, first): &(Option<syn::Ident>, syn::Type),
        rest: &[(Option<syn::Ident>, syn::Type)],
    ) -> syn::Result<syn::Type> {
        match rest.split_first() {
            None => Ok(first.clone()),
            Some((next, rest)) => {
                let next = recurse(next, rest)?;
                Ok(syn::parse_quote_spanned! { first.span() => either::Either<#first, #next> })
            }
        }
    }

    match input.enum_variants.split_first() {
        None => Ok(
            syn::parse_quote_spanned! { input.ident.span() => starlark::values::typing::StarlarkNever },
        ),
        Some((first, rest)) => recurse(first, rest),
    }
}

fn derive_starlark_type_repr_impl(
    input: syn::DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    let span = input.ident.span();

    let input = StarlarkTypeReprInput::parse(input, "StarlarkTypeRepr")?;

    let ident = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    let helper_type = union_type(&input)?;

    let trait_impl: syn::ItemImpl = syn::parse_quote_spanned! { span =>
        impl #impl_generics starlark::values::type_repr::StarlarkTypeRepr for #ident #type_generics #where_clause {
            type Canonical = <#helper_type as starlark::values::type_repr::StarlarkTypeRepr>::Canonical;

            fn starlark_type_repr() -> starlark::typing::Ty {
                <#helper_type as starlark::values::type_repr::StarlarkTypeRepr>::starlark_type_repr()
            }
        }
    };
    Ok(quote::quote_spanned! { span => #trait_impl })
}
