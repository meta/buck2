/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use itertools::Itertools;
use proc_macro2::Span;
use proc_macro2::TokenStream;
use quote::ToTokens as _;
use syn::Attribute;
use syn::Error;
use syn::Expr;
use syn::ExprLit;
use syn::FnArg;
use syn::Generics;
use syn::Ident;
use syn::ImplItem;
use syn::ImplItemFn;
use syn::Item;
use syn::ItemImpl;
use syn::Lit;
use syn::Meta;
use syn::MetaNameValue;
use syn::Pat;
use syn::PatIdent;
use syn::PatType;
use syn::Path;
use syn::Safety;
use syn::Signature;
use syn::Type;
use syn::parse::Result;
use syn::spanned::Spanned;

/// Validates and parses the macro input into structured data.
pub(crate) fn parse(args: TokenStream, item: TokenStream) -> Result<Parsed> {
    // Parse input first so that any error with it takes precedence.
    let module = syn::parse2::<Module>(item)?;
    let args = syn::parse2::<QueryModuleArgs>(args)?;

    Ok(Parsed { module, args })
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct DocString {
    pub short_help: String,
    pub details: String,
}

impl DocString {
    fn parse(attrs: &[syn::Attribute]) -> Option<Self> {
        let mut docs = Vec::new();
        for attr in attrs {
            if attr.path().is_ident("doc") {
                if let Meta::NameValue(MetaNameValue {
                    value:
                        Expr::Lit(ExprLit {
                            lit: Lit::Str(s), ..
                        }),
                    ..
                }) = &attr.meta
                {
                    docs.push(s.value());
                }
            }
        }
        if docs.is_empty() {
            None
        } else {
            let mut lines = docs.into_iter();
            let short_help = lines.next().unwrap().trim().to_owned();
            let mut lines = lines.skip_while(|v| v.is_empty());

            let details = lines.join("\n");
            let details = textwrap::dedent(&details);
            Some(Self {
                short_help,
                details,
            })
        }
    }
}

#[derive(Debug)]
pub(crate) struct Parsed {
    pub module: Module,
    pub args: QueryModuleArgs,
}

#[derive(Debug)]
pub(crate) struct QueryModuleArgs {
    pub env_ty: Type,
}

#[derive(Debug)]
pub(crate) struct Argument {
    pub name: Ident,
    pub span: Span,
    pub ty: Type,
}

#[derive(Debug)]
pub(crate) struct Method {
    pub docs: Option<DocString>,
    pub name: Ident,
    pub args: Vec<Argument>,
    pub binary_op: Option<Path>,
}

#[derive(Debug)]
pub(crate) struct Module {
    pub docs: Option<DocString>,
    pub generics: Generics,
    /// The Self type of the impl.
    pub self_ty: Box<Type>,
    pub methods: Vec<Method>,

    /// Holds the original annotated impl, with any attributes we consumed removed.
    pub original_impl: ItemImpl,
}

impl syn::parse::Parse for Module {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let span = input.span();
        let mut item = match input.parse::<Item>() {
            Ok(Item::Impl(item)) => item,
            Ok(item) => {
                return Err(Error::new_spanned(
                    item,
                    "`#[query_module(..)]` can only be used on impls",
                ));
            }
            Err(e) => return Err(Error::new(span, format!("{e}"))),
        };

        let ItemImpl {
            // Should we care if there are other attributes?
            attrs,
            modifiers,
            unsafety,
            impl_token: _impl_token,
            generics,
            trait_,
            self_ty,
            brace_token: _brace_token,
            items,
        } = &mut item;

        if let Some(v) = modifiers.defaultness {
            return Err(Error::new_spanned(
                v,
                "`#[query_module]` can't handle `default` in impl item",
            ));
        }
        if let Some(v) = unsafety {
            return Err(Error::new_spanned(
                v,
                "`#[query_module]` can't handle `unsafe` in impl item",
            ));
        }
        if trait_.is_some() {
            return Err(Error::new_spanned(
                self_ty,
                "`#[query_module]` should only be used on an inherent impl, not a trait impl",
            ));
        }
        modifiers.require_empty()?;

        let docs = DocString::parse(attrs);

        let mut methods = Vec::new();
        let context = ParseFunctionContext {
            binary_op_ident: Ident::new("binary_op", Span::call_site()),
        };
        for item in items {
            match item {
                ImplItem::Fn(method) => {
                    let function = parse_function(method, &context)?;
                    methods.push(function);
                }
                _ => {
                    return Err(Error::new_spanned(
                        item,
                        "`#[query_module]` only supports methods within the annotated impl",
                    ));
                }
            };
        }

        Ok(Self {
            docs,
            generics: generics.clone(),
            self_ty: self_ty.clone(),
            methods,
            original_impl: item,
        })
    }
}

