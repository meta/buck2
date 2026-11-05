/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The `#[buck_test]` attribute of the `buck2_e2e` test harness. Use it through `buck2_e2e`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::format_ident;
use quote::quote;
use quote::quote_spanned;
use syn::FnArg;
use syn::ItemFn;
use syn::Meta;
use syn::Pat;
use syn::Token;
use syn::parse_macro_input;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;

/// Turns an `async fn` into a buck e2e test case. See the `buck2_e2e` crate documentation.
#[proc_macro_attribute]
pub fn buck_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    let options = parse_macro_input!(attr with Punctuated::<Meta, Token![,]>::parse_terminated);
    let function = parse_macro_input!(item as ItemFn);
    expand(options, function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(options: Punctuated<Meta, Token![,]>, function: ItemFn) -> syn::Result<TokenStream2> {
    if function.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            function.sig.fn_token.span(),
            "a `#[buck_test]` function must be `async`",
        ));
    }
    if let Some(attr) = function
        .attrs
        .iter()
        .find(|attr| attr.path().is_ident("should_panic"))
    {
        return Err(syn::Error::new(
            attr.span(),
            "a `#[buck_test]` cannot be `#[should_panic]`: the test process aborts on a panic. \
             Assert on the failure instead, e.g. with `expect_failure()`",
        ));
    }
    if !function.sig.generics.params.is_empty() {
        return Err(syn::Error::new(
            function.sig.generics.span(),
            "a `#[buck_test]` function takes no generic parameters",
        ));
    }

    let name = &function.sig.ident;

    // Each option `name = value` becomes a call `.name(value)` on the test's `BuckTest`, and a
    // bare `name` becomes `.name(true)`.
    let mut builder = quote!(::buck2_e2e::BuckTest::new(
        module_path!(),
        stringify!(#name)
    ));
    for option in options {
        builder = match option {
            Meta::Path(path) => {
                let method = path.require_ident()?;
                quote!(#builder.#method(true))
            }
            Meta::NameValue(name_value) => {
                let method = name_value.path.require_ident()?;
                let value = &name_value.value;
                quote!(#builder.#method(#value))
            }
            Meta::List(list) => {
                return Err(syn::Error::new(
                    list.span(),
                    "expected a `#[buck_test]` option as `name = value` or `name`",
                ));
            }
        };
    }

    let mut arguments = Vec::new();
    let mut wants_buck = false;
    let mut wants_tmp_path = false;
    for input in &function.sig.inputs {
        let FnArg::Typed(typed) = input else {
            return Err(syn::Error::new(
                input.span(),
                "a `#[buck_test]` function takes no `self`",
            ));
        };
        let Pat::Ident(ident) = &*typed.pat else {
            return Err(syn::Error::new(
                typed.pat.span(),
                "a `#[buck_test]` parameter must be named `buck` or `tmp_path`",
            ));
        };
        if !matches!(&*typed.ty, syn::Type::Reference(_)) {
            return Err(syn::Error::new(
                typed.ty.span(),
                "a `#[buck_test]` parameter is a reference: `&Buck`, `&mut Buck` or `&Path`",
            ));
        }
        let span = typed.ty.span();
        // A test that only needs the project set up can name its `buck` `_buck`.
        let (name, seen) = match ident.ident.to_string().trim_start_matches('_') {
            "buck" => {
                arguments.push(quote_spanned!(span=> ::buck2_e2e::__macro_support::buck(
                    &mut fixture.buck
                )));
                ("buck", &mut wants_buck)
            }
            "tmp_path" => {
                arguments.push(quote_spanned!(span=> fixture.tmp_path.as_path()));
                ("tmp_path", &mut wants_tmp_path)
            }
            _ => {
                return Err(syn::Error::new(
                    ident.ident.span(),
                    "a `#[buck_test]` parameter must be named `buck` (a `&Buck` or `&mut Buck`) \
                     or `tmp_path` (a `&Path`)",
                ));
            }
        };
        if std::mem::replace(seen, true) {
            return Err(syn::Error::new(
                ident.ident.span(),
                format!("a `#[buck_test]` takes `{name}` only once"),
            ));
        }
    }
    if !wants_buck {
        builder = quote!(#builder.without_buck());
    }

    let mut body = function.clone();
    body.attrs.clear();
    body.vis = syn::Visibility::Inherited;
    body.sig.ident = format_ident!("__buck_test_body");

    let attrs = &function.attrs;
    let vis = &function.vis;
    Ok(quote! {
        #[test]
        #(#attrs)*
        #vis fn #name() -> ::buck2_e2e::__macro_support::TestResult {
            #body

            fn __buck_test_call(
                fixture: &mut ::buck2_e2e::__macro_support::Fixture,
            ) -> ::buck2_e2e::__macro_support::BoxFuture<'_, ::buck2_e2e::__macro_support::TestResult> {
                ::std::boxed::Box::pin(async move {
                    ::buck2_e2e::__macro_support::IntoTestResult::into_test_result(
                        __buck_test_body(#(#arguments),*).await,
                    )
                })
            }

            #builder.run(__buck_test_call)
        }
    })
}
