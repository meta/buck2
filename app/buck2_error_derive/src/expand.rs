/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// This code is adapted from https://github.com/dtolnay/thiserror licensed under Apache-2.0 or MIT.

use std::collections::BTreeSet as Set;

use proc_macro2::Span;
use proc_macro2::TokenStream;
use quote::ToTokens;
use quote::format_ident;
use quote::quote;
use syn::DeriveInput;
use syn::Member;
use syn::Result;
use syn::Token;

use crate::ast::Enum;
use crate::ast::Field;
use crate::ast::Input;
use crate::ast::Struct;
use crate::attr::Attrs;
use crate::attr::OptionStyle;
use crate::attr::Trait;
use crate::generics::InferredBounds;

pub fn derive(node: &DeriveInput) -> Result<TokenStream> {
    let input = Input::from_syn(node)?;
    input.validate()?;
    Ok(match input {
        Input::Struct(input) => impl_struct(input),
        Input::Enum(input) => impl_enum(input),
    })
}

fn impl_struct(input: Struct) -> TokenStream {
    let ty = &input.ident;
    let arg_token = quote! { value };
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let mut error_inferred_bounds = InferredBounds::new();

    if input.generics.type_params().next().is_some() {
        let self_token = <Token![Self]>::default();
        error_inferred_bounds.insert(self_token, Trait::Debug);
        error_inferred_bounds.insert(self_token, Trait::Display);
        error_inferred_bounds.insert(self_token, quote!(std::marker::Send));
        error_inferred_bounds.insert(self_token, quote!(std::marker::Sync));
        error_inferred_bounds.insert(self_token, quote!('static));
    }

    let tags = get_tags(&input.attrs);
    let mut members = input.fields.iter().map(|field| &field.member).peekable();
    let field_pats: TokenStream = match members.peek() {
        Some(Member::Named(_)) => quote!( #(let #members = &#arg_token.#members);*; ),
        _ => quote!(),
    };

    let content = if input.attrs.transparent.is_some() {
        let member = &input.fields[0].member;
        quote! {
            #field_pats
            #tags

            // All errors called by transparent should have From implemented
            let error: buck2_error::Error = #arg_token.#member.into();
            error.tag(tags)
        }
    } else if let Some(source_field) = input.source_field() {
        let member = &source_field.member;

        quote! {
            #field_pats
            #tags
            let error_msg = format!("{}", #arg_token);

            // All errors called by source should have From implemented
            let error: buck2_error::Error = #arg_token.#member.into();
            let error = error.tag(tags);
            error.context(error_msg)
        }
    } else {
        let source_location_type_name = syn::LitStr::new(&ty.to_string(), Span::call_site());

        quote! {
            #field_pats
            #tags

            let source_location = buck2_error::source_location::SourceLocation::new(core::file!(), core::line!()).with_type_name(#source_location_type_name);
            let root_error = buck2_error::Error::new(format!("{}", #arg_token), tags[0], source_location, None);
            root_error.tag(tags)
        }
    };

    let pat = fields_pat(&input.fields);
    let mut display_implied_bounds = Set::new();
    let display_body = if input.attrs.transparent.is_some() {
        let only_field = &input.fields[0].member;
        display_implied_bounds.insert((0, Trait::Display));
        Some(quote! {
            std::fmt::Display::fmt(&self.#only_field, __formatter)
        })
    } else if let Some(display) = &input.attrs.display {
        display_implied_bounds = display.implied_bounds.clone();
        Some(quote! {
            #[allow(unused_assignments, unused_variables, deprecated)]
            let Self #pat = self;
            #display
        })
    } else {
        None
    };
    let display_impl = display_body.map(|body| {
        let mut display_inferred_bounds = InferredBounds::new();
        for (field, bound) in display_implied_bounds {
            let field = &input.fields[field];
            if field.contains_generic {
                display_inferred_bounds.insert(field.ty, bound);
            }
        }
        let display_where_clause = display_inferred_bounds.augment_where_clause(input.generics);
        quote! {
            #[allow(unused_qualifications)]
            impl #impl_generics std::fmt::Display for #ty #ty_generics #display_where_clause {
                fn fmt(&self, __formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                    #body
                }
            }
        }
    });

    let error_where_clause = error_inferred_bounds.augment_where_clause(input.generics);

    quote! {
        #[allow(unused_qualifications)]
        impl #impl_generics From<#ty #ty_generics> for buck2_error::Error #error_where_clause
        {
            #[cold]
            #[allow(unused_assignments, unused_variables, deprecated)]
            fn from(#arg_token: #ty #ty_generics) -> buck2_error::Error {
                #content
            }
        }

        #display_impl
    }
}

fn impl_enum(mut input: Enum) -> TokenStream {
    let ty = &input.ident;
    let arg_token: TokenStream = quote! { value };
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let mut display_inferred_bounds = InferredBounds::new();

    // We let people specify these on the type or variant, so make sure that they always show up on
    // the variant and we don't have to re-check the type
    for variant in &mut input.variants {
        variant.attrs.tags.extend(input.attrs.tags.iter().cloned());
    }

    let display_impl = if input.has_display() {
        let void_deref = if input.variants.is_empty() {
            Some(quote!(*))
        } else {
            None
        };
        let arms = input.variants.iter().map(|variant| {
            let mut display_implied_bounds = Set::new();
            let display = match &variant.attrs.display {
                Some(display) => {
                    display_implied_bounds = display.implied_bounds.clone();
                    display.to_token_stream()
                }
                None => {
                    let only_field = match &variant.fields[0].member {
                        Member::Named(ident) => ident.clone(),
                        Member::Unnamed(index) => format_ident!("_{}", index),
                    };
                    display_implied_bounds.insert((0, Trait::Display));
                    quote!(std::fmt::Display::fmt(#only_field, __formatter))
                }
            };
            for (field, bound) in display_implied_bounds {
                let field = &variant.fields[field];
                if field.contains_generic {
                    display_inferred_bounds.insert(field.ty, bound);
                }
            }
            let ident = &variant.ident;
            let pat = fields_pat(&variant.fields);
            quote! {
                #ty::#ident #pat => #display
            }
        });
        let arms = arms.collect::<Vec<_>>();
        let display_where_clause = display_inferred_bounds.augment_where_clause(input.generics);
        Some(quote! {
            #[allow(unused_qualifications)]
            impl #impl_generics std::fmt::Display for #ty #ty_generics #display_where_clause {
                fn fmt(&self, __formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                    #[allow(unused_assignments, unused_variables, deprecated, clippy::used_underscore_binding)]
                    match #void_deref self {
                        #(#arms,)*
                    }
                }
            }
        })
    } else {
        None
    };

    let from_impl_arms = input.variants.iter().map(|variant| {
        let tags = get_tags(&variant.attrs);

        let content = if variant.attrs.transparent.is_some() {
            let only_field = match &variant.fields[0].member {
                Member::Named(ident) => ident.clone(),
                Member::Unnamed(index) => format_ident!("_{}", index),
            };

            quote! {
                #tags

                // All errors called by transparent must have From implemented
                let error: buck2_error::Error = #only_field.into();
                error.tag(tags)
            }
        } else if let Some(source_field) = variant.source_field() {
            let member = match &source_field.member {
                Member::Named(ident) => ident.clone(),
                Member::Unnamed(index) => format_ident!("_{}", index),
            };

            quote! {
                #tags

                // All errors called by source must have From implemented
                let error = buck2_error::Error::from(#member);
                let error = error.tag(tags);

                error.context(err_msg)
            }
        } else {
            let source_location_type_name =
                syn::LitStr::new(&format!("{}::{}", ty, variant.ident), Span::call_site());
            quote! {
                #tags

                let source_location = buck2_error::source_location::SourceLocation::new(core::file!(), core::line!()).with_type_name(#source_location_type_name);
                let root_error = buck2_error::Error::new(err_msg, tags[0], source_location, None);
                root_error.tag(tags)
            }
        };

        let ident = &variant.ident;
        let pat = fields_pat(&variant.fields);
        quote! {
            #[allow(unused_assignments, unused_variables, deprecated)]
            #ty::#ident #pat => {
                #content
            },
        }
    });
    let error_where_clause = display_inferred_bounds.augment_where_clause(input.generics);

    quote! {
        impl #impl_generics From<#ty #ty_generics> for buck2_error::Error #error_where_clause  {
            #[cold]
            fn from(#arg_token: #ty #ty_generics) -> buck2_error::Error {
                // This is a bit hacky but match moves #arg_token so we need this to be before the match
                let err_msg = format!("{}", #arg_token);

                match #arg_token {
                    #(#from_impl_arms)*
                }
            }
        }
        #display_impl
    }
}

/// Generates the from implementation to buck2_error for a provided error type
fn get_tags(attrs: &Attrs) -> TokenStream {
    let individual_tags: Vec<syn::Expr> = attrs
        .tags
        .iter()
        .map(|tag| match tag {
            OptionStyle::Explicit(tag) => syn::parse_quote! {
                buck2_error::ErrorTag::#tag
            },
            OptionStyle::ByExpr(e) => e.clone(),
        })
        .collect();

    let tags_expr: Option<syn::Expr> = match &attrs.tags_expr {
        Some(OptionStyle::ByExpr(e)) => Some(e.clone()),
        Some(OptionStyle::Explicit(_)) => unreachable!("tags must be an expression"),
        None => None,
    };

    if let Some(tags_expr) = tags_expr {
        quote! {
            let mut tags: Vec<buck2_error::ErrorTag> = #tags_expr;
            for tag in [#(#individual_tags,)*] {
                tags.push(tag);
            }
        }
    } else {
        quote! {
            let tags = [#(#individual_tags,)*];
        }
    }
}

fn fields_pat(fields: &[Field]) -> TokenStream {
    let mut members = fields.iter().map(|field| &field.member).peekable();
    match members.peek() {
        Some(Member::Named(_)) => quote!({ #(#members),* }),
        Some(Member::Unnamed(_)) => {
            let vars = members.map(|member| match member {
                Member::Unnamed(member) => format_ident!("_{}", member),
                Member::Named(_) => unreachable!(),
            });
            quote!((#(#vars),*))
        }
        None => quote!({}),
    }
}