struct ParseFunctionContext {
    binary_op_ident: Ident,
}

fn parse_function(method: &mut ImplItemFn, context: &ParseFunctionContext) -> Result<Method> {
    let sig = &method.sig;

    let Safety::Default = sig.safety else {
        return Err(Error::new_spanned(
            &sig.safety,
            format!(
                "`#[query_module]` methods can't be {}.",
                sig.safety.to_token_stream(),
            ),
        ));
    };

    if sig.asyncness.is_none() {
        return Err(Error::new_spanned(
            sig,
            "`#[query_module]` methods must be async.",
        ));
    }

    let parsed_args = parse_function_args(sig)?;

    let docs = DocString::parse(&method.attrs);

    let original_attrs = std::mem::take(&mut method.attrs);
    let (filtered_attrs, ExtraFunctionAttributes { binary_op }) =
        process_function_attributes(original_attrs, context)?;
    method.attrs = filtered_attrs;

    Ok(Method {
        name: sig.ident.clone(),
        args: parsed_args,
        binary_op,
        docs,
    })
}

struct ExtraFunctionAttributes {
    binary_op: Option<Path>,
}

fn process_function_attributes(
    attrs: Vec<Attribute>,
    context: &ParseFunctionContext,
) -> Result<(Vec<Attribute>, ExtraFunctionAttributes)> {
    // We look for a #[binary_op(BinaryOp::Foo)] attribute to identify functions that implement binary ops. We need to also remove that attribute from the original code.
    let mut binary_op = None;

    let mut filtered_attrs = Vec::with_capacity(attrs.len());
    for attr in attrs {
        if attr.path().is_ident(&context.binary_op_ident) {
            if let Ok(path) = attr.parse_args::<Path>() {
                binary_op = Some(path);
            } else {
                return Err(Error::new_spanned(
                    &attr.meta,
                    "#[query_module] attribute `binary_op` should receive a single argument identifying the binary op like `#[binary_op(BinaryOp::Intersect)]`",
                ));
            }
        } else {
            filtered_attrs.push(attr)
        }
    }
    Ok((filtered_attrs, ExtraFunctionAttributes { binary_op }))
}

fn parse_function_args(sig: &Signature) -> Result<Vec<Argument>> {
    let mut args = sig.inputs.iter();
    match args.next() {
        Some(FnArg::Receiver(_)) => {}
        _ => {
            return Err(Error::new(
                sig.span(),
                "`#[query_module]` methods must have a `&self` parameter",
            ));
        }
    }

    let mut parsed_args = Vec::new();
    for arg in args {
        let span = arg.span();
        match arg {
            FnArg::Receiver(_) => {
                unreachable!("a method can't have multiple receiver args")
            }
            FnArg::Typed(PatType { pat, ty, .. }) => {
                let name = match &**pat {
                    Pat::Ident(PatIdent { ident, .. }) => ident,
                    _ => {
                        return Err(Error::new(
                            span,
                            "`#[query_module]` method arguments can only have simple identity captures. You can destructure them within the method impl.",
                        ));
                    }
                };
                parsed_args.push(Argument {
                    span,
                    ty: (**ty).clone(),
                    name: name.clone(),
                })
            }
        }
    }
    Ok(parsed_args)
}

