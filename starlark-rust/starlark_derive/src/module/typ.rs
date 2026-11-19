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

use dupe::Dupe;
use proc_macro2::Ident;
use proc_macro2::Span;
use syn::Attribute;
use syn::Block;
use syn::Expr;
use syn::Type;
use syn::spanned::Spanned;

use crate::module::parse::ModuleKind;
use crate::module::simple_param::SimpleParam;
use crate::module::util::is_type_name;
use crate::module::util::unpack_option;

/// A type entry from `#[starlark_types(RustType as StarlarkName, ...)]`.
#[derive(Debug)]
pub(crate) struct StarTypeEntry {
    /// The Rust type (e.g., `StarlarkInt`).
    pub(crate) rust_type: syn::Path,
    /// The name to expose in Starlark (e.g., `Int`).
    pub(crate) starlark_name: syn::Ident,
    /// Whether to use `new_no_docs()` instead of `new()`.
    pub(crate) no_docs: bool,
}

#[derive(Debug)]
pub(crate) struct StarModule {
    pub(crate) module_kind: ModuleKind,
    /// The input `ItemFn`, with the body replaced by an empty block, and the one parameter having
    /// been renamed to `globals_builder`
    pub(crate) input: syn::ItemFn,
    pub(crate) docstring: Option<String>,
    pub(crate) stmts: Vec<StarStmt>,
    pub(crate) generics: StarGenerics,
    /// Types declared via `#[starlark_types(RustType as StarlarkName, ...)]`.
    pub(crate) starlark_types: Vec<StarTypeEntry>,
}

/// The generics the user provided on the starlark module
#[derive(Debug)]
pub(crate) struct StarGenerics {
    /// The user provided generics
    ///
    /// This is what we use to instantiate functions we define with their generics, because the `'v`
    /// lifetime is late bound and therefore never passed as an explicit type parameter
    generics: syn::Generics,
    /// The user provided generics with a `'v` param added in front.
    ///
    /// This is what we use to *declare* generics on functions that we define, because all of our
    /// functions want a `'v` param
    generics_with_v: syn::Generics,
}

