/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Derive macro for the `variant_name` crate, which re-exports it. Use it from there.

use quote::quote;
use syn::Data;
use syn::DeriveInput;
use syn::Fields;
use syn::parse_macro_input;
use syn::spanned::Spanned;

#[proc_macro_derive(VariantName)]
pub fn derive_variant_name(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    match derive_variant_name_impl(input) {
        Ok(tokens) => tokens,
        Err(err) => err.to_compile_error().into(),
    }
}

fn derive_variant_name_impl(input: DeriveInput) -> syn::Result<proc_macro::TokenStream> {
    if let Data::Enum(data_enum) = input.data {
        let mut variant_body = Vec::new();
        let mut variant_lowercase_body = Vec::new();
        for variant in data_enum.variants {
            let variant_name = &variant.ident;
            let patterns = match variant.fields {
                Fields::Unit => quote! {},
                Fields::Named(_) => quote! { {..} },
                Fields::Unnamed(_) => quote! { (..) },
            };
            let variant_name_str = variant_name.to_string();
            let variant_name_lowercase_str = to_snake_case(&variant_name_str);
            variant_body.push(quote! {
                Self::#variant_name #patterns => #variant_name_str
            });
            variant_lowercase_body.push(quote! {
                Self::#variant_name #patterns => #variant_name_lowercase_str
            });
        }

        let name = &input.ident;
        let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

        // Matching on `*self` rather than `self` keeps this compiling for enums with no variants.
        let r#gen = quote! {
            #[automatically_derived]
            impl #impl_generics ::variant_name::VariantName for #name #ty_generics #where_clause {
                fn variant_name(&self) -> &'static str {
                    match *self {
                        #(#variant_body,)*
                    }
                }

                fn variant_name_lowercase(&self) -> &'static str {
                    match *self {
                        #(#variant_lowercase_body,)*
                    }
                }
            }
        };

        Ok(r#gen.into())
    } else {
        Err(syn::Error::new(
            input.span(),
            "`VariantName` can only be derived for enums",
        ))
    }
}

fn to_snake_case(s: &str) -> String {
    let mut out = String::new();
    let mut is_first = true;
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            if !is_first {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
        is_first = false;
    }

    out
}
