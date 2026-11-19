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

use proc_macro2::Ident;
use proc_macro2::TokenStream;
use quote::quote;
use quote::quote_spanned;
use syn::Attribute;
use syn::DeriveInput;
use syn::GenericParam;
use syn::LitStr;
use syn::Token;
use syn::WherePredicate;
use syn::parse::ParseStream;
use syn::parse_macro_input;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;

use crate::util::DeriveInputUtil;

struct Input<'a> {
    input: &'a DeriveInput,
}

impl Input<'_> {
    fn angle_brankets(&self, tokens: &[TokenStream]) -> TokenStream {
        let span = self.input.span();
        if tokens.is_empty() {
            quote_spanned! { span=> }
        } else {
            quote_spanned! { span=> < #(#tokens,)* > }
        }
    }

    /// The generics of the impl: the brand `'v` of `Freeze<'v>`, the parameters of
    /// `impl<..>`, of the type, and of `Frozen<'fv>`.
    ///
    /// The brand is the type's lifetime parameter. A type without one holds no values, so it
    /// implements the trait at every brand, which the impl names `'v`. A type with several is
    /// rejected: the derive cannot tell which one is the heap's.
    ///
    /// Type parameters are frozen through their own `Freeze` impl, so they are bound by it and
    /// mapped through `Frozen<'fv>`.
    fn format_impl_generics(
        &self,
    ) -> syn::Result<(syn::Lifetime, TokenStream, TokenStream, TokenStream)> {
        let trait_ = freeze_trait();
        let span = self.input.span();
        let mut lifetimes = self.input.generics.lifetimes();
        let brand = match (lifetimes.next(), lifetimes.next()) {
            (None, _) => syn::Lifetime::new("'v", span),
            (Some(lt), None) => lt.lifetime.clone(),
            (Some(_), Some(second)) => {
                return Err(syn::Error::new_spanned(
                    second,
                    "`#[derive(Freeze)]` cannot tell which lifetime parameter is the \
                     heap's brand; implement `Freeze` by hand",
                ));
            }
        };
        let mut impl_params = Vec::new();
        let mut input_params = Vec::new();
        let mut output_params = Vec::new();
        if self.input.generics.lifetimes().next().is_none() {
            impl_params.push(quote_spanned! { span=> #brand });
        }
        for param in &self.input.generics.params {
            match param {
                GenericParam::Type(t) => {
                    let name = &t.ident;
                    let bounds = t.bounds.iter();
                    impl_params.push(quote_spanned! {
                        span=>
                        #name: #(#bounds +)* #trait_<#brand>
                    });
                    output_params.push(quote_spanned! {
                        span=>
                        <#name as #trait_<#brand>>::Frozen<'fv>
                    });
                    input_params.push(quote_spanned! {
                        span=>
                        #name
                    });
                }
                GenericParam::Lifetime(lt) => {
                    impl_params.push(quote_spanned! { span=> #lt });
                    input_params.push(quote_spanned! { span=> #lt });
                    output_params.push(quote! { 'fv });
                }
                GenericParam::Const(_) => {
                    return Err(syn::Error::new_spanned(
                        param,
                        "const generics not supported",
                    ));
                }
            }
        }
        Ok((
            brand,
            self.angle_brankets(&impl_params),
            self.angle_brankets(&input_params),
            self.angle_brankets(&output_params),
        ))
    }
}

fn derive_freeze_impl(input: DeriveInput) -> syn::Result<syn::ItemImpl> {
    let span = input.span();
    let input = Input { input: &input };

    let name = &input.input.ident;

    let FreezeDeriveOptions {
        validator,
        bounds,
        identity,
    } = extract_options(&input.input.attrs)?;

    if let Some(identity) = identity {
        return Err(syn::Error::new_spanned(
            identity,
            "`identity` can only be used on fields",
        ));
    }

    let (brand, impl_params, input_params, output_params) = input.format_impl_generics()?;

    let bounds_body = match bounds {
        Some(bounds) => quote_spanned! { span=> where #bounds },
        None => quote_spanned! { span=> },
    };

    let validate_body = match validator {
        Some(validator) => quote_spanned! {
            span=>
            match #validator(&frozen) {
                Ok(v) => v,
                Err(e) => return std::result::Result::Err(FreezeError::new(e.to_string()))
            };
        },
        None => quote_spanned! { span=> },
    };
    let body = freeze_impl(input.input)?;
    let body = quote_spanned! { span=>
        let frozen = #body;
        #validate_body
        std::result::Result::Ok(frozen)
    };

    let trait_ = freeze_trait();

    let r#gen = syn::parse_quote_spanned! {
        span=>
        impl #impl_params #trait_<#brand> for #name #input_params #bounds_body {
            type Frozen<'fv> = #name #output_params;
            #[allow(unused_variables)]
            fn freeze<'fv>(self, freezer: &starlark::values::Freezer<#brand, 'fv>) -> starlark::values::FreezeResult<Self::Frozen<'fv>> {
                #body
            }
        }
    };

    Ok(r#gen)
}

syn::custom_keyword!(identity);

#[derive(Default)]
struct FreezeDeriveOptions {
    /// `#[freeze(validator = function)]`.
    validator: Option<Ident>,
    /// `#[freeze(bounds = ...)]`.
    bounds: Option<Punctuated<WherePredicate, Token![,]>>,
    /// `#[freeze(identity)]`.
    identity: Option<identity>,
}

/// Parse a `#[freeze(...)]` annotation.
fn extract_options(attrs: &[Attribute]) -> syn::Result<FreezeDeriveOptions> {
    syn::custom_keyword!(validator);
    syn::custom_keyword!(bounds);

    let mut opts = FreezeDeriveOptions::default();

    for attr in attrs.iter() {
        if !attr.path().is_ident("freeze") {
            continue;
        }

        attr.parse_args_with(|input: ParseStream| {
            loop {
                if let Some(validator) = input.parse::<Option<validator>>()? {
                    if opts.validator.is_some() {
                        return Err(syn::Error::new_spanned(
                            validator,
                            "`validator` was set twice",
                        ));
                    }
                    input.parse::<Token![=]>()?;
                    opts.validator = Some(input.parse()?);
                } else if let Some(bounds) = input.parse::<Option<bounds>>()? {
                    if opts.bounds.is_some() {
                        return Err(syn::Error::new_spanned(bounds, "`bounds` was set twice"));
                    }
                    input.parse::<Token![=]>()?;
                    let bounds_input = input.parse::<LitStr>()?;
                    opts.bounds = Some(bounds_input.parse_with(Punctuated::parse_terminated)?);
                } else if let Some(identity) = input.parse::<Option<identity>>()? {
                    if opts.identity.is_some() {
                        return Err(syn::Error::new_spanned(
                            identity,
                            "`identity` was set twice",
                        ));
                    }
                    opts.identity = Some(identity);
                } else {
                    return Err(input.lookahead1().error());
                }

                if input.parse::<Option<Token![,]>>()?.is_none() {
                    break;
                }
            }

            Ok(())
        })?;
    }

    Ok(opts)
}

fn freeze_impl(derive_input: &DeriveInput) -> syn::Result<syn::Expr> {
    let trait_ = freeze_trait();
    let derive_input = DeriveInputUtil::new(derive_input)?;
    derive_input.match_self(|struct_or_enum_variant, fields| {
        let fields: Vec<syn::Expr> = fields
            .iter()
            .map(|(ident, f)| {
                let span = ident.span();

                let FreezeDeriveOptions {
                    validator,
                    bounds,
                    identity,
                } = extract_options(&f.attrs)?;
                if let Some(validator) = validator {
                    return Err(syn::Error::new_spanned(
                        validator,
                        "Cannot use `validator` on field",
                    ));
                }
                if let Some(bounds) = bounds {
                    return Err(syn::Error::new_spanned(
                        bounds,
                        "Cannot use `bounds` on field",
                    ));
                }

                if identity.is_some() {
                    Ok(syn::parse_quote_spanned! { span=>
                        #ident
                    })
                } else {
                    Ok(syn::parse_quote_spanned! { span=>
                        #trait_::freeze(#ident, freezer)?
                    })
                }
            })
            .collect::<syn::Result<_>>()?;
        struct_or_enum_variant.construct(fields)
    })
}

fn freeze_trait() -> TokenStream {
    quote! { starlark::values::Freeze }
}

pub fn derive_freeze(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    match derive_freeze_impl(input) {
        Ok(input) => quote! { #input }.into(),
        Err(e) => e.to_compile_error().into(),
    }
}
