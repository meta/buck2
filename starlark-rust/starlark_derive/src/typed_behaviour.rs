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

use quote::format_ident;
use quote::quote;
use quote::quote_spanned;
use syn::spanned::Spanned;

use crate::module::util::is_type_name;

/// What a method that may be written typed takes and returns as the trait declares it.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Shape {
    /// `(&self, Value) -> Result<bool>`.
    Equals,
    /// `(&self, Value) -> Result<Ordering>`.
    Compare,
    /// `(&self, &mut StarlarkHasher) -> Result<()>`.
    WriteHash,
    /// `add`, `radd`, `mul`, `rmul`: `(&self, Value, Heap) -> Option<Result<Value>>`, where
    /// `None` gives the other operand its turn.
    OperatorOrNone,
    /// The other binary operators: `(&self, Value, Heap) -> Result<Value>`.
    Operator,
    /// `plus`, `minus`, `bit_not`: `(&self, Heap) -> Result<Value>`.
    UnaryOperator,
    /// `(&self, Value, Heap) -> Result<Value>`.
    At,
    /// `(&self, Value) -> Result<bool>`.
    IsIn,
    /// `(&self) -> Result<i32>`.
    Length,
}

impl Shape {
    /// How many parameters the trait's method has after the receiver.
    fn parameters(self) -> usize {
        match self {
            Shape::Length => 0,
            Shape::Equals
            | Shape::Compare
            | Shape::WriteHash
            | Shape::UnaryOperator
            | Shape::IsIn => 1,
            Shape::OperatorOrNone | Shape::Operator | Shape::At => 2,
        }
    }
}

/// A method that may be written typed.
struct Method {
    name: String,
    shape: Shape,
}

impl Method {
    fn of(name: &syn::Ident) -> Option<Method> {
        let name = name.to_string();
        let shape = match name.as_str() {
            "equals" => Shape::Equals,
            "compare" => Shape::Compare,
            "write_hash" => Shape::WriteHash,
            "add" | "radd" | "mul" | "rmul" => Shape::OperatorOrNone,
            "sub" | "div" | "floor_div" | "percent" | "bit_and" | "bit_or" | "bit_xor"
            | "left_shift" | "right_shift" => Shape::Operator,
            "plus" | "minus" | "bit_not" => Shape::UnaryOperator,
            "at" => Shape::At,
            "is_in" => Shape::IsIn,
            "length" => Shape::Length,
            _ => return None,
        };
        Some(Method { name, shape })
    }

