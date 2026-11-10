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

use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::ToTokens;
use quote::format_ident;
use quote::quote;

use crate::module::render::fun::render_fun;
use crate::module::simple_param::SimpleParam;
use crate::module::typ::SpecialParam;
use crate::module::typ::StarAttr;
use crate::module::typ::StarConst;
use crate::module::typ::StarGenerics;
use crate::module::typ::StarModule;
use crate::module::typ::StarStmt;
use crate::module::typ::StarTypeEntry;
use crate::module::util::ident_string;

/// Generate the static variable name for a StarlarkValueAsType constant.
/// Uses uppercase to avoid `non_upper_case_globals` warnings.
fn starlark_type_static_name(name: &str) -> syn::Ident {
    format_ident!("__STARLARK_TYPE_{}", name.to_ascii_uppercase())
}

pub(crate) fn render(x: StarModule) -> syn::Result<TokenStream> {
    // Generate declare_starlark_value_as_type! calls for each #[starlark_types] entry.
    // Deduplicate by rust_type: only the first occurrence of each Rust type
    // gets the full declaration (which includes the AsTypeStaticRegistered trait impl).
    // Subsequent occurrences with the same Rust type use `skip_type_registration`
    // to avoid conflicting trait implementations.
    let mut seen_rust_types = HashSet::new();
    let type_declarations: Vec<TokenStream> = x
        .starlark_types
        .iter()
        .map(|entry| {
            let is_first = seen_rust_types.insert(entry.rust_type.to_token_stream().to_string());
            render_starlark_type_declaration(entry, is_first)
        })
        .collect();

    let body = render_impl(x)?;

    Ok(quote! {
        #( #type_declarations )*
        #body
    })
}

