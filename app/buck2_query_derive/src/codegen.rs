/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use proc_macro2::Span;
use proc_macro2::TokenStream;
use quote::quote;
use quote::quote_spanned;
use syn::Ident;
use syn::Path;
use syn::Type;
use syn::TypePath;
use syn::TypeReference;

use crate::parse::Argument;
use crate::parse::DocString;
use crate::parse::Method;
use crate::parse::Parsed;

struct MethodCodegen {
    method_def: TokenStream,
    method_describe: TokenStream,
    method_dispatch: Option<TokenStream>,
    op_dispatch: Option<TokenStream>,
}

enum ContextArgument {
    None,
    Evaluator,
    Environment,
}

fn is_type_name(x: &Type, name: &str) -> bool {
    if let Type::Path(TypePath {
        path: Path { segments, .. },
        ..
    }) = x
    {
        if let Some(seg1) = segments.last() {
            return seg1.ident == name;
        }
    }
    false
}

fn is_ref_ctx_type_name(x: &Type, name: &str) -> syn::Result<bool> {
    match x {
        Type::Reference(TypeReference {
            mutability: Some(_),
            ..
        }) => Err(syn::Error::new_spanned(
            x,
            "#[query_module] context args (QueryEvaluator/QueryEnvironment) should not be mutable",
        )),
        Type::Reference(TypeReference {
            mutability: None,
            elem: x,
            ..
        }) => Ok(is_type_name(x, name)),
        _ if is_type_name(x, name) => Err(syn::Error::new_spanned(
            x,
            "#[query_module] context args should be taken by reference, not value",
        )),
        _ => Ok(false),
    }
}

fn is_evaluator_arg(ty: &Type) -> syn::Result<bool> {
    is_ref_ctx_type_name(ty, "QueryEvaluator")
}

fn is_env_arg(ty: &Type, env_ty: &Type) -> syn::Result<bool> {
    match ty {
        Type::Reference(TypeReference {
            mutability: Some(_),
            ..
        }) => Err(syn::Error::new_spanned(
            ty,
            "#[query_module] context args (QueryEvaluator/QueryEnvironment) should not be mutable",
        )),
        Type::Reference(TypeReference {
            mutability: None,
            elem: x,
            ..
        }) => Ok(env_ty == &**x),
        _ if env_ty == ty => Err(syn::Error::new_spanned(
            ty,
            "#[query_module] context args should be taken by reference, not value",
        )),
        _ => Ok(false),
    }
}

fn ctx_arg_for(arg: &Argument, env_ty: &Type) -> syn::Result<ContextArgument> {
    Ok(match arg {
        Argument { ty, .. } if is_evaluator_arg(ty)? => ContextArgument::Evaluator,
        Argument { ty, .. } if is_env_arg(ty, env_ty)? => ContextArgument::Environment,
        _ => ContextArgument::None,
    })
}

fn analyze_args<'a>(
    method: &'a Method,
    env_ty: &Type,
) -> syn::Result<(ContextArgument, Vec<&'a Argument>)> {
    let mut args = method.args.iter().peekable();
    let ctx_arg = match args.peek() {
        Some(v) => ctx_arg_for(v, env_ty)?,
        None => ContextArgument::None,
    };
    match ctx_arg {
        ContextArgument::None => {}
        _ => {
            args.next();
        }
    };

    Ok((ctx_arg, args.map(|a| match ctx_arg_for(a, env_ty)? {
        ContextArgument::None => {Ok(a)},
        _ => {
            Err(syn::Error::new(a.span, "context arguments (QueryEvaluator/QueryEnvironment) can only be the first non-receiver arg"))
        }
    }).collect::<syn::Result<Vec<_>>>()?))
}