    /// The signatures the method may be written with, for an error message.
    fn forms(&self) -> String {
        let name = &self.name;
        match self.shape {
            Shape::Equals => "`fn equals(&self, other: Operand) -> bool`, where `Operand` is an \
                 `UnpackValue` such as `&Self`, the result may also be `starlark::Result<bool>`, \
                 and `fn equals(&self, other: Value<'v>) -> starlark::Result<bool>` is the \
                 trait's own form"
                .to_owned(),
            Shape::Compare => "`fn compare(&self, other: Operand) -> Ordering`, where `Operand` \
                 is an `UnpackValue` such as `&Self`, the result may also be \
                 `starlark::Result<Ordering>`, and \
                 `fn compare(&self, other: Value<'v>) -> starlark::Result<Ordering>` is the \
                 trait's own form"
                .to_owned(),
            Shape::WriteHash => "`fn write_hash(&self, hasher: &mut StarlarkHasher)` for a type \
                 whose values all hash, or the trait's own form \
                 `fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()>`"
                .to_owned(),
            Shape::OperatorOrNone => format!(
                "`fn {name}(&self, other: Operand, heap: Heap<'v>) -> Made`, where `Operand` is \
                 an `UnpackValue` such as `&Self`, `Made` is an `AllocValue` or a \
                 `starlark::Result` of one, and `heap` may be left out; \
                 `fn {name}(&self, other: Value<'v>, heap: Heap<'v>) -> \
                 Option<starlark::Result<Value<'v>>>` is the trait's own form"
            ),
            Shape::Operator => format!(
                "`fn {name}(&self, other: Operand, heap: Heap<'v>) -> Made`, where `Operand` is \
                 an `UnpackValue` such as `&Self`, `Made` is an `AllocValue` or a \
                 `starlark::Result` of one, and `heap` may be left out; \
                 `fn {name}(&self, other: Value<'v>, heap: Heap<'v>) -> \
                 starlark::Result<Value<'v>>` is the trait's own form"
            ),
            Shape::UnaryOperator => format!(
                "`fn {name}(&self, heap: Heap<'v>) -> Made`, where `Made` is an `AllocValue` or \
                 a `starlark::Result` of one, and `heap` may be left out; \
                 `fn {name}(&self, heap: Heap<'v>) -> starlark::Result<Value<'v>>` is the \
                 trait's own form"
            ),
            Shape::At => "`fn at(&self, index: Index, heap: Heap<'v>) -> Made`, where `Index` is \
                 an `UnpackValue` such as `i32`, `Made` is an `AllocValue` or a \
                 `starlark::Result` of one, and `heap` may be left out; \
                 `fn at(&self, index: Value<'v>, heap: Heap<'v>) -> starlark::Result<Value<'v>>` \
                 is the trait's own form"
                .to_owned(),
            Shape::IsIn => "`fn is_in(&self, other: Operand) -> bool`, where `Operand` is an \
                 `UnpackValue` such as `&str`, the result may also be `starlark::Result<bool>`, \
                 and `fn is_in(&self, other: Value<'v>) -> starlark::Result<bool>` is the \
                 trait's own form"
                .to_owned(),
            Shape::Length => "`fn length(&self) -> i32` for a length that cannot fail, or the \
                 trait's own form `fn length(&self) -> starlark::Result<i32>`"
                .to_owned(),
        }
    }

    fn error<T>(&self, at: impl quote::ToTokens, what: &str) -> syn::Result<T> {
        Err(syn::Error::new_spanned(
            at,
            format!("{what}; expected {}", self.forms()),
        ))
    }
}

/// What the typechecker goes by in the signature of a typed method.
pub(crate) enum Typing {
    /// `compare`: the operand's type.
    Compare { operand: Box<syn::Type> },
    /// `is_in`: the operand's type.
    IsIn { operand: Box<syn::Type> },
    /// A binary operator, or `at` with its index as the operand: the operand's type and the
    /// result's.
    Operator {
        operand: Box<syn::Type>,
        result: Box<syn::Type>,
    },
    /// A unary operator: the result's type.
    UnaryOperator { result: Box<syn::Type> },
}

/// The trait's method derived from a typed one.
pub(crate) struct Derived {
    pub(crate) method: syn::ImplItemFn,
    /// The typed method says that every value of the type hashes.
    pub(crate) always_hashes: bool,
    /// The method is one whose body is typically a few instructions and that the evaluator
    /// calls on its hot paths: `equals`, `compare`, `write_hash`, `at`, `is_in`, `length`.
    pub(crate) small: bool,
    /// What the typed method says to the typechecker, if it is one the typechecker reads.
    pub(crate) typing: Option<Typing>,
}

/// The first type argument of `ty`, if `ty` is a path whose last segment is `name`.
fn type_argument<'a>(ty: &'a syn::Type, name: &str) -> Option<&'a syn::Type> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != name {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    })
}

/// The type a function returns is spelled `Result`, as the trait's methods return.
fn returns_result(output: &syn::ReturnType) -> bool {
    match output {
        syn::ReturnType::Default => false,
        syn::ReturnType::Type(_, ty) => is_type_name(ty, "Result"),
    }
}

/// The type a function returns is spelled `Result<Value>`, as the trait's operators return.
fn returns_result_of_value(output: &syn::ReturnType) -> bool {
    match output {
        syn::ReturnType::Default => false,
        syn::ReturnType::Type(_, ty) => {
            type_argument(ty, "Result").is_some_and(|ty| is_type_name(ty, "Value"))
        }
    }
}

