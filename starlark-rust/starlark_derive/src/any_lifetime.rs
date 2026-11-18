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

use proc_macro2::Span;
use quote::quote;
use syn::DeriveInput;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;

use crate::util::DeriveInputUtil;
use crate::util::GenericsUtil;

fn punctuated_try_map<A, B, P: Clone>(
    punctuated: &Punctuated<A, P>,
    f: impl Fn(&A) -> syn::Result<B>,
) -> syn::Result<Punctuated<B, P>> {
    let mut new_punctuated = Punctuated::new();
    for pair in punctuated.pairs() {
        new_punctuated.push_value(f(pair.value())?);
        if let Some(p) = pair.punct() {
            new_punctuated.push_punct((*p).clone());
        }
    }
    Ok(new_punctuated)
}

fn generic_argument_replace_lifetimes(
    generic_argument: &syn::GenericArgument,
    replacement: &syn::GenericArgument,
) -> syn::Result<syn::GenericArgument> {
    match generic_argument {
        syn::GenericArgument::Lifetime(_) => Ok(replacement.clone()),
        a @ syn::GenericArgument::Const(..) => Ok(a.clone()),
        c => Err(syn::Error::new_spanned(c, "unsupported generic argument")),
    }
}

fn path_arguments_replace_lifetimes(
    path_arguments: &syn::PathArguments,
    replacement: &syn::GenericArgument,
) -> syn::Result<syn::PathArguments> {
    Ok(match path_arguments {
        syn::PathArguments::None => syn::PathArguments::None,
        syn::PathArguments::AngleBracketed(syn::AngleBracketedGenericArguments {
            colon2_token,
            lt_token,
            args,
            gt_token,
        }) => syn::PathArguments::AngleBracketed(syn::AngleBracketedGenericArguments {
            colon2_token: *colon2_token,
            lt_token: *lt_token,
            args: punctuated_try_map(args, |arg| {
                generic_argument_replace_lifetimes(arg, replacement)
            })?,
            gt_token: *gt_token,
        }),
        syn::PathArguments::Parenthesized(_) => {
            return Err(syn::Error::new_spanned(
                path_arguments,
                "unsupported path arguments",
            ));
        }
    })
}

fn path_segment_replace_lifetimes(
    path_segment: &syn::PathSegment,
    replacement: &syn::GenericArgument,
) -> syn::Result<syn::PathSegment> {
    Ok(syn::PathSegment {
        ident: path_segment.ident.clone(),
        arguments: path_arguments_replace_lifetimes(&path_segment.arguments, replacement)?,
    })
}

fn path_replace_lifetimes(
    path: &syn::Path,
    replacement: &syn::GenericArgument,
) -> syn::Result<syn::Path> {
    Ok(syn::Path {
        leading_colon: path.leading_colon,
        segments: punctuated_try_map(&path.segments, |seg| {
            path_segment_replace_lifetimes(seg, replacement)
        })?,
    })
}

