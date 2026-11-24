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

//! Memberships of predicate types, written in a `#[starlark_value]` block:
//!
//! ```ignore
//! #[starlark(implements = StarlarkCmdArgsLike)]
//! fn cmd_args_like(this: ValueTyped<'v, Self>) -> &'v dyn CommandLineArgLike<'v> {
//!     this.as_ref()
//! }
//! ```
//!
//! Such a function is not a member of `StarlarkType`. The functions of a block become the
//! type's `Memberships` impl, which holds each as the conversion of a `Membership`.

use proc_macro2::Span;
use quote::quote;

/// A function of a block that is a membership.
pub(crate) struct Membership {
    /// The predicate type, as written in the attribute.
    predicate: syn::Path,
    name: syn::Ident,
    /// The function's one parameter: the type's `Get`.
    param: syn::PatType,
    /// The function's result: the predicate's handle.
    result: syn::Type,
    body: syn::Block,
}

/// `implements = Path` of a `#[starlark(..)]` attribute.
struct Implements(syn::Path);

impl syn::parse::Parse for Implements {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let name = input.parse::<syn::Ident>()?;
        if name != "implements" {
            return Err(syn::Error::new_spanned(
                name,
                "unknown attribute; in a `#[starlark_value]` block `#[starlark(implements = P)]` \
                 marks a membership of the predicate type `P`",
            ));
        }
        input.parse::<syn::Token![=]>()?;
        Ok(Implements(input.parse::<syn::Path>()?))
    }
}

const SHAPE: &str = "a membership is `fn name(this: Get) -> Handle`: a conversion from the \
    type's own handle (`ValueTyped<'v, Self>` for a type with one representation, `Self` for a \
    view) to the predicate type's";

fn membership(f: &syn::ImplItemFn, predicate: syn::Path) -> syn::Result<Membership> {
    let sig = &f.sig;
    if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(&sig.generics, SHAPE));
    }
    let mut inputs = sig.inputs.iter();
    let param = match (inputs.next(), inputs.next()) {
        (Some(syn::FnArg::Typed(param)), None) => param.clone(),
        _ => return Err(syn::Error::new_spanned(&sig.inputs, SHAPE)),
    };
    let syn::ReturnType::Type(_, result) = &sig.output else {
        return Err(syn::Error::new_spanned(sig, SHAPE));
    };
    Ok(Membership {
        predicate,
        name: sig.ident.clone(),
        param,
        result: (**result).clone(),
        body: f.block.clone(),
    })
}

/// Takes the memberships out of the items of a block.
pub(crate) fn take_memberships(items: &mut Vec<syn::ImplItem>) -> syn::Result<Vec<Membership>> {
    let mut memberships = Vec::new();
    let mut error: Option<syn::Error> = None;
    items.retain(|item| {
        let syn::ImplItem::Fn(f) = item else {
            return true;
        };
        let Some(attr) = f.attrs.iter().find(|a| a.path().is_ident("starlark")) else {
            return true;
        };
        let parsed = attr
            .parse_args::<Implements>()
            .and_then(|Implements(predicate)| membership(f, predicate));
        match parsed {
            Ok(membership) => memberships.push(membership),
            Err(e) => match &mut error {
                Some(error) => error.combine(e),
                None => error = Some(e),
            },
        }
        false
    });
    match error {
        Some(error) => Err(error),
        None => Ok(memberships),
    }
}

/// The `Memberships` impl of the type of a block: one `Membership` for each of the block's
/// memberships, made for the representation the impl is generic over. A type without
/// memberships has the impl as well, with the trait's empty list.
pub(crate) fn memberships_impl(
    memberships: &[Membership],
    input: &syn::ItemImpl,
    lt: &syn::Lifetime,
) -> proc_macro2::TokenStream {
    // Not visible to the conversions, which are closures in the functions that hold them.
    let this = syn::Ident::new("this", Span::mixed_site());
    // The representation the impl is generic over.
    let rep = syn::Ident::new("StarlarkRepresentation", Span::mixed_site());
    let entries = memberships.iter().map(|m| {
        let Membership {
            predicate,
            name,
            param,
            result,
            body,
        } = m;
        quote! {
            starlark::values::predicate::Membership::new::<#predicate, #rep>(|#this| {
                let #name = |#param| -> #result #body;
                #name(<#rep as starlark::values::StarlarkValue<#lt>>::get_typed(#this))
            })
        }
    });
    let list = (!memberships.is_empty()).then(|| {
        quote! {
            const MEMBERSHIPS: &'static [starlark::values::predicate::Membership<
                <#rep as starlark::values::ProvidesStaticType<#lt>>::StaticType,
            >] = &[ #( #entries ),* ];
        }
    });
    let self_ty = &input.self_ty;
    let mut generics = input.generics.clone();
    generics.params.push(syn::parse_quote! { #rep });
    generics
        .make_where_clause()
        .predicates
        .push(syn::parse_quote! {
            #rep: starlark::values::StarlarkValue<#lt, Type = #self_ty>
        });
    let (impl_generics, _, where_clause) = generics.split_for_impl();
    quote! {
        impl #impl_generics starlark::values::predicate::Memberships<#lt, #rep> for #self_ty
        #where_clause
        {
            #list
        }
    }
}
