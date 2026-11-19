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

/// The variants of a union enum (or the one field of a struct), each a single unnamed field.
pub(crate) struct UnionInput {
    pub(crate) ident: syn::Ident,
    pub(crate) generics: syn::Generics,
    pub(crate) enum_variants: Vec<(Option<syn::Ident>, syn::Type)>,
}

impl UnionInput {
    pub(crate) fn parse(
        input: syn::DeriveInput,
        trait_name: &'static str,
    ) -> syn::Result<UnionInput> {
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
        Ok(UnionInput {
            ident: input.ident,
            generics: input.generics,
            enum_variants,
        })
    }
}

pub(crate) fn union_type(input: &UnionInput) -> syn::Result<syn::Type> {
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
