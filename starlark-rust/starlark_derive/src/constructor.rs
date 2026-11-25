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

//! The constructor of a type, written in its `#[starlark_value]` block:
//!
//! ```ignore
//! /// Makes a `foo`.
//! #[starlark(constructor)]
//! fn foo(x: i32, heap: Heap<'v>) -> starlark::Result<ValueOf<'v, Self>> { .. }
//! ```
//!
//! It is a native function like those of a `#[starlark_module]` of globals, and it is the
//! type's symbol: what a Starlark program calls, writes in a type annotation and passes to
//! `isinstance`. It is not a member of `StarlarkType`; the block hands it, with `Self` named, to
//! `#[starlark_module]` as a module of one function.

use proc_macro2::Group;
use proc_macro2::TokenStream;
use proc_macro2::TokenTree;
use quote::ToTokens;
use quote::format_ident;
use quote::quote;

/// The comma-separated arguments of a `#[starlark(..)]` attribute.
fn attr_items(attr: &syn::Attribute) -> Vec<TokenStream> {
    let syn::Meta::List(list) = &attr.meta else {
        return Vec::new();
    };
    let mut items = vec![TokenStream::new()];
    for tt in list.tokens.clone() {
        match &tt {
            TokenTree::Punct(p) if p.as_char() == ',' => items.push(TokenStream::new()),
            _ => items
                .last_mut()
                .expect("the list starts with an item")
                .extend([tt]),
        }
    }
    items.retain(|i| !i.is_empty());
    items
}

/// Takes the function marked `#[starlark(constructor)]` out of the items of a block, with the
/// marker removed from its attribute.
pub(crate) fn take_constructor(
    items: &mut Vec<syn::ImplItem>,
) -> syn::Result<Option<syn::ImplItemFn>> {
    let mut constructor = None;
    let mut kept = Vec::new();
    for item in items.drain(..) {
        let syn::ImplItem::Fn(mut f) = item else {
            kept.push(item);
            continue;
        };
        let mut marked = false;
        for attr in &mut f.attrs {
            if !attr.path().is_ident("starlark") {
                continue;
            }
            let (markers, rest): (Vec<TokenStream>, Vec<TokenStream>) =
                attr_items(attr).into_iter().partition(|item| {
                    matches!(
                        item.clone().into_iter().next(),
                        Some(TokenTree::Ident(i)) if i == "constructor"
                    )
                });
            if !markers.is_empty() {
                marked = true;
                *attr = syn::parse_quote! { #[starlark( #( #rest ),* )] };
            }
        }
        if !marked {
            kept.push(syn::ImplItem::Fn(f));
            continue;
        }
        // An attribute left with nothing in it says nothing.
        f.attrs
            .retain(|a| !(a.path().is_ident("starlark") && attr_items(a).is_empty()));
        if constructor.is_some() {
            return Err(syn::Error::new_spanned(
                &f.sig.ident,
                "a type has at most one constructor; another function that makes a value of \
                 the type is a function of a `#[starlark_module]`",
            ));
        }
        constructor = Some(f);
    }
    *items = kept;
    Ok(constructor)
}

/// `self_ty` with `::` before its generic arguments. That is the type where a type is written,
/// and where an expression or a pattern is, as in `Self(x)` or `Self { x }`, it is the path
/// that a type with generic arguments has to be written as.
fn with_turbofish(self_ty: &syn::Type) -> TokenStream {
    let mut self_ty = self_ty.clone();
    if let syn::Type::Path(path) = &mut self_ty {
        for segment in &mut path.path.segments {
            if let syn::PathArguments::AngleBracketed(arguments) = &mut segment.arguments {
                arguments.colon2_token = Some(Default::default());
            }
        }
    }
    self_ty.to_token_stream()
}

/// `tokens` with `Self` named: `Self::Get` is the type's `Get`, and `Self` is `self_ty`.
fn name_self(tokens: TokenStream, self_ty: &syn::Type, lt: &syn::Lifetime) -> TokenStream {
    let tokens: Vec<TokenTree> = tokens.into_iter().collect();
    let mut out = TokenStream::new();
    let mut i = 0;
    let is_colon =
        |tt: Option<&TokenTree>| matches!(tt, Some(TokenTree::Punct(p)) if p.as_char() == ':');
    while i < tokens.len() {
        match &tokens[i] {
            TokenTree::Ident(ident) if ident == "Self" => {
                let path = is_colon(tokens.get(i + 1)) && is_colon(tokens.get(i + 2));
                let get =
                    path && matches!(tokens.get(i + 3), Some(TokenTree::Ident(g)) if g == "Get");
                if get {
                    out.extend(quote! {
                        <#self_ty as starlark::values::StarlarkType<#lt>>::Get
                    });
                    i += 4;
                } else if path {
                    out.extend(quote! { <#self_ty> });
                    i += 1;
                } else {
                    out.extend(with_turbofish(self_ty));
                    i += 1;
                }
            }
            TokenTree::Group(g) => {
                let mut group = Group::new(g.delimiter(), name_self(g.stream(), self_ty, lt));
                group.set_span(g.span());
                out.extend([TokenTree::Group(group)]);
                i += 1;
            }
            tt => {
                out.extend([tt.clone()]);
                i += 1;
            }
        }
    }
    out
}

/// The module of globals that holds the constructor `f` of the type `self_ty`, and the name of
/// the function that declares it. `static_ty` is the type at `'static`.
pub(crate) fn constructor_module(
    f: &syn::ImplItemFn,
    self_ty: &syn::Type,
    static_ty: &syn::Type,
    lt: &syn::Lifetime,
) -> syn::Result<(TokenStream, syn::Ident)> {
    let syn::Type::Path(path) = self_ty else {
        return Err(syn::Error::new_spanned(
            self_ty,
            "a constructor needs a type that is named by a path",
        ));
    };
    let ident = &path
        .path
        .segments
        .last()
        .expect("a path has a segment")
        .ident;
    let module = format_ident!("__starlark_constructor_of_{}", ident);
    let attrs = &f.attrs;
    let name = &f.sig.ident;
    let inputs = &f.sig.inputs;
    let output = &f.sig.output;
    let block = &f.block;
    // The function names the block's lifetime; outside the block it declares it.
    let mut generics = f.sig.generics.clone();
    if !generics.lifetimes().any(|l| l.lifetime == *lt) {
        generics.params.insert(0, syn::parse_quote! { #lt });
    }
    let where_clause = &generics.where_clause;
    let function = name_self(
        quote! {
            #[starlark(as_type = #static_ty)]
            #( #attrs )*
            fn #name #generics (#inputs) #output #where_clause #block
        },
        self_ty,
        lt,
    );
    Ok((
        quote! {
            #[allow(non_snake_case)]
            #[starlark::starlark_module]
            fn #module(globals: &mut starlark::environment::GlobalsBuilder) {
                #function
            }
        },
        module,
    ))
}
