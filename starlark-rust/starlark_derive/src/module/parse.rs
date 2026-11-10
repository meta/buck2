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

mod fun;

use dupe::Dupe;
use syn::Attribute;
use syn::Expr;
use syn::ExprLit;
use syn::FnArg;
use syn::Item;
use syn::ItemConst;
use syn::ItemFn;
use syn::Meta;
use syn::MetaNameValue;
use syn::PatType;
use syn::Stmt;
use syn::Type;
use syn::TypeReference;
use syn::Visibility;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::token::Comma;

use crate::module::parse::fun::parse_fun;
use crate::module::typ::StarConst;
use crate::module::typ::StarGenerics;
use crate::module::typ::StarModule;
use crate::module::typ::StarStmt;
use crate::module::typ::StarTypeEntry;
use crate::module::util::is_type_name;

#[derive(Debug, Copy, Clone, Dupe, PartialEq, Eq)]
pub(crate) enum ModuleKind {
    Globals,
    Methods,
}

impl ModuleKind {
    pub(crate) fn statics_macro_name(self) -> &'static str {
        match self {
            ModuleKind::Globals => "globals_static",
            ModuleKind::Methods => "methods_static",
        }
    }
}

/// Parse a single entry in `#[starlark_types(RustType as StarlarkName)]`
/// or `#[starlark_types(RustType as StarlarkName no_docs)]`.
impl syn::parse::Parse for StarTypeEntry {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let rust_type: syn::Path = input.parse()?;
        input.parse::<syn::Token![as]>()?;
        let starlark_name: syn::Ident = input.parse()?;
        let no_docs = if input.peek(syn::Ident) {
            let kw: syn::Ident = input.parse()?;
            if kw != "no_docs" {
                return Err(syn::Error::new(kw.span(), "expected `no_docs`"));
            }
            true
        } else {
            false
        };
        Ok(StarTypeEntry {
            rust_type,
            starlark_name,
            no_docs,
        })
    }
}

/// Extract and remove `#[starlark_types(...)]` attributes from the function.
fn parse_starlark_types(input: &mut ItemFn) -> syn::Result<Vec<StarTypeEntry>> {
    let mut result = Vec::new();
    let mut remaining_attrs = Vec::new();
    for attr in input.attrs.drain(..) {
        if attr.path().is_ident("starlark_types") {
            let entries: Punctuated<StarTypeEntry, Comma> =
                attr.parse_args_with(Punctuated::parse_terminated)?;
            result.extend(entries);
        } else {
            remaining_attrs.push(attr);
        }
    }
    input.attrs = remaining_attrs;
    Ok(result)
}

pub(crate) fn parse(mut input: ItemFn) -> syn::Result<StarModule> {
    let starlark_types = parse_starlark_types(&mut input)?;
    let module_docstring = parse_module_docstring(&input)?;

    if input.sig.inputs.len() != 1 {
        return Err(syn::Error::new(
            input.sig.span(),
            "function must have exactly one argument",
        ));
    }
    let arg = input.sig.inputs.last_mut().unwrap();
    let arg_span = arg.span();

    let (pat, module_kind) = match arg {
        FnArg::Typed(PatType { ty, pat, .. }) if is_mut_globals_builder(ty) => {
            (pat, ModuleKind::Globals)
        }
        FnArg::Typed(PatType { ty, pat, .. }) if is_mut_methods_builder(ty) => {
            (pat, ModuleKind::Methods)
        }
        _ => {
            return Err(syn::Error::new(
                arg_span,
                "Expected a mutable globals or methods builder",
            ));
        }
    };
    // FIXME(JakobDegen): Pick one form and enforce it
    let (syn::Pat::Ident(_) | syn::Pat::Wild(_)) = &mut **pat else {
        return Err(syn::Error::new(pat.span(), "Expected ident"));
    };
    // Replace the argument with a known one - the user can't depend on it anyway
    **pat = syn::Pat::Ident(syn::PatIdent {
        attrs: Default::default(),
        by_ref: None,
        mutability: None,
        ident: syn::Ident::new("globals_builder", pat.span()),
        subpat: None,
    });

    let stmts = std::mem::replace(
        &mut input.block,
        Box::new(syn::Block {
            brace_token: Default::default(),
            stmts: Vec::new(),
        }),
    )
    .stmts
    .into_iter()
    .map(|stmt| parse_stmt(stmt, module_kind))
    .collect::<syn::Result<_>>()?;

    Ok(StarModule {
        generics: StarGenerics::new(input.sig.generics.clone()),
        module_kind,
        input,
        docstring: module_docstring,
        stmts,
        starlark_types,
    })
}

