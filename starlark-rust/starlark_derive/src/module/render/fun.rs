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

use std::iter;

use proc_macro2::TokenStream;
use quote::quote;
use syn::Expr;
use syn::ExprLit;
use syn::Lit;

use crate::module::param_spec::ParamSpec;
use crate::module::render::render_none;
use crate::module::render::render_some;
use crate::module::simple_param::SimpleParam;
use crate::module::typ::RegularParams;
use crate::module::typ::SpecialParam;
use crate::module::typ::StarArg;
use crate::module::typ::StarArgSource;
use crate::module::typ::StarFun;
use crate::module::typ::StarFunSource;
use crate::module::util::ident_string;

impl StarFun {
    fn ty_custom_expr(&self) -> syn::Expr {
        match &self.starlark_ty_custom_function {
            Some(x) => syn::parse_quote! {
                std::option::Option::Some(starlark::typing::Ty::custom_function(#x))
            },
            None => {
                syn::parse_quote! {
                    std::option::Option::None
                }
            }
        }
    }

    fn special_builtin_function_expr(&self) -> syn::Expr {
        match &self.special_builtin_function {
            Some(x) => syn::parse_quote! {
                std::option::Option::Some(#x)
            },
            None => {
                syn::parse_quote! {
                    std::option::Option::None
                }
            }
        }
    }

    fn as_type_expr(&self) -> syn::Expr {
        match &self.as_type {
            Some(x) => syn::parse_quote! {
                    std::option::Option::Some((
                        <#x as starlark::values::StarlarkValue>::get_type_starlark_repr(),
                        starlark::docs::DocType::from_starlark_value::<#x>(),
                    ))
            },
            None => syn::parse_quote! {
                std::option::Option::None
            },
        }
    }

    /// Evaluator function parameter and call argument.
    fn eval_param_arg(&self) -> (Option<SimpleParam>, Option<syn::Expr>) {
        if let Some(SpecialParam { param }) = &self.eval {
            (
                Some(param.clone()),
                Some(syn::parse_quote! {
                    eval
                }),
            )
        } else {
            (None, None)
        }
    }

    /// Heap function parameter and call argument.
    fn heap_param_arg(&self) -> (Option<SimpleParam>, Option<syn::Expr>) {
        if let Some(SpecialParam { param }) = &self.heap {
            (
                Some(param.clone()),
                Some(syn::parse_quote! {
                    eval.heap()
                }),
            )
        } else {
            (None, None)
        }
    }

    /// `this` param if needed and call argument.
    fn this_param_arg(
        &self,
    ) -> (
        // Outer function parameter.
        Option<syn::FnArg>,
        // Inner function parameter.
        Option<SimpleParam>,
        Option<syn::Stmt>,
        Option<syn::Expr>,
    ) {
        match &self.this {
            Some(this) => {
                let outer_param_name: syn::Ident = syn::parse_quote! { s_this_value };
                let local_var: syn::Ident = syn::parse_quote! { s_this_typed };
                (
                    Some(syn::parse_quote! { #outer_param_name: starlark::values::Value<'v> }),
                    Some(this.param.clone()),
                    Some(this.render_prepare(&local_var, &outer_param_name)),
                    Some(syn::parse_quote! { #local_var }),
                )
            }
            None => (None, None, None, None),
        }
    }

    /// Non-special params.
    fn binding_params_arg(&self) -> syn::Result<(Vec<SimpleParam>, TokenStream, Vec<syn::Expr>)> {
        let Bindings { prepare, bindings } = render_binding(self)?;
        let binding_params: Vec<_> = bindings.iter().map(|b| b.param.clone()).collect();
        let binding_args: Vec<_> = bindings.iter().map(|b| b.render_arg()).collect();
        Ok((binding_params, prepare, binding_args))
    }

    fn name_str(&self) -> String {
        ident_string(&self.name)
    }

    /// Globals builder call to register the function.
    fn builder_set(&self) -> syn::Result<syn::Stmt> {
        let name_str = self.name_str();
        let components = render_native_callable_components(self)?;
        let param_spec = render_signature(self)?;

        let special_builtin_function = self.special_builtin_function_expr();

        if self.is_method() {
            if self.as_type.is_some() {
                return Err(syn::Error::new(
                    self.span(),
                    "methods cannot have an `as_type` attribute",
                ));
            }
            if self.starlark_ty_custom_function.is_some() {
                return Err(syn::Error::new(
                    self.span(),
                    "methods cannot have a `ty_custom_function` attribute",
                ));
            }
            Ok(syn::parse_quote! {
                #[allow(clippy::redundant_closure)]
                globals_builder.set_method(
                    #name_str,
                    #components,
                    |__heap| #param_spec,
                    __starlark_invoke_outer,
                );
            })
        } else {
            let as_type = self.as_type_expr();
            let ty_custom = self.ty_custom_expr();
            Ok(syn::parse_quote! {
                #[allow(clippy::redundant_closure)]
                globals_builder.set_function(
                    #name_str,
                    #components,
                    |__heap| #param_spec,
                    #as_type,
                    #ty_custom,
                    #special_builtin_function,
                    __starlark_invoke_outer,
                );
            })
        }
    }
}

pub(crate) fn render_fun(x: StarFun) -> syn::Result<syn::Stmt> {
    let (this_outer_param, this_inner_param, this_prepare, this_arg) = x.this_param_arg();
    let (eval_param, eval_arg) = x.eval_param_arg();
    let (heap_param, heap_arg) = x.heap_param_arg();
    let (binding_params, prepare, binding_args) = x.binding_params_arg()?;

    let builder_set = x.builder_set()?;

    let StarFun {
        attrs,
        return_type,
        body,
        ..
    } = x;

    let invoke_params: Vec<SimpleParam> = iter::empty()
        .chain(this_inner_param)
        .chain(binding_params)
        .chain(eval_param)
        .chain(heap_param)
        .collect();

    let invoke_args = iter::empty()
        .chain(this_arg)
        .chain(binding_args)
        .chain(eval_arg)
        .chain(heap_arg);

    let param_types: Vec<_> = invoke_params.iter().map(|p| &p.ty).collect();

    let this_outer_param = this_outer_param.into_iter();

    let item_invoke_impl: syn::ItemFn = syn::parse_quote! {
        // TODO(nga): copy lifetime parameter from declaration,
        //   so the warning would be precise.
        #[allow(clippy::extra_unused_lifetimes)]
        #( #attrs )*
        fn __starlark_invoke_impl<'v>(
            #( #invoke_params, )*
        ) -> #return_type {
            #body
        }
    };

    let item_return_type_repr: syn::ItemFn = syn::parse_quote! {
        // When function signature declares return type as `anyhow::Result<impl AllocValue>`,
        // we cannot call `T::starlark_type_repr` to render documentation, because there's no T.
        // Future Rust will provide syntax `type ReturnType = impl AllocValue`:
        // https://github.com/rust-lang/rfcs/pull/2515
        // Until then we use this hack as a workaround.
        #[allow(dead_code)] // Function is not used when return type is specified explicitly.
        fn __starlark_return_type_starlark_type_repr() -> starlark::typing::Ty {
            fn get_impl<'v, T: starlark::values::AllocValue<'v>, E>(
                _f: fn(
                    #( #param_types, )*
                ) -> std::result::Result<T, E>,
            ) -> starlark::typing::Ty {
                <<T as starlark::values::AllocValue<'v>>::Type as starlark::values::StarlarkTypeExpr<'v>>::starlark_type_repr()
            }
            get_impl(__starlark_invoke_impl)
        }
    };

    let impl_invoke_outer: syn::ItemFn = syn::parse_quote! {
        #[allow(non_snake_case)] // Starlark doesn't have this convention
        fn __starlark_invoke_outer<'v>(
            eval: &mut starlark::eval::Evaluator<'v, '_, '_>,
            #(#this_outer_param,)*
            signature: &starlark::eval::ParametersSpec<starlark::values::Value<'v>>,
            parameters: &starlark::eval::Arguments<'v, '_>,
        ) -> starlark::Result<starlark::values::Value<'v>> {
            #this_prepare
            #prepare
            match __starlark_invoke_impl(#( #invoke_args, )*) {
                Ok(v) => Ok(eval.heap().alloc(v)),
                Err(e) => Err(starlark::__derive_refs::invoke_macro_error::InvokeMacroError::into_starlark_error(e)),
            }
        }
    };

    Ok(syn::parse_quote! {
        {
            #item_invoke_impl
            #item_return_type_repr
            #impl_invoke_outer
            #builder_set
        }
    })
}

struct Bindings {
    prepare: TokenStream,
    bindings: Vec<BindingArg>,
}

// Given __args and __signature (if render_signature was Some)
// create bindings for all the arguments
fn render_binding(x: &StarFun) -> syn::Result<Bindings> {
    match (&x.args, &x.source) {
        (RegularParams::Arguments(arguments), StarFunSource::Arguments) => Ok(Bindings {
            prepare: TokenStream::new(),
            bindings: vec![BindingArg {
                param: arguments.param.clone(),
                expr: syn::parse_quote! { parameters },
            }],
        }),
        (RegularParams::Arguments(_), _) | (_, StarFunSource::Arguments) => Err(syn::Error::new(
            x.span(),
            "Inconsistent params/source (internal error)",
        )),
        (RegularParams::Unpack(args), StarFunSource::Signature { count }) => {
            let bind_args: Vec<BindingArg> = args
                .iter()
                .map(render_binding_arg)
                .collect::<syn::Result<_>>()?;
            Ok(Bindings {
                prepare: quote! {
                    let __args: [_; #count] =
                        starlark::__derive_refs::parse_args::parse_signature(
                            signature, parameters, eval.heap())?;
                },
                bindings: bind_args,
            })
        }
        (
            RegularParams::Unpack(args),
            StarFunSource::Positional {
                required,
                optional,
                kwargs: false,
            },
        ) => {
            let bind_args = args
                .iter()
                .map(render_binding_arg)
                .collect::<syn::Result<_>>()?;
            Ok(Bindings {
                prepare: quote! {
                    let (__required, __optional): ([_; #required], [_; #optional]) =
                        starlark::__derive_refs::parse_args::parse_positional(
                            &parameters, eval.heap())?;
                },
                bindings: bind_args,
            })
        }
        (
            RegularParams::Unpack(args),
            StarFunSource::Positional {
                required,
                optional,
                kwargs: true,
            },
        ) => {
            let bind_args = args
                .iter()
                .map(render_binding_arg)
                .collect::<syn::Result<_>>()?;
            Ok(Bindings {
                prepare: quote! {
                    let (__required, __optional, s_kwargs_value): ([_; #required], [_; #optional], _) =
                        starlark::__derive_refs::parse_args::parse_positional_kwargs_alloc(
                            &parameters, eval.heap())?;
                },
                bindings: bind_args,
            })
        }
    }
}

struct BindingArg {
    expr: syn::Expr,
    param: SimpleParam,
}

impl BindingArg {
    fn render_arg(&self) -> syn::Expr {
        self.expr.clone()
    }
}

/// Convert an expression of type `Value` to an expression of type of parameter.
fn render_unpack_value(value: syn::Expr, arg: &StarArg) -> syn::Expr {
    if arg.is_value() {
        // If we already have a `Value`, no need to unpack it.
        value
    } else {
        let name_str = ident_string(&arg.param.ident);
        syn::parse_quote! {
            starlark::__derive_refs::parse_args::check_unpack(#name_str, #value)?
        }
    }
}

/// Convert an expression of type `Option<Value>` to an expression of type of parameter.
fn render_unpack_option_value(option_value: syn::Expr, arg: &StarArg) -> syn::Expr {
    let name_str = ident_string(&arg.param.ident);
    if arg.is_option_value() {
        // If we already have a `Option<Value>`, no need to unpack it.
        option_value
    } else if arg.is_option() {
        syn::parse_quote! {
            starlark::__derive_refs::parse_args::check_optional(#name_str, #option_value)?
        }
    } else if arg.is_value() {
        // We call `check_required` even if `default` is set because for `Value`,
        // default is pulled into `ParametersSpec`.
        syn::parse_quote! {
            starlark::__derive_refs::parse_args::check_required(#name_str, #option_value)?
        }
    } else if let Some(default) = &arg.default {
        syn::parse_quote! {
            starlark::__derive_refs::parse_args::check_defaulted(#name_str, #option_value, || #default)?
        }
    } else {
        syn::parse_quote! {
            starlark::__derive_refs::parse_args::check_required(#name_str, #option_value)?
        }
    }
}

// Create a binding for an argument given. If it requires an index, take from the index
fn render_binding_arg(arg: &StarArg) -> syn::Result<BindingArg> {
    let next: syn::Expr = match &arg.source {
        StarArgSource::Argument(i) => {
            render_unpack_option_value(syn::parse_quote! { __args[#i] }, arg)
        }
        StarArgSource::Optional(i) => {
            render_unpack_option_value(syn::parse_quote! { __optional[#i] }, arg)
        }
        StarArgSource::Required(i) => {
            render_unpack_value(syn::parse_quote! { __required[#i] }, arg)
        }
        StarArgSource::Kwargs => render_unpack_value(syn::parse_quote! { s_kwargs_value }, arg),
        s => {
            return Err(syn::Error::new(
                arg.span,
                format!("Unexpected source {s:?} (internal error)"),
            ));
        }
    };

    Ok(BindingArg {
        expr: next,
        param: arg.param.clone(),
    })
}

// Given the arguments, create an expression building the `ParametersSpec` on the builder's
// heap, which is in scope as `__heap`.
fn render_signature(x: &StarFun) -> syn::Result<syn::Expr> {
    let name_str = ident_string(&x.name);

    match &x.args {
        RegularParams::Arguments(_) => Ok(syn::parse_quote! {
            starlark::__derive_refs::sig::parameter_spec_for_arguments(#name_str)
        }),
        RegularParams::Unpack(args) => {
            let ParametersSpecArgs {
                pos_only,
                pos_or_named,
                args,
                named_only,
                kwargs,
            } = parameter_spec_args(args)?;

            let pos_only: Vec<syn::Expr> =
                pos_only.iter().map(SignatureRegularArg::render).collect();
            let pos_or_named: Vec<syn::Expr> = pos_or_named
                .iter()
                .map(SignatureRegularArg::render)
                .collect();
            let named_only: Vec<syn::Expr> =
                named_only.iter().map(SignatureRegularArg::render).collect();

            Ok(syn::parse_quote! {
                starlark::__derive_refs::sig::parameter_spec(
                    #name_str,
                    &[#(#pos_only),*],
                    &[#(#pos_or_named),*],
                    #args,
                    &[#(#named_only),*],
                    #kwargs,
                )
            })
        }
    }
}

fn render_option(expr: Option<syn::Expr>) -> syn::Expr {
    match expr {
        Some(x) => render_some(x),
        None => render_none(),
    }
}

fn render_starlark_type(typ: &syn::Type) -> syn::Expr {
    syn::parse_quote! {
        {
            fn get_type_string<'v>() -> starlark::typing::Ty {
                <<#typ as starlark::values::UnpackValue<'v>>::Type as starlark::values::StarlarkTypeExpr<'v>>::starlark_type_repr()
            }
            get_type_string()
        }
    }
}

fn render_regular_native_callable_param(arg: &StarArg) -> syn::Result<syn::Expr> {
    let ty = render_starlark_type(arg.without_option());
    let name_str = ident_string(&arg.param.ident);
    let doc_hidden = arg.doc_hidden;
    let required: syn::Expr = match (&arg.default, arg.is_option()) {
        (Some(_), true) => {
            return Err(syn::Error::new(
                arg.span,
                "Option arguments cannot have defaults",
            ));
        }
        (None, true) => render_some(
            syn::parse_quote! { starlark::__derive_refs::param_spec::NativeCallableParamDefaultValue::Optional },
        ),
        (None, false) => render_none(),
        (Some(default), _) => {
            // For things that are type Value, we put them on the frozen heap.
            // For things that aren't type value, use optional and then next_opt/unwrap
            // to avoid the to/from value conversion.
            let default = if arg.is_value() {
                Some(syn::parse_quote! { |__heap| __heap.alloc(#default) })
            } else {
                render_default_as_value(default)
            };
            render_some(match default {
                None => {
                    syn::parse_quote! { starlark::__derive_refs::param_spec::NativeCallableParamDefaultValue::Optional }
                }
                Some(_) => {
                    syn::parse_quote! { starlark::__derive_refs::param_spec::NativeCallableParamDefaultValue::Value(#default) }
                }
            })
        }
    };

    Ok(syn::parse_quote! {
        {
            starlark::__derive_refs::static_str!(__PARAM_STATIC_NAME = #name_str);
            starlark::__derive_refs::param_spec::NativeCallableParam {
                name: __PARAM_STATIC_NAME,
                ty: #ty,
                required: #required,
                doc_hidden: #doc_hidden,
            }
        }
    })
}

fn render_native_callable_components(x: &StarFun) -> syn::Result<TokenStream> {
    let docs = match x.docstring.as_ref() {
        Some(d) => quote!(Some(#d)),
        None => quote!(None),
    };

    let param_spec: syn::Expr = match &x.args {
        RegularParams::Arguments(_) => {
            syn::parse_quote! {
                starlark::__derive_refs::param_spec::NativeCallableParamSpec::for_arguments()
            }
        }
        RegularParams::Unpack(args) => {
            let ParamSpec {
                pos_only,
                pos_or_named,
                args,
                named_only,
                kwargs,
            } = ParamSpec::split(args)?;

            let pos_only: Vec<syn::Expr> = pos_only
                .iter()
                .copied()
                .map(render_regular_native_callable_param)
                .collect::<syn::Result<Vec<_>>>()?;
            let pos_or_named: Vec<syn::Expr> = pos_or_named
                .iter()
                .copied()
                .map(render_regular_native_callable_param)
                .collect::<syn::Result<Vec<_>>>()?;
            let args: Option<syn::Expr> = args.map(|arg| {
                let name_str = ident_string(&arg.param.ident);
                let ty = render_starlark_type(&arg.param.ty);
                syn::parse_quote! {
                    {
                        starlark::__derive_refs::static_str!(__PARAM_STATIC_NAME = #name_str);
                        starlark::__derive_refs::param_spec::NativeCallableParam::args(__PARAM_STATIC_NAME, #ty)
                    }
                }
            });
            let named_only: Vec<syn::Expr> = named_only
                .iter()
                .copied()
                .map(render_regular_native_callable_param)
                .collect::<syn::Result<Vec<_>>>()?;
            let kwargs: Option<syn::Expr> = kwargs.map(|arg| {
                let name_str = ident_string(&arg.param.ident);
                let ty = render_starlark_type(&arg.param.ty);
                syn::parse_quote! {
                    {
                        starlark::__derive_refs::static_str!(__PARAM_STATIC_NAME = #name_str);
                        starlark::__derive_refs::param_spec::NativeCallableParam::kwargs(__PARAM_STATIC_NAME, #ty)
                    }
                }
            });

            let args = render_option(args);
            let kwargs = render_option(kwargs);
            syn::parse_quote! {
                starlark::__derive_refs::param_spec::NativeCallableParamSpec {
                    pos_only: vec![#(#pos_only),*],
                    pos_or_named: vec![#(#pos_or_named),*],
                    args: #args,
                    named_only: vec![#(#named_only),*],
                    kwargs: #kwargs,
                }
            }
        }
    };

    let speculative_exec_safe = x.speculative_exec_safe;
    Ok(quote!(
        {
            let param_spec = #param_spec;
            starlark::__derive_refs::components::NativeCallableComponents {
                speculative_exec_safe: #speculative_exec_safe,
                rust_docstring: #docs,
                param_spec,
                return_type: __starlark_return_type_starlark_type_repr(),
            }
        }
    ))
}

#[allow(clippy::large_enum_variant)]
enum SignatureRegularArgMode {
    Required,
    Optional,
    Defaulted(syn::Expr),
}

impl SignatureRegularArgMode {
    fn from_star_arg(arg: &StarArg) -> SignatureRegularArgMode {
        if arg.is_option() {
            SignatureRegularArgMode::Optional
        } else if let Some(default) = &arg.default {
            // For things that are type Value, we put them on the frozen heap.
            // For things that aren't type value, use optional and then next_opt/unwrap
            // to avoid the to/from value conversion.
            if arg.is_value() {
                SignatureRegularArgMode::Defaulted(syn::parse_quote! {
                    __heap.alloc(#default)
                })
            } else {
                SignatureRegularArgMode::Optional
            }
        } else {
            SignatureRegularArgMode::Required
        }
    }
}

/// Derive version of `NativeSigArg`.
struct SignatureRegularArg {
    name: String,
    mode: SignatureRegularArgMode,
}

impl SignatureRegularArg {
    fn from_star_arg(arg: &StarArg) -> SignatureRegularArg {
        SignatureRegularArg {
            name: ident_string(&arg.param.ident),
            mode: SignatureRegularArgMode::from_star_arg(arg),
        }
    }

    fn render(&self) -> syn::Expr {
        let name_str = &self.name;
        match &self.mode {
            SignatureRegularArgMode::Required => {
                syn::parse_quote! { starlark::__derive_refs::sig::NativeSigArg::Required(#name_str) }
            }
            SignatureRegularArgMode::Optional => {
                syn::parse_quote! { starlark::__derive_refs::sig::NativeSigArg::Optional(#name_str) }
            }
            SignatureRegularArgMode::Defaulted(value) => {
                syn::parse_quote! { starlark::__derive_refs::sig::NativeSigArg::Defaulted(#name_str, #value) }
            }
        }
    }
}

/// Arguments to pass to `parameter_spec` to render `ParametersSpec`.
struct ParametersSpecArgs {
    pos_only: Vec<SignatureRegularArg>,
    pos_or_named: Vec<SignatureRegularArg>,
    /// `*args`.
    args: bool,
    named_only: Vec<SignatureRegularArg>,
    /// `**kwargs`.
    kwargs: bool,
}

/// Return the number of positional and positional-only arguments.
fn parameter_spec_args(star_args: &[StarArg]) -> syn::Result<ParametersSpecArgs> {
    let ParamSpec {
        pos_only,
        pos_or_named,
        args,
        named_only,
        kwargs,
    } = ParamSpec::split(star_args)?;

    let pos_only = pos_only
        .iter()
        .map(|a| SignatureRegularArg::from_star_arg(a))
        .collect();
    let pos_or_named = pos_or_named
        .iter()
        .map(|a| SignatureRegularArg::from_star_arg(a))
        .collect();
    let args = args.is_some();
    let named_only = named_only
        .iter()
        .map(|a| SignatureRegularArg::from_star_arg(a))
        .collect();
    let kwargs = kwargs.is_some();

    Ok(ParametersSpecArgs {
        pos_only,
        pos_or_named,
        args,
        named_only,
        kwargs,
    })
}

/// We have an argument that the user wants to use as a default.
/// That _might_ have a valid Starlark value representation, if so, it would be great to use for
/// documentation. Try and synthesise it if we can, as a function of the builder's heap.
fn render_default_as_value(default: &Expr) -> Option<syn::Expr> {
    let x = quote!(#default).to_string();
    if let Ok(x) = x.trim_end_matches("i32").parse::<i32>() {
        Some(syn::parse_quote! { |__heap| __heap.alloc(#x) })
    } else if let Ok(x) = x.parse::<bool>() {
        Some(syn::parse_quote! { |_| starlark::values::Value::new_bool(#x) })
    } else if x == "NoneOr :: None" {
        Some(syn::parse_quote! { |_| starlark::values::Value::new_none() })
    } else if matches!(
        default,
        Expr::Lit(ExprLit {
            lit: Lit::Str(_),
            ..
        })
    ) {
        // Make sure we don't splice in `x` again, or we double quote the string
        Some(syn::parse_quote! { |__heap| __heap.alloc(#default) })
    } else if x == "ValueOf :: empty_list()" {
        Some(syn::parse_quote! { |_| starlark::values::Value::new_empty_list() })
    } else if x == "ValueOf :: empty_dict()" {
        Some(syn::parse_quote! { |_| starlark::values::Value::new_empty_dict() })
    } else {
        None
    }
}