/// Replace `X<'a>` with `X<'static>`.
fn type_param_bound_replace_lifetimes_with_static(
    bound: &syn::TypeParamBound,
) -> syn::Result<syn::TypeParamBound> {
    type_param_bound_replace_lifetimes(bound, syn::parse_quote!('static))
}

/// Replace `X<'a>` with `X<'lt>`.
fn type_param_bound_replace_lifetimes(
    bound: &syn::TypeParamBound,
    new_lifetime: syn::Lifetime,
) -> syn::Result<syn::TypeParamBound> {
    match bound {
        syn::TypeParamBound::Lifetime(_) => Ok(syn::TypeParamBound::Lifetime(new_lifetime)),
        syn::TypeParamBound::Trait(trait_bound) => {
            if trait_bound.lifetimes.is_some() {
                return Err(syn::Error::new_spanned(
                    trait_bound,
                    "trait bounds with generic lifetimes are not supported",
                ));
            }
            Ok(syn::TypeParamBound::Trait(syn::TraitBound {
                paren_token: trait_bound.paren_token,
                lifetimes: trait_bound.lifetimes.clone(),
                modifiers: trait_bound.modifiers.clone(),
                maybe: trait_bound.maybe,
                path: path_replace_lifetimes(
                    &trait_bound.path,
                    &syn::GenericArgument::Lifetime(new_lifetime),
                )?,
            }))
        }
        _ => Ok(bound.clone()),
    }
}

pub(crate) fn derive_provides_static_type(
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    match derive_provides_static_type_impl(input) {
        Ok(r#gen) => r#gen.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Single lifetime parameter for `ProvidesStaticType`
fn pst_lifetime(generics: &syn::Generics) -> syn::Result<syn::Lifetime> {
    let generics = GenericsUtil::new(generics);
    let lifetime = generics
        .assert_at_most_one_lifetime_param()?
        .map(|p| p.lifetime.clone());
    Ok(match lifetime {
        Some(lifetime) => lifetime,
        None => syn::parse_quote_spanned! { Span::call_site() => 'pst },
    })
}

pub(crate) fn derive_provides_static_type_impl(
    input: proc_macro::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let input: DeriveInput = syn::parse(input)?;
    derive_provides_static_type_for(&input)
}

/// The `ProvidesStaticType` impl for a parsed input; `#[derive(StarlarkRepresentations)]`
/// emits it for a view, whose `StarlarkType` requires it.
pub(crate) fn derive_provides_static_type_for(
    input: &DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    let input = DeriveInputUtil::new(input)?;

    let name = &input.ident;
    let (_impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let lifetime = pst_lifetime(&input.generics)?;

    // For ProvidesStaticType
    let mut lifetimes: Vec<syn::Lifetime> = Vec::new();
    let mut static_lifetimes: Vec<syn::Lifetime> = Vec::new();
    let mut type_param_names = Vec::new();
    let mut type_param_bounds = Vec::new();
    let mut type_param_static_type_bounds = Vec::new();
    let mut static_type_params = Vec::new();
    let mut const_params = Vec::new();
    let mut const_param_names = Vec::new();

    // For IsStaticType
    let mut is_static_type_param_bounds = Vec::new();
    let mut is_static_type_where_clauses = Vec::new();
    let mut reinfect_type_params = Vec::new();
    let mut reinfect_lifetimes: Vec<syn::Lifetime> = Vec::new();

    for param in &input.generics.params {
        match param {
            syn::GenericParam::Lifetime(param) => {
                lifetimes.push(param.lifetime.clone());
                static_lifetimes.push(syn::parse_quote_spanned! { param.span() => 'static });
                reinfect_lifetimes.push(syn::parse_quote_spanned! { param.span() => '__lst });
            }
            syn::GenericParam::Type(param) => {
                let has_static_lifetime_bound = param.bounds.iter().any(|bound| {
                    if let syn::TypeParamBound::Lifetime(lifetime) = bound {
                        lifetime.ident == "static"
                    } else {
                        false
                    }
                });

                let param_name = &param.ident;
                type_param_names.push(param.ident.clone());

                let param_bounds_static = param
                    .bounds
                    .iter()
                    .map(type_param_bound_replace_lifetimes_with_static)
                    .collect::<syn::Result<Vec<_>>>()?;
                let reinfect_lt: syn::Lifetime = syn::parse_quote!('__lst);
                let param_bounds_reinfect = param
                    .bounds
                    .iter()
                    .map(|b| type_param_bound_replace_lifetimes(b, reinfect_lt.clone()))
                    .collect::<syn::Result<Vec<_>>>()?;

                if has_static_lifetime_bound {
                    // Type param has 'static bound - use as-is for both traits
                    let param_bounds = param.bounds.iter();
                    type_param_bounds.push(quote! {
                        #param_name : #(#param_bounds)+*
                    });
                    static_type_params.push(quote! { #param_name });

                    is_static_type_param_bounds.push(quote! {
                        #param_name : #(#param_bounds_static)+*
                    });
                    reinfect_type_params.push(quote! { #param_name });
                } else {
                    // Type param needs ProvidesStaticType/IsStaticType bounds
                    let param_bounds = param.bounds.iter();
                    type_param_bounds.push(quote! {
                        #param_name : #(#param_bounds+)* starlark::any::ProvidesStaticType<#lifetime> + Sized
                    });
                    type_param_static_type_bounds.push(quote! {
                        #param_name :: StaticType : #(#param_bounds_static+)* Sized
                    });
                    static_type_params.push(quote! { #param_name :: StaticType });

                    is_static_type_param_bounds.push(quote! {
                        #param_name : #(#param_bounds_static+)* starlark::any::IsStaticType
                    });
                    is_static_type_where_clauses.push(quote! {
                        for<'__lst> #param_name :: Reinfect<'__lst> : #(#param_bounds_reinfect+)* Sized
                    });
                    reinfect_type_params.push(quote! { #param_name :: Reinfect<'__lst> });
                }
            }
            syn::GenericParam::Const(params) => {
                const_params.push(params.clone());
                const_param_names.push(params.ident.clone());
            }
        }
    }

    Ok(if input.generics.lt_token.is_none() {
        quote! {
            unsafe impl<#lifetime> starlark::any::ProvidesStaticType<#lifetime> for #name #ty_generics #where_clause {
                type StaticType = #name #ty_generics;
            }

            impl starlark::any::IsStaticType for #name #ty_generics #where_clause {
                type Reinfect<'__lst> = #name #ty_generics;
            }
        }
    } else {
        quote! {
            unsafe impl <
                #lifetime,
                #(#type_param_bounds,)*
                #(#const_params,)*
                    > starlark::any::ProvidesStaticType<#lifetime>
            for #name <
                #(#lifetimes,)*
                #(#type_param_names,)*
                #(#const_param_names,)*
                    > #where_clause
            where
                #(#type_param_static_type_bounds,)*
            {
                type StaticType = #name <
                    #(#static_lifetimes,)*
                    #(#static_type_params,)*
                    #(#const_param_names,)*
                        >;
            }

            impl <
                #(#is_static_type_param_bounds,)*
                #(#const_params,)*
                    > starlark::any::IsStaticType
            for #name <
                #(#static_lifetimes,)*
                #(#type_param_names,)*
                #(#const_param_names,)*
                    > #where_clause
            where
                #(#is_static_type_where_clauses,)*
            {
                type Reinfect<'__lst> = #name <
                    #(#reinfect_lifetimes,)*
                    #(#reinfect_type_params,)*
                    #(#const_param_names,)*
                        >;
            }
        }
    })
}