fn is_attribute_docstring(x: &Attribute) -> syn::Result<Option<String>> {
    if x.path().is_ident("doc") {
        if let Meta::NameValue(MetaNameValue {
            value:
                Expr::Lit(ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }),
            ..
        }) = &x.meta
        {
            let ds = s.value();
            if ds.starts_with("# ") || ds.starts_with("## ") {
                return Err(syn::Error::new(
                    x.span(),
                    "Docstrings may not contain H1 or H2 headers (`#` or `##`) as this leads to \
                    poor generated documentation. Use at least H3 (`###`) instead.",
                ));
            }

            return Ok(Some(ds));
        }
    }
    Ok(None)
}

/// Return (docstring, other attributes)
fn parse_module_docstring(input: &ItemFn) -> syn::Result<Option<String>> {
    let mut doc_attrs = Vec::new();
    for attr in &input.attrs {
        if let Some(ds) = is_attribute_docstring(attr)? {
            doc_attrs.push(ds);
        }
    }
    if doc_attrs.is_empty() {
        Ok(None)
    } else {
        Ok(Some(doc_attrs.join("\n")))
    }
}

fn parse_stmt(stmt: Stmt, module_kind: ModuleKind) -> syn::Result<StarStmt> {
    match stmt {
        Stmt::Item(Item::Fn(x)) => parse_fun(x, module_kind),
        Stmt::Item(Item::Const(x)) => Ok(StarStmt::Const(parse_const(x)?)),
        s => Err(syn::Error::new(
            s.span(),
            "Can only put constants and functions inside a #[starlark_module]",
        )),
    }
}

fn parse_visibility(vis: &Visibility) -> syn::Result<()> {
    match vis {
        Visibility::Inherited => Ok(()),
        _ => Err(syn::Error::new(
            vis.span(),
            "Visibility modifiers are not allowed inside a `#[starlark_module]`",
        )),
    }
}

fn parse_const(x: ItemConst) -> syn::Result<StarConst> {
    parse_visibility(&x.vis)?;
    let mut representation = false;
    for attr in &x.attrs {
        if attr.path().is_ident("starlark") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("representation") {
                    representation = true;
                    Ok(())
                } else {
                    Err(meta.error("Expecting `#[starlark(representation)]`"))
                }
            })?;
        }
    }

    Ok(StarConst {
        name: x.ident,
        ty: *x.ty,
        value: *x.expr,
        representation,
    })
}

fn is_mut_something(x: &Type, smth: &str) -> bool {
    match x {
        Type::Reference(TypeReference {
            mutability: Some(_),
            elem: x,
            ..
        }) => is_type_name(x, smth),
        _ => false,
    }
}

pub(crate) fn is_ref_something(x: &Type, smth: &str) -> bool {
    match x {
        Type::Reference(TypeReference {
            mutability: None,
            elem: x,
            ..
        }) => is_type_name(x, smth),
        _ => false,
    }
}

// Is the type `&mut GlobalsBuilder`
fn is_mut_globals_builder(x: &Type) -> bool {
    is_mut_something(x, "GlobalsBuilder")
}

// Is the type `&mut MethodsBuilder`
fn is_mut_methods_builder(x: &Type) -> bool {
    is_mut_something(x, "MethodsBuilder")
}
