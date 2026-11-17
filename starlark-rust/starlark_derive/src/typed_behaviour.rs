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

//! The `StarlarkType` methods that a `#[starlark_value]` block may write with a typed
//! signature, and the trait's method derived from one.
//!
//! The typed forms are documented on the methods of `StarlarkType`. What a derived method does
//! is in `starlark::__derive_refs::behaviour`; here it is only told apart and wrapped.

use quote::quote;
use quote::quote_spanned;
use syn::spanned::Spanned;

use crate::module::util::is_type_name;

/// A method that may be written typed.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Method {
    Equals,
    Compare,
    WriteHash,
}

impl Method {
    fn of(name: &syn::Ident) -> Option<Method> {
        if name == "equals" {
            Some(Method::Equals)
        } else if name == "compare" {
            Some(Method::Compare)
        } else if name == "write_hash" {
            Some(Method::WriteHash)
        } else {
            None
        }
    }

    /// The signatures the method may be written with, for an error message.
    fn forms(self) -> &'static str {
        match self {
            Method::Equals => {
                "`fn equals(&self, other: Operand) -> bool`, where `Operand` is an \
                 `UnpackValue` such as `&Self`, the result may also be `starlark::Result<bool>`, \
                 and `fn equals(&self, other: Value<'v>) -> starlark::Result<bool>` is the \
                 trait's own form"
            }
            Method::Compare => {
                "`fn compare(&self, other: Operand) -> Ordering`, where `Operand` is an \
                 `UnpackValue` such as `&Self`, the result may also be \
                 `starlark::Result<Ordering>`, and \
                 `fn compare(&self, other: Value<'v>) -> starlark::Result<Ordering>` is the \
                 trait's own form"
            }
            Method::WriteHash => {
                "`fn write_hash(&self, hasher: &mut StarlarkHasher)` for a type whose values \
                 all hash, or the trait's own form \
                 `fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()>`"
            }
        }
    }

    fn error<T>(self, at: impl quote::ToTokens, what: &str) -> syn::Result<T> {
        Err(syn::Error::new_spanned(
            at,
            format!("{what}; expected {}", self.forms()),
        ))
    }
}

/// The trait's method derived from a typed one.
pub(crate) struct Derived {
    pub(crate) method: syn::ImplItemFn,
    /// The typed method says that every value of the type hashes.
    pub(crate) always_hashes: bool,
}

/// The type a function returns is spelled `Result`, as the trait's methods return.
fn returns_result(output: &syn::ReturnType) -> bool {
    match output {
        syn::ReturnType::Default => false,
        syn::ReturnType::Type(_, ty) => is_type_name(ty, "Result"),
    }
}

/// The trait's method derived from `f`, if `f` is one of the methods that may be written typed
/// and its signature is not the trait's. `None` for every other item, which stays as written.
///
/// A signature is the trait's by its spelling, as `#[starlark_module]` recognises a `Value`
/// parameter: the operand's type is named `Value` and the result's `Result`.
pub(crate) fn derive_from_typed(
    f: &syn::ImplItemFn,
    lt: &syn::Lifetime,
) -> syn::Result<Option<Derived>> {
    let Some(method) = Method::of(&f.sig.ident) else {
        return Ok(None);
    };
    let sig = &f.sig;
    let mut inputs = sig.inputs.iter();
    let (Some(receiver), Some(syn::FnArg::Typed(operand)), None) =
        (inputs.next(), inputs.next(), inputs.next())
    else {
        return method.error(&sig.inputs, "unexpected parameters");
    };

    let typed = match method {
        Method::Equals | Method::Compare => {
            !(is_type_name(&operand.ty, "Value") && returns_result(&sig.output))
        }
        Method::WriteHash => match &sig.output {
            syn::ReturnType::Default => true,
            syn::ReturnType::Type(_, ty) => match &**ty {
                syn::Type::Tuple(unit) if unit.elems.is_empty() => true,
                ty if is_type_name(ty, "Result") => false,
                ty => return method.error(ty, "unexpected result type"),
            },
        },
    };
    if !typed {
        return Ok(None);
    }

    match receiver {
        syn::FnArg::Receiver(syn::Receiver {
            mutability: None,
            kind: syn::ReceiverKind::Reference(_, None, None),
            ..
        }) => {}
        receiver => return method.error(receiver, "the receiver of a typed method is `&self`"),
    }
    if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
        return method.error(&sig.generics, "a typed method has no generics");
    }
    if sig.constness.is_some()
        || sig.asyncness.is_some()
        || sig.safety != syn::Safety::Default
        || sig.abi.is_some()
    {
        return method.error(sig.fn_token, "a typed method has no qualifiers");
    }

    let attrs = &f.attrs;
    let output = &sig.output;
    let block = &f.block;
    // The derived method's parameter takes the name the typed method gave its operand, so that
    // the expansion reads as the method that was written. It cannot be seen from the typed
    // body, whose own operand has that name.
    let untyped = match &*operand.pat {
        syn::Pat::Ident(syn::PatIdent { ident, .. }) => ident.to_string(),
        _ => match method {
            Method::Equals | Method::Compare => "other".to_owned(),
            Method::WriteHash => "hasher".to_owned(),
        },
    };
    let untyped = syn::Ident::new(&untyped, proc_macro2::Span::mixed_site());
    // The typed body goes into the derived method as a closure: there it sees `self`, `Self`
    // and the impl's generics as it did where it was written, which a nested `fn` would not,
    // and it needs no item on `Self`, which may be a type of another crate.
    //
    // The call is spanned to the typed signature, which is what a bound it fails is about.
    let derived: syn::ImplItemFn = match method {
        Method::Equals => {
            let call = quote_spanned! { sig.span() =>
                starlark::__derive_refs::behaviour::typed_equals(#untyped, |#operand| #output #block)
            };
            syn::parse_quote! {
                #( #attrs )*
                fn equals(&self, #untyped: starlark::values::Value<#lt>) -> starlark::Result<bool> {
                    #call
                }
            }
        }
        Method::Compare => {
            let call = quote_spanned! { sig.span() =>
                starlark::__derive_refs::behaviour::typed_compare(self, #untyped, |#operand| #output #block)
            };
            syn::parse_quote! {
                #( #attrs )*
                fn compare(
                    &self,
                    #untyped: starlark::values::Value<#lt>,
                ) -> starlark::Result<std::cmp::Ordering> {
                    #call
                }
            }
        }
        Method::WriteHash => {
            let call = quote_spanned! { sig.span() =>
                starlark::__derive_refs::behaviour::typed_write_hash(#untyped, |#operand| #block)
            };
            syn::parse_quote! {
                #( #attrs )*
                fn write_hash(
                    &self,
                    #untyped: &mut starlark::collections::StarlarkHasher,
                ) -> starlark::Result<()> {
                    #call
                }
            }
        }
    };
    Ok(Some(Derived {
        method: derived,
        always_hashes: method == Method::WriteHash,
    }))
}

/// The marker that every value of the type hashes, for the `impl` whose block wrote a typed
/// `write_hash`.
pub(crate) fn always_hashes_impl(
    input: &syn::ItemImpl,
    lt: &syn::Lifetime,
) -> proc_macro2::TokenStream {
    let (impl_generics, _, where_clause) = input.generics.split_for_impl();
    let self_ty = &input.self_ty;
    quote! {
        impl #impl_generics starlark::values::StarlarkTypeExprHashable<#lt> for #self_ty
        #where_clause
        {
        }
    }
}