impl syn::parse::Parse for QueryModuleArgs {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        const ARGS_ERROR: &str = "this attribute takes a single argument, the identifier for the QueryEnvironment type. use `#[query_module(Env)]`, for example";
        let span = input.span();

        let env_ty: Type = match input.parse() {
            Ok(v) => v,
            Err(_) => return Err(Error::new(span, ARGS_ERROR)),
        };

        Ok(QueryModuleArgs { env_ty })
    }
}

#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::Generics;
    use syn::Type;
    use syn::parse_quote;

    use crate::parse::DocString;
    use crate::parse::Parsed;

    #[test]
    fn test() {
        let args = quote! {Env};

        let item = quote! {
            /// module-level docs line 1
            ///
            /// module-level docs line 2
            impl <Env: QueryEnvironment> MySimpleFunctions<Env> {
                /// buildfile docs
                async fn buildfile(&self, targets: TargetSet<Env::Target>) -> QueryFuncResult<Env> {
                    Ok(targets.buildfile()?.into())
                }

                /// deps docs
                async fn deps(
                    &self,
                    evaluator: &QueryEvaluator<'_, Env>,
                    targets: TargetSet<Env::Target>,
                    depth: QueryValueDepth,
                    captured_expr: Option<CapturedExpr<'_>>,
                ) -> QueryFuncResult<Env> {
                    DepsFunction::<Env> {
                        _marker: PhantomData,
                    }
                    .invoke_deps(evaluator, targets, depth, captured_expr)
                    .await
                }

                /// filter docs
                async fn filter(&self, regex: String, targets: TargetSet<Env::Target>) -> QueryFuncResult<Env> {
                    Ok(targets.filter_name(&regex)?.into())
                }
            }
        };

        let Parsed { module, .. } = super::parse(args, item).unwrap();

        assert_eq!(
            &Some(DocString {
                short_help: "module-level docs line 1".to_owned(),
                details: "module-level docs line 2".to_owned()
            }),
            &module.docs
        );

        let expected: Generics = parse_quote!(<Env: QueryEnvironment>);
        assert_eq!(&expected, &module.generics);
        let expected: Type = parse_quote!(MySimpleFunctions<Env>);
        assert_eq!(&expected, &*module.self_ty);

        assert_eq!(3, module.methods.len());

        let method = module.methods.first().unwrap();
        assert_eq!("buildfile", method.name.to_string());
        let args = &method.args;
        assert_eq!(1, args.len());
        let expected: Type = parse_quote!(TargetSet<Env::Target>);
        assert_eq!(&expected, &args.first().unwrap().ty);

        let method = module.methods.get(1).unwrap();
        assert_eq!("deps", method.name.to_string());
        let args = &method.args;
        assert_eq!(4, args.len());
        let expected: Type = parse_quote!(&QueryEvaluator<'_, Env>);
        assert_eq!(&expected, &args.first().unwrap().ty);
        let expected: Type = parse_quote!(TargetSet<Env::Target>);
        assert_eq!(&expected, &args.get(1).unwrap().ty);
        let expected: Type = parse_quote!(QueryValueDepth);
        assert_eq!(&expected, &args.get(2).unwrap().ty);
        let expected: Type = parse_quote!(Option<CapturedExpr<'_>>);
        assert_eq!(&expected, &args.get(3).unwrap().ty);

        let method = module.methods.get(2).unwrap();
        assert_eq!("filter", method.name.to_string());
        let args = &method.args;
        assert_eq!(2, args.len());
        let expected: Type = parse_quote!(String);
        assert_eq!(&expected, &args.first().unwrap().ty);
        let expected: Type = parse_quote!(TargetSet<Env::Target>);
        assert_eq!(&expected, &args.get(1).unwrap().ty);
    }
}