fn gen_for_docstring(span: Span, docs: Option<&DocString>) -> TokenStream {
    match &docs {
        Some(DocString {
            short_help,
            details,
        }) => quote_spanned!(span =>
            short_help: Some(#short_help.to_owned()),
            details: Some(#details.to_owned())
        ),
        None => quote_spanned!(span =>
            short_help: None,
            details: None
        ),
    }
}

fn gen_for_method(parsed: &Parsed, method: &Method) -> syn::Result<MethodCodegen> {
    let func_ty = Ident::new(
        &format!("__{}QueryFunction", method.name),
        method.name.span(),
    );

    let func_ident = &method.name;
    let func_ident_str = func_ident.to_string();
    let method_doc = gen_for_docstring(method.name.span(), method.docs.as_ref());
    let env_ident = &parsed.args.env_ty;
    let env_target = quote! {<#env_ident as QueryEnvironment>::Target};
    let self_ty = &parsed.module.self_ty;
    let (impl_generics, ty_generics, where_clause) = parsed.module.generics.split_for_impl();

    let (ctx_arg, value_args) = analyze_args(method, &parsed.args.env_ty)?;

    let pass_ctx = match ctx_arg {
        ContextArgument::None => quote! {},
        ContextArgument::Evaluator => quote! {evaluator,},
        ContextArgument::Environment => quote! {evaluator.env(),},
    };

    let mut describe_args = Vec::new();
    let mut pass_args = Vec::new();
    let mut arg_type_match = Vec::new();
    let mut arg_name_match = Vec::new();
    for (i, arg) in value_args.iter().enumerate() {
        let arg_type = &arg.ty;
        let as_arg_type = quote! {<#arg_type as QueryFunctionArg<'_, #env_ident>>};
        let arg_name = arg.name.to_string();
        arg_type_match.push(quote_spanned!(arg.span => #i => Ok(#as_arg_type::ARG_TYPE)));
        arg_name_match.push(quote_spanned!(arg.span => #i => Ok(#arg_name)));
        pass_args.push(quote_spanned!(arg.span => eval_arg(self, evaluator, args, #i).await?));
        describe_args.push(quote_spanned!(arg.span => ArgDescription {
            name: #arg_name.to_owned(),
            repr_format: #as_arg_type::describe_format(),
            arg_type: #as_arg_type::ARG_TYPE
        }))
    }

    let method_describe = quote! {
        #func_ident_str => FunctionDescription {
            name: #func_ident_str,
            args: vec![
                #(#describe_args,)*
            ],
            #method_doc,
        }
    };

    match &method.binary_op {
        Some(op) => {
            if value_args.len() != 2 {
                return Err(syn::Error::new(
                    method.name.span(),
                    "binary ops must take exactly two arguments",
                ));
            }

            let op_dispatch = Some(quote_spanned!(method.name.span() =>
                #op => Some(#func_ty::ref_cast(self) as &dyn QueryBinaryOp<#env_ident>)
            ));

            let struct_def: syn::ItemStruct = syn::parse_quote_spanned! { method.name.span() =>
                #[derive(RefCast)]
                #[repr(transparent)]
                struct #func_ty #impl_generics (#self_ty) #where_clause;
            };

            let impl_def: syn::ItemImpl = syn::parse_quote_spanned! { method.name.span() =>
                #[async_trait]
                impl #impl_generics QueryBinaryOp<#env_ident> for #func_ty #ty_generics #where_clause {
                    fn name(&self) -> &'static str { #func_ident_str }

                    async fn invoke(
                        &self,
                        env: &Env,
                        left: QueryValue<#env_target>,
                        right: QueryValue<#env_target>,
                    ) -> Result<QueryValue<#env_target>, QueryError> {
                        self.0.#func_ident(
                            env,
                            left,
                            right
                        ).await
                    }
                }
            };

            let method_def = quote_spanned!(method.name.span() =>
                #struct_def
                #impl_def
            );

            Ok(MethodCodegen {
                method_def,
                method_describe,
                method_dispatch: None,
                op_dispatch,
            })
        }
        None => {
            let max_args = value_args.len();

            let method_dispatch = Some(quote_spanned!(method.name.span() =>
                stringify!(#func_ident) => Some(#func_ty::ref_cast(self) as &dyn QueryFunction<#env_ident>)
            ));

            let struct_def: syn::ItemStruct = syn::parse_quote_spanned! { method.name.span() =>
                #[derive(RefCast)]
                #[repr(transparent)]
                struct #func_ty #impl_generics (#self_ty) #where_clause;
            };

            let impl_ref: syn::ItemImpl = syn::parse_quote_spanned! { method.name.span() =>
                #[async_trait]
                impl #impl_generics QueryFunction<#env_ident> for #func_ty #ty_generics #where_clause {
                    fn name(&self) -> &'static str { stringify!(#func_ident) }

                    async fn invoke(
                        &self,
                        evaluator: &QueryEvaluator<#env_ident>,
                        args: &[SpannedExpr<'_>],
                    ) -> Result<QueryValue<#env_target>, QueryError> {
                        if args.len() > #max_args {
                            return Err(QueryError::TooManyArgs {
                                function: self.name().to_owned(),
                                max: #max_args,
                                actual: args.len(),
                            });
                        }
                        self.0.#func_ident(
                            #pass_ctx
                            #(#pass_args,)*
                        ).await
                    }

                    fn arg_type(&self, idx: usize) -> Result<QueryArgType, QueryError> {
                        match idx {
                            #(#arg_type_match,)*
                            v => Err(QueryError::TooManyArgs {
                                function: self.name().to_owned(),
                                max: #max_args,
                                actual: idx + 1,
                            })
                        }
                    }

                    fn arg_name(&self, idx: usize) -> Result<&'static str, QueryError> {
                        match idx {
                            #(#arg_name_match,)*
                            v => Err(QueryError::TooManyArgs {
                                function: self.name().to_owned(),
                                max: #max_args,
                                actual: idx + 1,
                            })
                        }
                    }
                }
            };

            let method_def = quote_spanned!(method.name.span() =>
                #struct_def
                #impl_ref
            );

            Ok(MethodCodegen {
                method_def,
                method_describe,
                method_dispatch,
                op_dispatch: None,
            })
        }
    }
}

pub(crate) fn codegen(parsed: Parsed) -> syn::Result<TokenStream> {
    let mod_name = quote!(__buck_query_gen);
    let mut methods = Vec::new();
    for method in &parsed.module.methods {
        methods.push(gen_for_method(&parsed, method)?);
    }

    let method_defs = methods.iter().map(|v| &v.method_def);
    let method_describe = methods.iter().map(|v| &v.method_describe);
    let method_dispatch = methods
        .iter()
        .map(|v| &v.method_dispatch)
        .filter_map(|v| v.as_ref());
    let op_dispatch = methods
        .iter()
        .map(|v| &v.op_dispatch)
        .filter_map(|v| v.as_ref());

    let (impl_generics, _ty_generics, where_clause) = parsed.module.generics.split_for_impl();
    let self_ty = &parsed.module.self_ty;
    let env_ty = &parsed.args.env_ty;

    let modified_original = &parsed.module.original_impl;
    let module_docs = gen_for_docstring(Span::call_site(), parsed.module.docs.as_ref());

    let result = quote! {
        #modified_original

        mod #mod_name {
            #![allow(non_camel_case_types)]
            #![allow(unused)]
            #![allow(clippy::match_single_binding)]

            use ::buck2_query::__derive_refs::ref_cast::RefCast;
            use ::buck2_query::__derive_refs::buck2_query_parser::{BinaryOp, SpannedExpr};
            use ::buck2_query::__derive_refs::buck2_hash::buck_mut_insertion_map;
            use ::buck2_query::__derive_refs::async_trait::async_trait;

            use super::*;
            use ::buck2_query::query::{
                environment::{QueryEnvironment, QueryTarget},
                syntax::simple::{
                    eval::{error::QueryError, evaluator::QueryEvaluator, values::QueryValue},
                    functions::docs::{ModuleDescription, ArgDescription, FunctionDescription},
                    functions::{QueryFunctions, HasModuleDescription},
                    functions::helpers::{
                        eval_arg, QueryArgType, QueryBinaryOp, QueryFunction, QueryFunctionArg,
                    },
                },
            };

            #(#method_defs)*

            impl #impl_generics QueryFunctions for #self_ty #where_clause {
                type Env = #env_ty;
                fn get(&self, name: &str) -> Option<&dyn QueryFunction<#env_ty>> {
                    match name {
                        #(#method_dispatch,)*
                        _ => None
                    }
                }

                fn get_op(&self, op: BinaryOp) -> Option<&dyn QueryBinaryOp<#env_ty>> {
                    match op {
                        #(#op_dispatch,)*
                        _ => None
                    }
                }
            }

            impl #impl_generics HasModuleDescription for #self_ty #where_clause {
                fn describe() -> ModuleDescription {
                    ModuleDescription {
                        functions: buck_mut_insertion_map! {
                                #(#method_describe,)*
                        },
                        #module_docs,
                    }
                }
            }

        }

    };

    Ok(result)
}