/// Generate a `declare_starlark_value_as_type!` call for a `#[starlark_types]` entry.
///
/// When `register_type` is true, the full macro is used (including the
/// `AsTypeStaticRegistered` trait impl). When false, the `skip_type_registration`
/// variant is used to avoid duplicate trait implementations for the same Rust type.
fn render_starlark_type_declaration(entry: &StarTypeEntry, register_type: bool) -> TokenStream {
    let starlark_name_str = ident_string(&entry.starlark_name);
    let static_name = starlark_type_static_name(&starlark_name_str);
    let rust_type = &entry.rust_type;
    match (register_type, entry.no_docs) {
        (true, false) => quote! {
            starlark::declare_starlark_value_as_type!(#static_name, #rust_type);
        },
        (true, true) => quote! {
            starlark::declare_starlark_value_as_type!(#static_name, #rust_type, no_docs);
        },
        (false, false) => quote! {
            starlark::declare_starlark_value_as_type!(#static_name, #rust_type, skip_type_registration);
        },
        (false, true) => quote! {
            starlark::declare_starlark_value_as_type!(#static_name, #rust_type, skip_type_registration_no_docs);
        },
    }
}

fn render_impl(x: StarModule) -> syn::Result<TokenStream> {
    let StarModule {
        mut input,
        docstring,
        stmts,
        module_kind,
        generics,
        starlark_types,
    } = x;
    let statics_macro = format_ident!("{}", module_kind.statics_macro_name());
    let stmts: Vec<_> = stmts
        .into_iter()
        .map(|s| render_stmt(s, &generics))
        .collect::<syn::Result<_>>()?;
    let set_docstring = docstring.map(|ds| quote!(globals_builder.set_docstring(#ds);));

    // Generate globals_builder.set() calls for #[starlark_types] entries
    let type_set_stmts: Vec<syn::Stmt> = starlark_types
        .iter()
        .map(render_starlark_type_set)
        .collect();

    let inner_fn = syn::ItemFn {
        attrs: Default::default(),
        vis: syn::Visibility::Inherited,
        modifiers: syn::FnModifiers::default(),
        sig: syn::Signature {
            ident: syn::Ident::new("build", input.sig.ident.span()),
            ..input.sig.clone()
        },
        block: syn::parse_quote! {
            {
                #set_docstring
                #( #type_set_stmts )*
                #( #stmts )*
                // Mute warning if stmts is empty.
                let _ = globals_builder;
            }
        },
    };
    let fn_name = &input.sig.ident;
    let statics_name = format_ident!("{}_STATICS", fn_name.to_string().to_uppercase());
    input.block = syn::parse_quote! {
        {
            #statics_name.populate(globals_builder);
        }
    };

    Ok(quote! {
        starlark::#statics_macro!(#statics_name = {
            #inner_fn
            build
        });

        #input
    })
}

fn render_stmt(x: StarStmt, generics: &StarGenerics) -> syn::Result<syn::Stmt> {
    match x {
        StarStmt::Const(x) => Ok(render_const(x)),
        StarStmt::Attr(x) => Ok(render_attr(x, generics)),
        StarStmt::Fun(x) => render_fun(x),
    }
}

fn render_const(x: StarConst) -> syn::Stmt {
    let StarConst {
        name,
        ty,
        value,
        representation,
    } = x;
    let name_str = ident_string(&name);
    if representation {
        syn::parse_quote! {
            globals_builder.set_representation::<#ty>(#name_str, #value);
        }
    } else {
        syn::parse_quote! {
            globals_builder.set::<#ty>(#name_str, #value);
        }
    }
}

/// Generate a `globals_builder.set()` call for a `#[starlark_types]` entry.
fn render_starlark_type_set(entry: &StarTypeEntry) -> syn::Stmt {
    let starlark_name_str = ident_string(&entry.starlark_name);
    let static_name = starlark_type_static_name(&starlark_name_str);
    let rust_type = &entry.rust_type;
    syn::parse_quote! {
        globals_builder.set::<starlark::__derive_refs::StarlarkValueAsType<#rust_type>>(
            #starlark_name_str,
            #static_name,
        );
    }
}

fn render_attr(x: StarAttr, generics: &StarGenerics) -> syn::Stmt {
    let StarAttr {
        name,
        this,
        heap,
        attrs,
        return_type,
        speculative_exec_safe,
        body,
        docstring,
    } = x;
    let name_str = ident_string(&name);
    let name_inner = syn::Ident::new(&format!("{name_str}__inner"), name.span());
    let docstring: syn::Expr = match docstring {
        Some(d) => render_some(syn::parse_quote! { #d.to_owned() }),
        None => render_none(),
    };

    let generic_decls = generics.decls();
    let where_clause = generics.where_clause();
    let turbofish = generics.turbofish();

    let let_heap = if let Some(SpecialParam {
        param: SimpleParam { ident, ty, .. },
    }) = heap
    {
        Some(quote! { let #ident: #ty = __heap; })
    } else {
        None
    };

    let this_value: syn::Ident = syn::parse_quote! { s_this_value };
    let this_return_type: &syn::Type = &this.param.ty;

    let unpack = this.render_prepare(&this.param.ident, &this_value);

    let inner: syn::ItemFn = syn::parse_quote! {
        #( #attrs )*
        #[allow(non_snake_case)] // Starlark doesn't have this convention
        #[allow(unused_variables)]
        fn #name_inner #generic_decls(
            this: #this_return_type,
            __heap: starlark::values::Heap<'v>,
        ) -> #return_type
        #where_clause
        {
            #let_heap
            #body
        }
    };

    let outer: syn::ItemFn = syn::parse_quote! {
        #[allow(non_snake_case)]
        fn #name #generic_decls(
            _ignored: std::option::Option<starlark::values::Value<'v>>,
            #this_value: starlark::values::Value<'v>,
            heap: starlark::values::Heap<'v>,
        ) -> starlark::Result<starlark::values::Value<'v>>
        #where_clause
        {
            #unpack
            Ok(heap.alloc(#name_inner #turbofish(this, heap)?))
        }
    };

    syn::parse_quote! {
        {
            #inner
            #outer

            globals_builder.set_attribute_fn(
                #name_str,
                #speculative_exec_safe,
                #docstring,
                starlark::values::type_repr::type_repr_from_attr_impl(#name_inner #turbofish),
                #name #turbofish
            );
        }
    }
}

pub(crate) fn render_none() -> syn::Expr {
    syn::parse_quote! { std::option::Option::None }
}

pub(crate) fn render_some(expr: syn::Expr) -> syn::Expr {
    syn::parse_quote! { std::option::Option::Some(#expr) }
}
