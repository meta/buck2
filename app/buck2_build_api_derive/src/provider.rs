/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use convert_case::Case;
use convert_case::Casing;
use quote::ToTokens;
use quote::format_ident;
use quote::quote;
use syn::Fields;
use syn::ext::IdentExt;

pub(crate) struct InternalProviderArgs {
    creator_func: syn::Ident,
    /// A `#[starlark_module]` of methods for the provider, replacing the generated attribute
    /// lookup; it must then define the attributes itself.
    methods_func: Option<syn::Ident>,
}

impl syn::parse::Parse for InternalProviderArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::parse::Result<Self> {
        let creator_func = syn::Ident::parse(input)?;
        let mut methods_func = None;
        while input.peek(syn::Token![,]) {
            input.parse::<syn::Token![,]>()?;
            if !input.peek(syn::Ident) {
                break;
            }
            let key = syn::Ident::parse(input)?;
            if key == "methods" {
                input.parse::<syn::Token![=]>()?;
                methods_func = Some(syn::Ident::parse(input)?);
            } else {
                return Err(syn::Error::new_spanned(key, "expected `methods`"));
            }
        }
        Ok(InternalProviderArgs {
            creator_func,
            methods_func,
        })
    }
}

/// The wrapper a provider field is declared with.
///
/// `ValueOf<'v, T>` holds a value checked to be a `T` when the provider is constructed, for any
/// `T` that is a parameter binding (a view, a `ValueTyped`, `ListType<T>`, a union), and reads
/// never re-check. `ValueTyped<'v, T>` is the same for a type that is exactly one `StarlarkValue`
/// implementation.
///
/// A field that may legitimately be absent is declared `Option<ValueOf<'v, T>>` or
/// `Option<ValueTyped<'v, T>>`, which is the only spelling that documents itself honestly - the
/// others describe a field that is always a `T`, so an absent value would have to be smuggled
/// through as a `None` the starlark type does not mention.
#[derive(Copy, Clone)]
enum FieldKind {
    Of,
    OptionOf,
    Typed,
    OptionTyped,
}

/// Provider field information.
/// This does not include the `id` field.
struct Field {
    /// The name of the field
    name: syn::Ident,
    /// The docstring for the field, if present
    docstring: syn::Expr,
    /// The type argument of the wrapper, which carries the starlark type.
    field_type: syn::Type,
    kind: FieldKind,
}

impl Field {
    /// The Starlark attribute name: the field name without a raw-identifier prefix.
    fn attr_name(&self) -> syn::LitStr {
        syn::LitStr::new(&self.name.unraw().to_string(), self.name.span())
    }

    /// Expression which produces `Ty` for the field.
    fn field_type_ty(&self) -> syn::Expr {
        let field_type = &self.field_type;
        let ty: syn::Type = match self.kind {
            FieldKind::Of | FieldKind::Typed => field_type.clone(),
            FieldKind::OptionOf | FieldKind::OptionTyped => {
                syn::parse_quote_spanned! { self.name.span() =>
                    starlark::values::none::NoneOr<#field_type>
                }
            }
        };
        syn::parse_quote_spanned! { self.name.span() =>
            <#ty as starlark::values::type_repr::StarlarkTypeRepr>::starlark_type_repr()
        }
    }

    /// Expression which produces the field's `Value`, given an expression for the provider.
    fn value(&self, this: &syn::Expr) -> syn::Expr {
        let name = &self.name;
        match self.kind {
            FieldKind::Of | FieldKind::Typed => syn::parse_quote_spanned! { name.span() =>
                #this.#name.to_value()
            },
            FieldKind::OptionOf | FieldKind::OptionTyped => {
                syn::parse_quote_spanned! { name.span() =>
                    #this.#name.map_or_else(starlark::values::Value::new_none, |v| v.to_value())
                }
            }
        }
    }
}