impl StarGenerics {
    pub(crate) fn new(g: syn::Generics) -> Self {
        let mut with_v = g.clone();
        with_v
            .params
            .insert(0, syn::GenericParam::Lifetime(syn::parse_quote! { 'v }));
        Self {
            generics: g,
            generics_with_v: with_v,
        }
    }

    pub(crate) fn decls(&self) -> syn::ImplGenerics<'_> {
        self.generics_with_v.split_for_impl().0
    }

    pub(crate) fn turbofish(&self) -> syn::Turbofish<'_> {
        self.generics.split_for_impl().1.as_turbofish()
    }

    pub(crate) fn where_clause(&self) -> Option<&syn::WhereClause> {
        self.generics_with_v.split_for_impl().2
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub(crate) enum StarStmt {
    Const(StarConst),
    Fun(StarFun),
    Attr(StarAttr),
}

#[derive(Debug)]
pub(crate) struct StarConst {
    pub name: Ident,
    pub ty: Type,
    pub value: Expr,
}

#[derive(Debug)]
pub(crate) struct SpecialParam {
    pub(crate) param: SimpleParam,
}

#[derive(Debug)]
pub(crate) struct StarFun {
    pub name: Ident,
    pub as_type: Option<syn::Path>,
    pub attrs: Vec<Attribute>,
    pub this: Option<ThisParam>,
    pub args: RegularParams,
    /// Has `Heap<'_>` parameter.
    pub heap: Option<SpecialParam>,
    /// Has `&mut Evaluator` parameter.
    pub eval: Option<SpecialParam>,
    /// `anyhow::Result<T>`.
    pub return_type: Type,
    pub starlark_ty_custom_function: Option<Expr>,
    pub special_builtin_function: Option<Expr>,
    pub speculative_exec_safe: bool,
    pub body: Block,
    pub source: StarFunSource,
    pub docstring: Option<String>,
}

impl StarFun {
    /// Is this function a method? (I. e. has `this` as first parameter).
    pub(crate) fn is_method(&self) -> bool {
        self.this.is_some()
    }

    pub(crate) fn span(&self) -> Span {
        self.name
            .span()
            .join(self.body.span())
            .unwrap_or_else(|| self.name.span())
    }
}

#[derive(Debug)]
pub(crate) struct StarAttr {
    pub name: Ident,
    pub this: ThisParam,
    /// Has `Heap<'_>` parameter.
    pub heap: Option<SpecialParam>,
    pub attrs: Vec<Attribute>,
    /// `anyhow::Result<T>`.
    pub return_type: Type,
    pub speculative_exec_safe: bool,
    pub body: Block,
    pub docstring: Option<String>,
}

#[derive(Debug, PartialEq, Copy, Clone, Dupe)]
pub(crate) enum StarArgPassStyle {
    /// Parameter can be filled only positionally.
    PosOnly,
    /// Parameter can be filled positionally or by name.
    PosOrNamed,
    /// Parameter can be filled by name.
    NamedOnly,
    /// `*args`.
    Args,
    /// `**kwargs`.
    Kwargs,
}

/// Method `this` parameter, always first.
#[derive(Debug, Clone)]
pub(crate) struct ThisParam {
    pub(crate) param: SimpleParam,
}

impl ThisParam {
    pub(crate) fn render_prepare(&self, target: &syn::Ident, value: &syn::Ident) -> syn::Stmt {
        let ty = &self.param.ty;
        syn::parse_quote! {
            let #target: #ty = starlark::__derive_refs::parse_args::check_this(#value)?;
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StarArg {
    pub span: Span,
    pub(crate) param: SimpleParam,
    pub pass_style: StarArgPassStyle,
    pub default: Option<Expr>,
    /// Omit from generated documentation.
    pub doc_hidden: bool,
    pub source: StarArgSource,
}

/// `&Arguments` parameter.
#[derive(Debug)]
pub(crate) struct StarArguments {
    pub(crate) param: SimpleParam,
}

/// How we handle `&Arguments`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub(crate) enum RegularParams {
    /// Pass `&Arguments` as is.
    Arguments(StarArguments),
    /// Unpack the `&Arguments` into a multiple typed parameters.
    Unpack(Vec<StarArg>),
}

#[derive(Debug, PartialEq, Clone)]
pub(crate) enum StarArgSource {
    Unknown,
    Argument(usize),
    Required(usize),
    Optional(usize),
    Kwargs,
}

#[derive(Debug)]
pub(crate) enum StarFunSource {
    /// Function signature is single `Arguments` parameter.
    Arguments,
    /// Normal function which uses a signature and parameters parser.
    Signature { count: usize },
    /// Fast-path function of some required parameters, followed by some optional parameters.
    /// No named parameters or `*args`, but may have `**kwargs`.
    Positional {
        required: usize,
        optional: usize,
        kwargs: bool,
    },
}

impl StarArg {
    pub fn is_option(&self) -> bool {
        is_type_name(&self.param.ty, "Option")
    }

    /// Remove the `Option` if it exists, otherwise return the real type.
    pub fn without_option(&self) -> &Type {
        unpack_option(&self.param.ty).unwrap_or(&self.param.ty)
    }

    pub fn is_value(&self) -> bool {
        is_type_name(&self.param.ty, "Value")
    }

    /// Parameter type is `Option<Value>`.
    pub(crate) fn is_option_value(&self) -> bool {
        self.is_option() && is_type_name(self.without_option(), "Value")
    }

    pub fn requires_signature(&self) -> bool {
        // We need to use a signature if something has a name
        // There are *args
        // There is a default that needs promoting to a Value (since the signature stores that value)
        (self.pass_style != StarArgPassStyle::PosOnly
            && self.pass_style != StarArgPassStyle::Kwargs)
            || (self.is_value() && self.default.is_some())
    }
}
