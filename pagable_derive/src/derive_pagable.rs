/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use proc_macro2::Ident;
use proc_macro2::Span;
use quote::ToTokens;
use quote::quote;
use syn::Attribute;
use syn::Data;
use syn::DataEnum;
use syn::DataStruct;
use syn::DeriveInput;
use syn::Fields;
use syn::GenericParam;
use syn::Generics;
use syn::LitStr;
use syn::Token;
use syn::Type;
use syn::Variant;
use syn::WherePredicate;
use syn::parse::ParseStream;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;

pub(crate) fn derive_pagable(
    input: proc_macro::TokenStream,
    generate_serialize: bool,
    generate_deserialize: bool,
) -> proc_macro::TokenStream {
    match derive_pagable_impl(input.into(), generate_serialize, generate_deserialize) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

pub fn map_to_tokens<T>(
    iter: impl IntoIterator<Item = T>,
    f: impl Fn(T) -> syn::Result<proc_macro2::TokenStream>,
) -> syn::Result<Vec<proc_macro2::TokenStream>> {
    iter.into_iter()
        .map(f)
        .collect::<syn::Result<Vec<proc_macro2::TokenStream>>>()
}

struct SerializeGenerator;
impl SerializeGenerator {
    fn gen_field(field: &ParsedField) -> syn::Result<proc_macro2::TokenStream> {
        let ty = &field.ty;
        let ident = &field.var_name;
        let ident_str = ident.to_string();
        if field.attrs.skip || field.attrs.discard.is_some() {
            Ok(quote! {})
        } else if field.attrs.flatten_serde {
            Ok(quote! {
                pagable::__internal::anyhow::Context::with_context(
                    <#ty as pagable::__internal::serde::ser::Serialize>::serialize(#ident, serializer.serde()),
                    || format!("serializing serde flattened field {}", #ident_str),
                )?
            })
        } else {
            Ok(quote! {
                pagable::__internal::anyhow::Context::with_context(
                    <#ty as pagable::PagableSerialize>::pagable_serialize(#ident, serializer),
                    || format!("serializing field {}", #ident_str),
                )?

            })
        }
    }

    fn gen_unpack(v: &ParsedFields) -> syn::Result<proc_macro2::TokenStream> {
        match &v.data {
            ParsedFieldsData::Unit => Ok(quote! {}),
            ParsedFieldsData::Named(fields) => {
                let names = fields.iter().map(|f| &f.var_name);
                Ok(quote! {{#(#names),*} })
            }
            ParsedFieldsData::Unnamed(fields) => {
                let names = fields.iter().map(|f| &f.var_name);
                Ok(quote! {(#(#names),*) })
            }
        }
    }

    fn gen_visit_fields(v: &ParsedFields) -> syn::Result<proc_macro2::TokenStream> {
        let visit_fields = match &v.data {
            ParsedFieldsData::Unit => {
                return Ok(quote! {});
            }
            ParsedFieldsData::Named(fields) => map_to_tokens(fields, Self::gen_field)?,
            ParsedFieldsData::Unnamed(fields) => map_to_tokens(fields, Self::gen_field)?,
        };
        Ok(quote! {#(#visit_fields;)*})
    }

    fn gen_struct(v: &ParsedStruct) -> syn::Result<proc_macro2::TokenStream> {
        let unpack = Self::gen_unpack(&v.fields)?;
        let visit_fields = Self::gen_visit_fields(&v.fields)?;
        Ok(quote! {
            let Self #unpack = self;
            #visit_fields;
        })
    }

    fn gen_enum_variant(v: &ParsedEnumVariant) -> syn::Result<proc_macro2::TokenStream> {
        let index = v.index;
        let name = &v.name;
        let unpack = Self::gen_unpack(&v.fields)?;
        let visit_fields = Self::gen_visit_fields(&v.fields)?;
        Ok(quote! {
            Self::#name #unpack => {
                <usize as pagable::__internal::serde::Serialize>::serialize(&#index, serializer.serde())?;
                #visit_fields;
            }
        })
    }

    fn gen_enum(v: &ParsedEnum) -> syn::Result<proc_macro2::TokenStream> {
        if v.cases.is_empty() {
            Ok(quote! {})
        } else {
            let cases = map_to_tokens(&v.cases, Self::gen_enum_variant)?;
            Ok(quote! {
                match self {
                    #(#cases),*
                }
            })
        }
    }

    fn gen_visit_body(parsed: &ParsedType) -> syn::Result<proc_macro2::TokenStream> {
        match &parsed.data {
            ParsedTypeData::Struct(s) => Self::gen_struct(s),
            ParsedTypeData::Enum(e) => Self::gen_enum(e),
        }
    }
}

struct DeserializeGenerator;

impl DeserializeGenerator {
    fn gen_field(field: &ParsedField) -> syn::Result<proc_macro2::TokenStream> {
        let ty = &field.ty;
        let ident_str = field.var_name.to_string();

        if field.attrs.skip {
            Ok(quote! {})
        } else if let Some(discard) = &field.attrs.discard {
            let discard = discard.parse::<proc_macro2::TokenStream>()?;
            Ok(quote! {#discard})
        } else if field.attrs.flatten_serde {
            Ok(quote! {
                pagable::__internal::anyhow::Context::with_context(
                    <#ty as pagable::__internal::serde::de::Deserialize>::deserialize(deserializer.serde()),
                    || format!("deserializing serde flattened field {}", #ident_str),
                )?
            })
        } else {
            Ok(quote! {
                pagable::__internal::anyhow::Context::with_context(
                    <#ty as pagable::PagableDeserialize>::pagable_deserialize(deserializer),
                    || format!("deserializing field {}", #ident_str),
                )?
            })
        }
    }

    fn gen_initializer(v: &ParsedFields) -> syn::Result<proc_macro2::TokenStream> {
        match &v.data {
            ParsedFieldsData::Unit => Ok(quote! {}),
            ParsedFieldsData::Named(fields) => {
                let names = fields.iter().map(|f| &f.var_name);
                let visit_fields = map_to_tokens(fields, Self::gen_field)?;
                Ok(quote! {
                    {#(#names: #visit_fields),*}
                })
            }
            ParsedFieldsData::Unnamed(fields) => {
                let visit_fields = map_to_tokens(fields, Self::gen_field)?;
                Ok(quote! {
                     (#(#visit_fields),*)
                })
            }
        }
    }

    fn gen_struct(v: &ParsedStruct) -> syn::Result<proc_macro2::TokenStream> {
        let initializer = Self::gen_initializer(&v.fields)?;
        Ok(quote! {Self #initializer})
    }

    fn gen_enum_variant(v: &ParsedEnumVariant) -> syn::Result<proc_macro2::TokenStream> {
        let index = v.index;
        let name = &v.name;
        let initializer = Self::gen_initializer(&v.fields)?;
        Ok(quote! { #index => Self::#name #initializer})
    }

    fn gen_enum(v: &ParsedEnum) -> syn::Result<proc_macro2::TokenStream> {
        if v.cases.is_empty() {
            Ok(quote! {})
        } else {
            let cases = map_to_tokens(&v.cases, Self::gen_enum_variant)?;
            Ok(quote! {
                {
                    let index = <usize as pagable::__internal::serde::Deserialize>::deserialize(deserializer.serde())?;
                    match index {
                        #(#cases,)*
                        _ => unreachable!()
                    }
                }
            })
        }
    }

    fn gen_visit_body(parsed: &ParsedType) -> syn::Result<proc_macro2::TokenStream> {
        match &parsed.data {
            ParsedTypeData::Struct(s) => Self::gen_struct(s),
            ParsedTypeData::Enum(e) => Self::gen_enum(e),
        }
    }
}

fn impl_generics(
    generics: &Generics,
    attrs: &PagableAttrs,
) -> syn::Result<(proc_macro2::TokenStream, proc_macro2::TokenStream)> {
    let mut impl_generics = Vec::new();
    for p in &generics.params {
        impl_generics.push(match p {
            GenericParam::Type(tp) => {
                let mut tp = tp.clone();
                if attrs.bound.is_none() && !attrs.skip && attrs.discard.is_none() {
                    tp.bounds.push(syn::parse2(quote! {
                        pagable::Pagable
                    })?);
                }

                tp.default = None;
                tp.to_token_stream()
            }
            GenericParam::Lifetime(l) => l.to_token_stream(),
            GenericParam::Const(c) => c.to_token_stream(),
        });
    }
    if impl_generics.is_empty() {
        Ok((quote! {}, quote! { <'de>}))
    } else {
        Ok((
            quote! { <#(#impl_generics),*> },
            quote! { <'de, #(#impl_generics),*> },
        ))
    }
}

fn where_clause(generics: &Generics, attrs: &PagableAttrs) -> proc_macro2::TokenStream {
    let own = generics
        .where_clause
        .as_ref()
        .map(|where_clause| where_clause.predicates.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let extra = attrs.bound.iter().flatten();
    let extra_is_empty = attrs.bound.as_ref().is_none_or(Vec::is_empty);
    if own.is_empty() && extra_is_empty {
        return quote! {};
    }
    let predicates = own.iter().chain(extra);
    quote! {  where #(#predicates,)* }
}

fn derive_pagable_impl(
    input: proc_macro2::TokenStream,
    generate_serialize: bool,
    generate_deserialize: bool,
) -> syn::Result<proc_macro2::TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    let name = &input.ident;
    let name_str = name.to_string();
    let (_impl_generics, type_generics, _where_clause) = input.generics.split_for_impl();

    let attrs = extract_attrs(&input.attrs)?;
    let (ser_impl_generics, de_impl_generics) = impl_generics(&input.generics, &attrs)?;
    let where_clause = where_clause(&input.generics, &attrs);

    if attrs.skip {
        return Err(syn::Error::new(
            input.ident.span(),
            "skip was set on struct level",
        ));
    }

    let parsed = parse_body(&input)?;

    let serialize_body = if !generate_serialize {
        quote! {}
    } else {
        let body = SerializeGenerator::gen_visit_body(&parsed)?;
        quote! {
            #[allow(clippy::redundant_closure_call, unused, clippy::todo)]
            impl #ser_impl_generics pagable::PagableSerialize for #name #type_generics #where_clause {
                    fn pagable_serialize(&self, serializer: &mut dyn pagable::PagableSerializer) -> pagable::__internal::anyhow::Result<()> {
                        #body
                        Ok(())
                    }

            }
        }
    };

    let deserialize_body = if !generate_deserialize {
        quote! {}
    } else {
        let body = DeserializeGenerator::gen_visit_body(&parsed)?;
        quote! {
            #[allow(clippy::redundant_closure_call, unused, clippy::todo)]
            impl #de_impl_generics pagable::PagableDeserialize<'de> for #name #type_generics #where_clause {
                fn pagable_deserialize<__D: pagable::PagableDeserializer<'de> + ?Sized>(deserializer: &mut __D) -> pagable::Result<Self> {
                    let res : pagable::Result<Self> = (|| {Ok(#body)})();
                    pagable::__internal::anyhow::Context::with_context(res, || format!("deserializing type {}", #name_str))
                }
            }
        }
    };

    let stable_name_impl = if generate_serialize {
        stable_name_impl(&input, &name_str)?
    } else {
        quote! {}
    };

    Ok(quote! {
        #serialize_body
        #deserialize_body
        #stable_name_impl
    })
}

/// Generate a `PagableStableName` impl composing the name from
/// `module_path!()`, the type's identifier, and (for generics) the stable
/// names of type arguments plus `Display`-formatted const arguments.
///
/// Emitted only from the serialize side so that deriving `PagableSerialize`
/// and `PagableDeserialize` separately does not produce duplicate impls.
/// Types with lifetime parameters get no impl: composed names are cached by
/// `TypeId`, which requires `Self: 'static`.
fn stable_name_impl(input: &DeriveInput, name_str: &str) -> syn::Result<proc_macro2::TokenStream> {
    if input
        .generics
        .params
        .iter()
        .any(|p| matches!(p, GenericParam::Lifetime(_)))
    {
        return Ok(quote! {});
    }

    let name = &input.ident;
    let (_impl_generics, type_generics, where_clause) = input.generics.split_for_impl();

    if input.generics.params.is_empty() {
        return Ok(quote! {
            impl pagable::typetag::PagableStableName for #name {
                fn pagable_stable_name() -> &'static str {
                    concat!(module_path!(), "::", #name_str)
                }
            }
        });
    }

    let mut params = Vec::new();
    let mut arg_names = Vec::new();
    for p in &input.generics.params {
        match p {
            GenericParam::Type(tp) => {
                let mut tp = tp.clone();
                tp.bounds
                    .push(syn::parse2(quote! { pagable::typetag::PagableStableName })?);
                tp.bounds.push(syn::parse2(quote! { 'static })?);
                tp.default = None;
                let ident = tp.ident.clone();
                params.push(tp.to_token_stream());
                arg_names.push(quote! {
                    <#ident as pagable::typetag::PagableStableName>::pagable_stable_name().to_owned()
                });
            }
            GenericParam::Const(c) => {
                let mut c = c.clone();
                c.default = None;
                let ident = c.ident.clone();
                params.push(c.to_token_stream());
                arg_names.push(quote! { #ident.to_string() });
            }
            GenericParam::Lifetime(_) => unreachable!("returned early above"),
        }
    }

    Ok(quote! {
        impl<#(#params),*> pagable::typetag::PagableStableName for #name #type_generics #where_clause {
            fn pagable_stable_name() -> &'static str {
                pagable::typetag::memoized_stable_name::<Self>(|| {
                    let __args: Vec<String> = vec![#(#arg_names),*];
                    format!("{}::{}<{}>", module_path!(), #name_str, __args.join(", "))
                })
            }
        }
    })
}

#[allow(unused)]
struct ParsedType {
    data: ParsedTypeData,
}

enum ParsedTypeData {
    Struct(ParsedStruct),
    Enum(ParsedEnum),
}

struct ParsedStruct {
    fields: ParsedFields,
}

struct ParsedFields {
    data: ParsedFieldsData,
}

enum ParsedFieldsData {
    Unit,
    Named(Vec<ParsedField>),
    Unnamed(Vec<ParsedField>),
}

struct ParsedEnum {
    cases: Vec<ParsedEnumVariant>,
}

struct ParsedEnumVariant {
    index: usize,
    name: Ident,
    fields: ParsedFields,
}

struct ParsedField {
    attrs: PagableAttrs,
    var_name: Ident,
    ty: Type,
}

fn parse_body(input: &DeriveInput) -> syn::Result<ParsedType> {
    let data = match &input.data {
        Data::Struct(s) => ParsedTypeData::Struct(parse_struct(s)?),
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(input, "unions are not supported"));
        }
        Data::Enum(e) => ParsedTypeData::Enum(parse_enum(e)?),
    };
    Ok(ParsedType { data })
}

fn parse_struct(input: &DataStruct) -> syn::Result<ParsedStruct> {
    let data = parse_fields(&input.fields)?;
    Ok(ParsedStruct { fields: data })
}

fn parse_enum(input: &DataEnum) -> syn::Result<ParsedEnum> {
    let cases = input
        .variants
        .iter()
        .enumerate()
        .map(|(i, v)| parse_enum_variant(i, v))
        .collect::<syn::Result<_>>()?;
    Ok(ParsedEnum { cases })
}

fn parse_enum_variant(index: usize, input: &Variant) -> syn::Result<ParsedEnumVariant> {
    let data = parse_fields(&input.fields)?;
    Ok(ParsedEnumVariant {
        index,
        name: input.ident.clone(),
        fields: data,
    })
}

fn parse_fields(input: &Fields) -> syn::Result<ParsedFields> {
    let data = match &input {
        Fields::Named(fields_named) => ParsedFieldsData::Named(
            fields_named
                .named
                .iter()
                .map(|f| {
                    <Result<_, syn::Error>>::Ok(ParsedField {
                        attrs: extract_attrs(&f.attrs)?,
                        var_name: f
                            .ident
                            .clone()
                            .ok_or_else(|| syn::Error::new_spanned(f, "unnamed field in named"))?,
                        ty: f.ty.clone(),
                    })
                })
                .collect::<syn::Result<Vec<_>>>()?,
        ),
        Fields::Unnamed(fields_unnamed) => ParsedFieldsData::Unnamed(
            fields_unnamed
                .unnamed
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    <Result<_, syn::Error>>::Ok(ParsedField {
                        attrs: extract_attrs(&f.attrs)?,
                        var_name: Ident::new(&format!("f{}", i), f.span()),
                        ty: f.ty.clone(),
                    })
                })
                .collect::<syn::Result<Vec<_>>>()?,
        ),
        Fields::Unit => ParsedFieldsData::Unit,
    };
    Ok(ParsedFields { data })
}

#[derive(Default)]
struct PagableAttrs {
    span: Option<Span>,
    skip: bool,
    flatten_serde: bool,
    bound: Option<Vec<WherePredicate>>,
    discard: Option<String>,
}

/// Parse an `#[pagable(...)]` annotation.
fn extract_attrs(attrs: &[Attribute]) -> syn::Result<PagableAttrs> {
    syn::custom_keyword!(skip);
    syn::custom_keyword!(stash);
    syn::custom_keyword!(bound);
    syn::custom_keyword!(discard);
    syn::custom_keyword!(flatten_serde);

    let mut opts = PagableAttrs::default();

    for attr in attrs.iter() {
        if !attr.path().is_ident("pagable") {
            continue;
        }

        opts.span = Some(attr.span());

        attr.parse_args_with(|input: ParseStream| {
            loop {
                if input.parse::<skip>().is_ok() {
                    if opts.skip {
                        return Err(input.error("`skip` was set twice"));
                    }
                    opts.skip = true;
                } else if input.parse::<discard>().is_ok() {
                    input.parse::<Token![=]>()?;
                    let discard = input.parse::<LitStr>()?;
                    if opts.discard.is_some() {
                        return Err(input.error("`discard` was set twice"));
                    }
                    opts.discard = Some(discard.value());
                } else if input.parse::<stash>().is_ok() {
                    if opts.skip {
                        return Err(input.error("`skip` was set twice"));
                    }
                    opts.skip = true;
                } else if input.parse::<flatten_serde>().is_ok() {
                    if opts.flatten_serde {
                        return Err(input.error("`flatten_serde` was set twice"));
                    }
                    opts.flatten_serde = true;
                } else if input.parse::<bound>().is_ok() {
                    input.parse::<Token![=]>()?;
                    let bound = input.parse::<LitStr>()?;
                    if opts.bound.is_some() {
                        return Err(input.error("`bound` was set twice"));
                    }
                    let predicates = bound
                        .parse_with(Punctuated::<WherePredicate, Token![,]>::parse_terminated)
                        .map_err(|err| {
                            syn::Error::new(
                                bound.span(),
                                format!("failed to parse `bound = \"...\"` as a comma-separated list of `where` predicates: {err}"),
                            )
                        })?;
                    opts.bound = Some(predicates.into_iter().collect());
                }

                if input.is_empty() {
                    return Ok(());
                }

                input.parse::<Token![,]>()?;
            }
        })?;
    }

    Ok(opts)
}