/// The type a function returns is spelled `Option`, as the trait's `add` and its like return.
fn returns_option(output: &syn::ReturnType) -> bool {
    match output {
        syn::ReturnType::Default => false,
        syn::ReturnType::Type(_, ty) => is_type_name(ty, "Option"),
    }
}

/// The result type of a typed operator, which the typechecker reads and so has to be named.
fn operator_result<'a>(method: &Method, sig: &'a syn::Signature) -> syn::Result<&'a syn::Type> {
    struct HasImplTrait(bool);

    impl<'ast> syn::visit::Visit<'ast> for HasImplTrait {
        fn visit_type_impl_trait(&mut self, _: &'ast syn::TypeImplTrait) {
            self.0 = true;
        }
    }

    let syn::ReturnType::Type(_, ty) = &sig.output else {
        return method.error(sig, "a typed operator returns what it makes");
    };
    let mut has_impl_trait = HasImplTrait(false);
    syn::visit::Visit::visit_type(&mut has_impl_trait, ty);
    if has_impl_trait.0 {
        return method.error(
            ty,
            "the result type of a typed operator is named, because the typechecker reads it",
        );
    }
    Ok(ty)
}

/// The name of a parameter of the derived method: the name the typed method gave the parameter
/// it stands for, so that the expansion reads as the method that was written. It cannot be seen
/// from the typed body, whose own parameter has that name. `default` is for a parameter the
/// typed method left out or did not name.
fn derived_parameter(typed: Option<&syn::PatType>, default: &str) -> syn::Ident {
    let name = match typed.map(|typed| &*typed.pat) {
        Some(syn::Pat::Ident(syn::PatIdent { ident, .. })) => {
            ident.to_string().trim_start_matches('_').to_owned()
        }
        _ => String::new(),
    };
    let name = if name.is_empty() { default } else { &name };
    syn::Ident::new(name, proc_macro2::Span::mixed_site())
}

