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
//! trait's `provide_predicate_handle`, which holds each as a closure and provides what it
//! returns, and `implemented_predicates`, which lists the predicate types.

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

/// `provide_predicate_handle` and `implemented_predicates` for the memberships of a block;
/// nothing, which leaves the trait's defaults, if it has none.
pub(crate) fn membership_items(
    memberships: &[Membership],
    lt: &syn::Lifetime,
) -> Vec<syn::ImplItem> {
    if memberships.is_empty() {
        return Vec::new();
    }
    // Not visible to the conversions, which are closures in the function's body.
    let this = syn::Ident::new("this", Span::mixed_site());
    let demand = syn::Ident::new("demand", Span::mixed_site());
    let provide = memberships.iter().map(|m| {
        let Membership {
            predicate,
            name,
            param,
            result,
            body,
        } = m;
        quote! {
            let #name = |#param| -> #result #body;
            #demand.provide_handle::<#predicate>(|| #name(#this));
        }
    });
    let predicates = memberships.iter().map(|m| &m.predicate);
    vec![
        syn::parse_quote! {
            #[inline]
            fn provide_predicate_handle(
                #this: Self::Get,
                #demand: &mut starlark::__derive_refs::Demand<'_, #lt>,
            ) {
                #( #provide )*
            }
        },
        syn::parse_quote! {
            fn implemented_predicates() -> std::vec::Vec<starlark::values::predicate::Predicate> {
                std::vec::Vec::from([
                    #( <#predicates as starlark::values::predicate::StarlarkPredicate>::PREDICATE ),*
                ])
            }
        },
    ]
}