/// Split `Wrapper<..>` into the wrapper's name and its generic arguments.
fn generic_segment(ty: &syn::Type) -> Option<(&syn::Ident, &syn::AngleBracketedGenericArguments)> {
    let syn::Type::Path(syn::TypePath {
        attrs: _,
        qself: None,
        path: syn::Path {
            leading_colon: None,
            segments,
        },
    }) = ty
    else {
        return None;
    };
    let [
        syn::PathSegment {
            ident,
            arguments: syn::PathArguments::AngleBracketed(args),
        },
    ] = Vec::from_iter(segments).as_slice()
    else {
        return None;
    };
    Some((ident, args))
}

struct ProviderCodegen {
    span: proc_macro2::Span,
    input: syn::ItemStruct,
    args: InternalProviderArgs,
}

impl ProviderCodegen {
    /// Create an instance from an input and args.
    ///
    /// This modifies the original input and removes any instances of `#[provider()]` macros
    /// on fields of the provided structs, and saves them into `field_attr_providers`.
    fn new(mut input: syn::ItemStruct, args: InternalProviderArgs) -> syn::Result<Self> {
        if let Fields::Named(fields_named) = &mut input.fields {
            for field in fields_named.named.iter_mut() {
                field.attrs = field.attrs.clone();
            }
        };
        Ok(Self {
            span: input.ident.span(),
            input,
            args,
        })
    }

    fn name(&self) -> syn::Result<syn::Ident> {
        if self.input.ident.to_string().ends_with("Gen") {
            return Err(syn::Error::new_spanned(
                &self.input.ident,
                "should not have a `Gen` suffix",
            ));
        }
        Ok(self.input.ident.clone())
    }

    fn name_str(&self) -> syn::Result<String> {
        Ok(self.name()?.to_string())
    }

    fn callable_name(&self) -> syn::Result<syn::Ident> {
        let name = self.name()?;
        Ok(format_ident!("{}Callable", name))
    }

    /// `id` field is object identity, which is ignored for equality or display purposes.
    fn is_id_field(&self, field: &syn::Field) -> bool {
        field.ident.as_ref().unwrap() == "id" && field.ty.to_token_stream().to_string() == "u64"
    }

    /// The attribute names, as string literals.
    fn field_names(&self) -> syn::Result<Vec<syn::LitStr>> {
        Ok(self
            .fields()?
            .iter()
            .map(|f| f.attr_name())
            .collect::<Vec<_>>())
    }

    /// Expressions producing each field's `Value`, given an expression for the provider.
    fn field_values(&self, this: syn::Expr) -> syn::Result<Vec<syn::Expr>> {
        Ok(self
            .fields()?
            .into_iter()
            .map(|f| f.value(&this))
            .collect::<Vec<_>>())
    }