/// The heap parameter of the method derived from a typed operator, and the parameter of the
/// typed body that takes it: the one the typed operator wrote, or a `_` if it left the heap out.
fn heap_parameter(typed: Option<&syn::PatType>) -> (syn::Ident, proc_macro2::TokenStream) {
    let closure_parameter = match typed {
        Some(typed) => quote! { #typed },
        None => quote! { _ },
    };
    (derived_parameter(typed, "heap"), closure_parameter)
}

/// The trait's method derived from `f`, if `f` is one of the methods that may be written typed
/// and its signature is not the trait's. `None` for every other item, which stays as written.
///
/// A signature is the trait's by its spelling, as `#[starlark_module]` recognises a `Value`
/// parameter: the operand's type is named `Value` and the result's is spelled as the trait's
/// (`Result` for `equals`, `compare`, `is_in` and `length`, `Option` for `add` and its like,
/// `Result<Value>` for the other operators and `at`). The trait's operators and `at` take the
/// heap, which a typed one may leave out, so one without it is typed whatever its types are.
pub(crate) fn derive_from_typed(
    f: &syn::ImplItemFn,
    lt: &syn::Lifetime,
) -> syn::Result<Option<Derived>> {
    let Some(method) = Method::of(&f.sig.ident) else {
        return Ok(None);
    };
    let sig = &f.sig;
    let mut inputs = sig.inputs.iter();
    let Some(receiver) = inputs.next() else {
        return method.error(sig, "unexpected parameters");
    };
    // The parameters after the receiver: the operand, if the method has one, then the heap, if
    // the method takes it.
    let mut params = Vec::new();
    for input in inputs {
        match input {
            syn::FnArg::Typed(param) => params.push(param),
            syn::FnArg::Receiver(_) => return method.error(input, "unexpected parameters"),
        }
    }
    let operand_is_value = match (method.shape, &params[..]) {
        (Shape::UnaryOperator | Shape::Length, _) | (_, []) => false,
        (_, [operand, ..]) => is_type_name(&operand.ty, "Value"),
    };
    let every_parameter = params.len() == method.shape.parameters();

    let typed = match method.shape {
        Shape::Equals | Shape::Compare | Shape::IsIn => {
            !(operand_is_value && returns_result(&sig.output))
        }
        Shape::Length => !returns_result(&sig.output),
        Shape::WriteHash => match &sig.output {
            syn::ReturnType::Default => true,
            syn::ReturnType::Type(_, ty) => match &**ty {
                syn::Type::Tuple(unit) if unit.elems.is_empty() => true,
                ty if is_type_name(ty, "Result") => false,
                ty => return method.error(ty, "unexpected result type"),
            },
        },
        Shape::OperatorOrNone => {
            match (
                operand_is_value && every_parameter,
                returns_option(&sig.output),
            ) {
                (true, true) => false,
                (false, true) => {
                    return method.error(
                        &sig.output,
                        "a typed operator returns what it makes, and it is a value that is not \
                         of the operand's type that is `None`",
                    );
                }
                (_, false) => true,
            }
        }
        Shape::Operator | Shape::At => {
            !(operand_is_value && every_parameter && returns_result_of_value(&sig.output))
        }
        Shape::UnaryOperator => !(every_parameter && returns_result_of_value(&sig.output)),
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
    let name = &sig.ident;
    let output = &sig.output;
    let block = &f.block;
    let glue = format_ident!("typed_{}", name);
    // The typed body goes into the derived method as a closure: there it sees `self`, `Self`
    // and the impl's generics as it did where it was written, which a nested `fn` would not,
    // and it needs no item on `Self`, which may be a type of another crate.
    //
    // The call is spanned to the typed signature, which is what a bound it fails is about.
    let (derived, typing): (syn::ImplItemFn, Option<Typing>) = match (method.shape, &params[..]) {
        (Shape::Equals, [operand]) => {
            let other = derived_parameter(Some(operand), "other");
            let call = quote_spanned! { sig.span() =>
                starlark::__derive_refs::behaviour::#glue(#other, |#operand| #output #block)
            };
            let derived = syn::parse_quote! {
                #( #attrs )*
                fn #name(&self, #other: starlark::values::Value<#lt>) -> starlark::Result<bool> {
                    #call
                }
            };
            (derived, None)
        }
        (Shape::Compare, [operand]) => {
            let other = derived_parameter(Some(operand), "other");
            let call = quote_spanned! { sig.span() =>
                starlark::__derive_refs::behaviour::#glue::<Self, _, _>(#other, |#operand| #output #block)
            };
            let derived = syn::parse_quote! {
                #( #attrs )*
                fn #name(
                    &self,
                    #other: starlark::values::Value<#lt>,
                ) -> starlark::Result<std::cmp::Ordering> {
                    #call
                }
            };
            let typing = Typing::Compare {
                operand: operand.ty.clone(),
            };
            (derived, Some(typing))
        }
        (Shape::WriteHash, [typed_hasher]) => {
            let hasher = derived_parameter(Some(typed_hasher), "hasher");
            let call = quote_spanned! { sig.span() =>
                starlark::__derive_refs::behaviour::#glue(#hasher, |#typed_hasher| #block)
            };
            let derived = syn::parse_quote! {
                #( #attrs )*
                fn #name(
                    &self,
                    #hasher: &mut starlark::collections::StarlarkHasher,
                ) -> starlark::Result<()> {
                    #call
                }
            };
            (derived, None)
        }
        (Shape::OperatorOrNone | Shape::Operator, [operand, typed_heap @ ..])
            if typed_heap.len() <= 1 =>
        {
            let result = operator_result(&method, sig)?;
            let other = derived_parameter(Some(operand), "other");
            let (heap, typed_heap) = heap_parameter(typed_heap.first().copied());
            // The error for a value that is not of the operand's type names the type of the
            // receiver, so the glue of an operator that answers with it takes the receiver.
            let of_type = (method.shape == Shape::Operator).then(|| quote! { ::<Self, _, _> });
            let call = quote_spanned! { sig.span() =>
                starlark::__derive_refs::behaviour::#glue #of_type (
                    #other,
                    #heap,
                    |#operand, #typed_heap| #output #block,
                )
            };
            let derived_result = match method.shape {
                Shape::OperatorOrNone => quote! {
                    std::option::Option<starlark::Result<starlark::values::Value<#lt>>>
                },
                _ => quote! { starlark::Result<starlark::values::Value<#lt>> },
            };
            let derived = syn::parse_quote! {
                #( #attrs )*
                fn #name(
                    &self,
                    #other: starlark::values::Value<#lt>,
                    #heap: starlark::values::Heap<#lt>,
                ) -> #derived_result {
                    #call
                }
            };
            let typing = Typing::Operator {
                operand: operand.ty.clone(),
                result: Box::new(result.clone()),
            };
            (derived, Some(typing))
        }
        (Shape::At, [index, typed_heap @ ..]) if typed_heap.len() <= 1 => {
            let result = operator_result(&method, sig)?;
            let untyped_index = derived_parameter(Some(index), "index");
            let (heap, typed_heap) = heap_parameter(typed_heap.first().copied());
            let call = quote_spanned! { sig.span() =>
                starlark::__derive_refs::behaviour::#glue(
                    #untyped_index,
                    #heap,
                    |#index, #typed_heap| #output #block,
                )
            };
            let derived = syn::parse_quote! {
                #( #attrs )*
                fn #name(
                    &self,
                    #untyped_index: starlark::values::Value<#lt>,
                    #heap: starlark::values::Heap<#lt>,
                ) -> starlark::Result<starlark::values::Value<#lt>> {
                    #call
                }
            };
            let typing = Typing::Operator {
                operand: index.ty.clone(),
                result: Box::new(result.clone()),
            };
            (derived, Some(typing))
        }
        (Shape::IsIn, [operand]) => {
            let other = derived_parameter(Some(operand), "other");
            let call = quote_spanned! { sig.span() =>
                starlark::__derive_refs::behaviour::#glue(#other, |#operand| #output #block)
            };
            let derived = syn::parse_quote! {
                #( #attrs )*
                fn #name(&self, #other: starlark::values::Value<#lt>) -> starlark::Result<bool> {
                    #call
                }
            };
            let typing = Typing::IsIn {
                operand: operand.ty.clone(),
            };
            (derived, Some(typing))
        }
        (Shape::Length, []) => {
            let call = quote_spanned! { sig.span() =>
                starlark::__derive_refs::behaviour::#glue(|| #output #block)
            };
            let derived = syn::parse_quote! {
                #( #attrs )*
                fn #name(&self) -> starlark::Result<i32> {
                    #call
                }
            };
            (derived, None)
        }
        (Shape::UnaryOperator, typed_heap) if typed_heap.len() <= 1 => {
            let result = operator_result(&method, sig)?;
            let (heap, typed_heap) = heap_parameter(typed_heap.first().copied());
            let call = quote_spanned! { sig.span() =>
                starlark::__derive_refs::behaviour::#glue(#heap, |#typed_heap| #output #block)
            };
            let derived = syn::parse_quote! {
                #( #attrs )*
                fn #name(
                    &self,
                    #heap: starlark::values::Heap<#lt>,
                ) -> starlark::Result<starlark::values::Value<#lt>> {
                    #call
                }
            };
            let typing = Typing::UnaryOperator {
                result: Box::new(result.clone()),
            };
            (derived, Some(typing))
        }
        _ => return method.error(&sig.inputs, "unexpected parameters"),
    };
    Ok(Some(Derived {
        method: derived,
        always_hashes: method.shape == Shape::WriteHash,
        small: matches!(
            method.shape,
            Shape::Equals
                | Shape::Compare
                | Shape::WriteHash
                | Shape::At
                | Shape::IsIn
                | Shape::Length
        ),
        typing,
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