    /// Parse the "doc" attribute and return a tokenstream that is either None if "doc" is not
    /// present, or the result of DocString::parse_docstring if present.
    fn get_docstring_impl(&self, attrs: &Vec<syn::Attribute>) -> syn::Expr {
        let mut doc_lines = vec![];

        for attr in attrs {
            if attr.path().is_ident("doc") {
                if let syn::Meta::NameValue(syn::MetaNameValue {
                    value:
                        syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(s),
                            ..
                        }),
                    ..
                }) = &attr.meta
                {
                    doc_lines.push(s.value());
                }
            }
        }

        if doc_lines.is_empty() {
            syn::parse_quote_spanned! { self.span=> None }
        } else {
            let docstring = Some(doc_lines.join("\n"));
            syn::parse_quote_spanned! { self.span=>
                starlark::docs::DocString::from_docstring(
                    starlark::docs::DocStringKind::Rust,
                    #docstring,
                )
            }
        }
    }

    fn field(&self, field: &syn::Field) -> syn::Result<Field> {
        if self.is_id_field(field) {
            return Err(syn::Error::new_spanned(
                field,
                "id field should not be documented",
            ));
        }

        let error = "Field type must be `ValueOf<'v, SomeType>`, `ValueTyped<'v, SomeType>` or \
             `StringValue<'v>`, any of them in an `Option`";
        let err = || syn::Error::new_spanned(field, error);

        let (ident, args) = generic_segment(&field.ty).ok_or_else(err)?;
        // `Option<..>` marks the field optional; everything inside it is parsed as usual.
        let (optional, ident, args) = if ident == "Option" {
            let [syn::GenericArgument::Type(value_ty)] = Vec::from_iter(&args.args).as_slice()
            else {
                return Err(err());
            };
            let (ident, args) = generic_segment(value_ty).ok_or_else(err)?;
            (true, ident, args)
        } else {
            (false, ident, args)
        };

        let typed = |optional| {
            if optional {
                FieldKind::OptionTyped
            } else {
                FieldKind::Typed
            }
        };
        // `StringValue<'v>` is `ValueTyped<'v, StarlarkStr>` under its usual name.
        let (kind, field_type): (FieldKind, syn::Type) = if ident == "StringValue" {
            let [syn::GenericArgument::Lifetime(_)] = Vec::from_iter(&args.args).as_slice() else {
                return Err(err());
            };
            (
                typed(optional),
                syn::parse_quote! { starlark::values::string::StarlarkStr },
            )
        } else {
            let kind = if ident == "ValueTyped" {
                typed(optional)
            } else if ident == "ValueOf" {
                if optional {
                    FieldKind::OptionOf
                } else {
                    FieldKind::Of
                }
            } else {
                return Err(err());
            };
            let [
                syn::GenericArgument::Lifetime(_),
                syn::GenericArgument::Type(field_type),
            ] = Vec::from_iter(&args.args).as_slice()
            else {
                return Err(err());
            };
            (kind, field_type.clone())
        };

        let name = field.ident.as_ref().unwrap().to_owned();

        let docstring = self.get_docstring_impl(&field.attrs);

        Ok(Field {
            name,
            docstring,
            field_type,
            kind,
        })
    }

    fn fields(&self) -> syn::Result<Vec<Field>> {
        match &self.input.fields {
            syn::Fields::Named(fields) => Ok(fields
                .named
                .iter()
                .filter(|f| !self.is_id_field(f))
                .map(|f| self.field(f))
                .collect::<syn::Result<Vec<_>>>()?),
            _ => Err(syn::Error::new_spanned(
                &self.input,
                "providers only support named fields",
            )),
        }
    }

    /// The `documentation()` of the provider's callable: the constructor plus the fields.
    fn documentation_function(&self) -> syn::Result<syn::ImplItemFn> {
        let provider_docstring = self.get_docstring_impl(&self.input.attrs);
        let create_func = &self.args.creator_func;
        if self.args.methods_func.is_some() {
            let methods_static_name = self.methods_static_name()?;
            return Ok(syn::parse_quote_spanned! {self.span=>
                fn documentation(&self) -> starlark::docs::DocItem {
                    let docstring = #provider_docstring;
                    buck2_build_api::interpreter::rule_defs::provider::doc::provider_callable_documentation(
                        Some(#create_func),
                        buck2_build_api::interpreter::rule_defs::provider::doc::ProviderMembersSource::FromMethods(#methods_static_name.methods()),
                        BUILTIN_PROVIDER_TY.instance(),
                        &docstring,
                    )
                }
            });
        }
        let fields = self.fields()?;
        let field_names = fields.iter().map(|f| f.attr_name()).collect::<Vec<_>>();
        let field_docstrings = fields.iter().map(|f| &f.docstring).collect::<Vec<_>>();
        let field_types = fields.iter().map(|f| f.field_type_ty()).collect::<Vec<_>>();
        Ok(syn::parse_quote_spanned! {self.span=>
            fn documentation(&self) -> starlark::docs::DocItem {
                let docstring = #provider_docstring;
                let field_names: [&str; _] = [
                    #(#field_names),*
                ];
                let field_docs: [std::option::Option<starlark::docs::DocString>; _] = [
                    #(#field_docstrings),*
                ];
                let field_types: [starlark::typing::Ty; _] = [
                    #(#field_types),*
                ];
                buck2_build_api::interpreter::rule_defs::provider::doc::provider_callable_documentation(
                    Some(#create_func),
                    buck2_build_api::interpreter::rule_defs::provider::doc::ProviderMembersSource::FromFields {
                        fields: &field_names,
                        field_docs: &field_docs,
                        field_types: &field_types,
                    },
                    BUILTIN_PROVIDER_TY.instance(),
                    &docstring,
                )
            }
        })
    }

    fn builtin_provider_ty(&self) -> syn::Result<syn::Item> {
        let name = self.name()?;
        let callable_name = self.callable_name()?;
        let fields: syn::Expr = match &self.args.methods_func {
            // The methods table describes the attributes to the typechecker.
            Some(_) => syn::parse_quote_spanned! { self.span=>
                buck2_build_api::interpreter::rule_defs::provider::builtin::ty::no_fields
            },
            None => syn::parse_quote_spanned! { self.span=>
                <#name<'static>>::__builtin_provider_fields
            },
        };
        Ok(syn::parse_quote_spanned! { self.span =>
            static BUILTIN_PROVIDER_TY: buck2_build_api::interpreter::rule_defs::provider::builtin::ty::BuiltinProviderTy<
                    #name<'static>,
                    #callable_name,
            > =
                buck2_build_api::interpreter::rule_defs::provider::builtin::ty::BuiltinProviderTy::new(
                    #fields,
                );
        })
    }

    fn typechecker_ty_function(&self) -> syn::Result<syn::Item> {
        let creator_func = &self.args.creator_func;
        Ok(syn::parse_quote_spanned! {
            self.span=>
            fn typechecker_ty(&self) -> Option<starlark::typing::Ty> {
                Some(BUILTIN_PROVIDER_TY.callable(#creator_func))
            }
        })
    }

    fn impl_display(&self) -> syn::Result<syn::Item> {
        let name = self.name()?;
        let name_str = self.name_str()?;
        let field_names = self.field_names()?;
        let field_values = self.field_values(syn::parse_quote_spanned! { self.span=> self })?;
        Ok(syn::parse_quote_spanned! { self.span=>
            impl<'v> std::fmt::Display for #name<'v> {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    buck2_build_api::__derive_refs::display_container::fmt_keyed_container(
                        f,
                        &format!("{}(", #name_str),
                        ")",
                        "=",
                        [
                            #((#field_names, &#field_values)),*
                        ]
                    )
                }
            }
        })
    }

    /// Attributes are resolved directly from the fields. A methods table would say the same
    /// thing in a few hundred more lines of generated code per provider.
    fn impl_starlark_value(&self) -> syn::Result<Vec<syn::Item>> {
        let vis = &self.input.vis;
        let name = self.name()?;
        let name_str = self.name_str()?;
        let field_names = self.field_names()?;
        let self_values = self.field_values(syn::parse_quote_spanned! { self.span=> self })?;
        let other_values = self.field_values(syn::parse_quote_spanned! { self.span=> other })?;
        let starlark_value_attr: syn::Attribute = syn::parse_quote_spanned! { self.span=>
            #[starlark::values::starlark_value(type = #name_str)]
        };
        let (methods_static, attrs): (Option<syn::Item>, Vec<syn::ImplItemFn>) =
            match &self.args.methods_func {
                Some(methods_func) => {
                    let methods_static_name = self.methods_static_name()?;
                    (
                        Some(syn::parse_quote_spanned! { self.span=>
                            starlark::methods_static!(#methods_static_name = #methods_func);
                        }),
                        vec![syn::parse_quote_spanned! { self.span=>
                            fn get_methods() -> Option<&'static starlark::environment::Methods> {
                                Some(#methods_static_name.methods())
                            }
                        }],
                    )
                }
                None => (
                    None,
                    vec![
                        syn::parse_quote_spanned! { self.span=>
                            fn get_attr(
                                &self,
                                attribute: &str,
                                _heap: starlark::values::Heap<'v>,
                            ) -> Option<starlark::values::Value<'v>> {
                                match attribute {
                                    #(#field_names => Some(#self_values),)*
                                    _ => None,
                                }
                            }
                        },
                        syn::parse_quote_spanned! { self.span=>
                            fn dir_attr(&self) -> Vec<String> {
                                vec![#(#field_names.to_owned()),*]
                            }
                        },
                    ],
                ),
            };
        Ok(vec![syn::parse_quote_spanned! { self.span=>
            starlark::starlark_complex_value!(#vis #name);
        }]
        .into_iter()
        .chain(methods_static)
        .chain([syn::parse_quote_spanned! { self.span=>
            #starlark_value_attr
            impl<'v> starlark::values::StarlarkValue<'v> for #name<'v> {
                #(#attrs)*
                fn provide(&'v self, demand: &mut starlark::values::Demand<'_, 'v>) {
                    demand.provide_value::<
                        &dyn buck2_build_api::interpreter::rule_defs::provider::ProviderLike>(self);
                }
                fn equals(&self, other: starlark::values::Value<'v>) -> starlark::Result<bool> {
                    let other: &#name<'v> = match #name::from_value(other) {
                        Some(other) => other,
                        None => return Ok(false),
                    };
                    #(
                        if !#self_values.equals(#other_values)? {
                            return Ok(false);
                        }
                    )*
                    Ok(true)
                }
                fn get_type_starlark_repr() -> starlark::typing::Ty {
                    BUILTIN_PROVIDER_TY.instance()
                }
            }
        }])
        .collect())
    }

    /// The `methods_static!` holding a custom methods table.
    fn methods_static_name(&self) -> syn::Result<syn::Ident> {
        Ok(format_ident!(
            "{}_PROVIDER_STATICS",
            self.args
                .methods_func
                .as_ref()
                .expect("only with custom methods")
                .to_string()
                .to_uppercase()
        ))
    }

    fn impl_serializable_value(&self) -> syn::Result<syn::Item> {
        let name = self.name()?;
        let field_names = self.field_names()?;
        let field_values = self.field_values(syn::parse_quote_spanned! { self.span=> self })?;
        let field_len = field_names.len();
        Ok(syn::parse_quote_spanned! { self.span=>
            impl<'v> buck2_build_api::__derive_refs::serde::Serialize for #name<'v> {
                fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error> where S : buck2_build_api::__derive_refs::serde::Serializer {
                    use buck2_build_api::__derive_refs::serde::ser::SerializeMap;

                    let mut s = s.serialize_map(Some(#field_len))?;
                    #(
                        s.serialize_entry(
                            #field_names,
                            &#field_values
                        )?;
                    )*
                    s.end()
                }
            }
        })
    }

    fn impl_provider_like(&self) -> syn::Result<syn::Item> {
        let name = self.name()?;
        let field_names = self.field_names()?;
        let field_values = self.field_values(syn::parse_quote_spanned! { self.span=> self })?;
        let callable_name = self.callable_name()?;
        Ok(syn::parse_quote_spanned! { self.span=>
            impl<'v> buck2_build_api::interpreter::rule_defs::provider::ProviderLike<'v> for #name<'v> {
                fn id(&self) -> &std::sync::Arc<buck2_core::provider::id::ProviderId> {
                    #callable_name::provider_id()
                }

                fn items(&self) -> Vec<(&str, starlark::values::Value<'v>)> {
                    vec![
                        #((#field_names, #field_values)),*
                    ]
                }
            }
        })
    }

    fn impl_frozen_builtin_provider(&self) -> syn::Result<syn::Item> {
        let name = self.name()?;
        let callable_name = self.callable_name()?;
        Ok(syn::parse_quote_spanned! { self.span=>
            impl buck2_build_api::interpreter::rule_defs::provider::FrozenBuiltinProviderLike for #name<'static> {
                fn builtin_provider_id() -> &'static std::sync::Arc<buck2_core::provider::id::ProviderId> {
                    #callable_name::provider_id()
                }
            }
        })
    }

    fn callable_struct(&self) -> syn::Result<syn::Item> {
        let vis = &self.input.vis;
        let callable_name = self.callable_name()?;
        // `StarlarkPagablePanic` is safe here: a `*Callable` value is only created from
        // from the starlark `Globals` registry (registered via `register_provider`).
        // The pagable of values on Globals is hanlded by Globals pagable support.
        Ok(syn::parse_quote_spanned! { self.span=>
            #[derive(Debug, Clone, dupe::Dupe, starlark::any::ProvidesStaticType, starlark::values::NoSerialize, allocative::Allocative, starlark::values::StarlarkPagable)]
            #vis struct #callable_name {

            }
        })
    }

    fn callable_impl_starlark_value(&self) -> syn::Result<Vec<syn::Item>> {
        let callable_name = self.callable_name()?;
        let documentation_function = self.documentation_function()?;
        let typechecker_ty_function = self.typechecker_ty_function()?;
        let create_func = &self.args.creator_func;
        let callable_name_snake_str = callable_name.to_string().to_case(Case::Snake);
        let callable_globals_static_name =
            format_ident!("{}_GLOBALS_STATIC", callable_name_snake_str.to_uppercase());

        Ok(vec![
            syn::parse_quote_spanned! {self.span=>
                starlark::starlark_simple_value!(#callable_name);
            },
            syn::parse_quote_spanned! {self.span=>
                starlark::globals_static!(#callable_globals_static_name = #create_func);
            },
            syn::parse_quote_spanned! {self.span=>
                #[starlark::values::starlark_value(type = #callable_name_snake_str)]
                impl<'v> starlark::values::StarlarkValue<'v> for #callable_name
                {
                    fn invoke(
                        &self,
                        _me: starlark::values::Value<'v>,
                        args: &starlark::eval::Arguments<'v, '_>,
                        eval: &mut starlark::eval::Evaluator<'v, '_, '_>,
                    ) -> starlark::Result<starlark::values::Value<'v>> {
                        #callable_globals_static_name.function().invoke(args, eval)
                    }

                    fn provide(&'v self, demand: &mut starlark::values::Demand<'_, 'v>) {
                        demand.provide_value::<
                            &dyn buck2_interpreter::types::provider::callable::ProviderCallableLike>(self);
                    }

                    fn eval_type(&self) -> Option<starlark::typing::Ty> {
                        Some(BUILTIN_PROVIDER_TY.instance())
                    }

                    #documentation_function
                    #typechecker_ty_function
                }
            },
        ])
    }

    fn callable_impl_provider_callable_like(&self) -> syn::Result<syn::Item> {
        let callable_name = self.callable_name()?;
        Ok(syn::parse_quote_spanned! { self.span=>
            impl buck2_interpreter::types::provider::callable::ProviderCallableLike for #callable_name {
                fn id(&self) -> buck2_error::Result<&std::sync::Arc<buck2_core::provider::id::ProviderId>> {
                    Ok(Self::provider_id_t().id())
                }
            }
        })
    }

    fn callable_impl(&self) -> syn::Result<syn::Item> {
        let callable_name = self.callable_name()?;
        let vis = &self.input.vis;
        let name = self.name()?;
        let name_str = self.name_str()?;

        Ok(syn::parse_quote_spanned! { self.span=>
            impl #callable_name {
                #vis fn provider_id()
                -> &'static std::sync::Arc<buck2_core::provider::id::ProviderId> {
                    Self::provider_id_t().id()
                }

                #vis fn provider_id_t() -> &'static std::sync::Arc<
                    buck2_core::provider::id::ProviderIdWithType<#name<'static>>,
                > {
                    static PROVIDER_ID_T: std::sync::OnceLock<
                        std::sync::Arc<
                            buck2_core::provider::id::ProviderIdWithType<#name<'static>>,
                        >
                    > = std::sync::OnceLock::new();
                    PROVIDER_ID_T.get_or_init(|| {
                        std::sync::Arc::new(
                            buck2_core::provider::id::ProviderIdWithType::new(
                                None,
                                #name_str.to_owned(),
                            ),
                        )
                    })
                }

                #vis fn new() -> Self {
                    Self {
                    }
                }
            }
        })
    }

    fn callable_impl_display(&self) -> syn::Result<syn::Item> {
        let callable_name = self.callable_name()?;
        let name_str = self.name_str()?;
        Ok(syn::parse_quote_spanned! { self.span=>
            impl std::fmt::Display for #callable_name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, "{}", #name_str)
                }
            }
        })
    }

    fn callable(&self) -> syn::Result<Vec<syn::Item>> {
        Ok([
            vec![self.callable_struct()?],
            vec![self.callable_impl()?],
            vec![self.callable_impl_display()?],
            self.callable_impl_starlark_value()?,
            vec![self.callable_impl_provider_callable_like()?],
        ]
        .into_iter()
        .flatten()
        .collect())
    }

    fn register(&self) -> syn::Result<Vec<syn::Item>> {
        let name = self.name()?;
        let name_str = self.name_str()?;
        let callable_name = self.callable_name()?;
        let fields = self.fields()?;
        let field_names = fields.iter().map(|f| f.attr_name()).collect::<Vec<_>>();
        let field_types = fields.iter().map(|f| f.field_type_ty()).collect::<Vec<_>>();
        Ok(vec![
            syn::parse_quote_spanned! { self.span=>
                impl<'v> #name<'v> {
                    #[doc(hidden)]
                    pub fn __builtin_provider_fields() -> buck2_build_api::__derive_refs::buck2_hash::BuckFixedSortedMap<String, starlark::typing::Ty> {
                        std::iter::FromIterator::from_iter([
                            #((#field_names.to_owned(), #field_types)),*
                        ])
                    }
                }
            },
            syn::parse_quote_spanned! { self.span=>
                fn register_provider(builder: &mut starlark::environment::GlobalsBuilder) {
                    builder.set(#name_str, #callable_name::new());
                }
            },
        ])
    }

    fn inventory(&self) -> syn::Result<syn::Item> {
        Ok(syn::parse_quote_spanned! { self.span=>
            buck2_build_api::__derive_refs::inventory::submit! {
                buck2_build_api::interpreter::rule_defs::provider::registration::ProviderRegistration {
                    register_globals: |globals| {
                        register_provider(globals)
                    }
                }
            }
        })
    }
}

pub(crate) fn define_provider(
    args: InternalProviderArgs,
    input: syn::ItemStruct,
) -> syn::Result<proc_macro::TokenStream> {
    let codegen = ProviderCodegen::new(input, args)?;

    if let Some(where_clause) = codegen.input.generics.where_clause {
        return Err(syn::Error::new_spanned(
            where_clause,
            "should have no where clauses",
        ));
    }
    if let Some(const_param) = codegen.input.generics.const_params().next() {
        return Err(syn::Error::new_spanned(
            const_param,
            "should have no const params",
        ));
    }
    if let Some(type_param) = codegen.input.generics.type_params().next() {
        return Err(syn::Error::new_spanned(
            type_param,
            "should have no type params",
        ));
    }
    if codegen.input.generics.lifetimes().count() != 1 {
        return Err(syn::Error::new_spanned(
            codegen.input,
            "should have exactly one lifetime param",
        ));
    }

    let input = &codegen.input;
    let input: syn::Item = syn::parse_quote_spanned! { codegen.span=>
        #input
    };
    let generated: Vec<syn::Item> = [
        vec![codegen.builtin_provider_ty()?],
        vec![input],
        vec![codegen.impl_display()?],
        codegen.impl_starlark_value()?,
        vec![codegen.impl_serializable_value()?],
        vec![codegen.impl_provider_like()?],
        vec![codegen.impl_frozen_builtin_provider()?],
        codegen.callable()?,
        codegen.register()?,
        vec![codegen.inventory()?],
    ]
    .into_iter()
    .flatten()
    .collect();

    let generated = quote! {
        #(#generated)*
    };

    Ok(generated.into())
}
