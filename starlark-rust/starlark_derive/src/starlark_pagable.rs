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

//! Derive macros for `StarlarkSerialize` and `StarlarkDeserialize`.
//!
//! By default, each field is serialized/deserialized through the starlark context
//! (`StarlarkSerialize::starlark_serialize` / `StarlarkDeserialize::starlark_deserialize`).
//!
//! Fields annotated with `#[starlark_pagable(pagable)]` use the pagable bridge instead
//! (`PagableSerialize::pagable_serialize(ctx.pagable())` /
//!  `PagableDeserialize::pagable_deserialize(ctx.pagable())`).
//!
//! `StarlarkDeserialize` is implemented at the type's brand, see
//! [`deserialize_brand`](crate::pagable_brand::deserialize_brand).

use std::collections::HashSet;

use proc_macro2::Ident;
use proc_macro2::TokenStream;
use quote::quote;
use quote::quote_spanned;
use syn::Attribute;
use syn::Data;
use syn::DeriveInput;
use syn::Field;
use syn::Fields;
use syn::GenericArgument;
use syn::Generics;
use syn::Index;
use syn::Lifetime;
use syn::LitStr;
use syn::PathArguments;
use syn::Token;
use syn::Type;
use syn::TypePath;
use syn::WherePredicate;
use syn::parse::ParseStream;
use syn::parse_macro_input;
use syn::parse_quote;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;

use crate::pagable_brand::deserialize_brand;

#[derive(Default)]
struct FieldAttrs {
    /// Route this field through `PagableSerialize`/`PagableDeserialize`
    /// (`ctx.pagable()`) instead of the starlark context.
    pagable: bool,
    /// Skip this field during ser/de.
    ///
    /// - Serialize: emit nothing for this field.
    /// - Deserialize: produce the value via `Default::default()` (requires
    ///   `T: Default`) *or* via the expression provided in `skip_expr` if set.
    ///
    /// Useful for runtime-only fields such as caches, profiling data, or
    /// fields holding non-serializable handles, where `Option::None` /
    /// `Vec::new()` / etc. is the right "empty" value to restore.
    skip: bool,
    /// Custom expression used to reconstruct the field on deserialize when
    /// combined with `skip`. If `Some("Ty::any()")`, the generated
    /// deserialize code uses that expression instead of `Default::default()`.
    skip_expr: Option<String>,
    /// `serialize_with = "path::to::fn"` — replace the field's serialize
    /// call with a free function.
    serialize_with: Option<String>,
    /// `deserialize_with = "path::to::fn"`
    deserialize_with: Option<String>,
}

#[derive(Default)]
struct TypeAttrs {
    /// Override the auto-synthesized per-field bounds (see
    /// [`compute_auto_bounds`]). When set, these predicates are appended
    /// verbatim to the impl's where clause and the auto-bounds are
    /// suppressed entirely — same container-level semantics as serde's
    /// `#[serde(bound = "...")]`. The string is a comma-separated list
    /// of `syn::WherePredicate`s, so projection predicates like
    /// `V::String: StarlarkSerialize` are valid. The predicates apply to
    /// both impls; a `StarlarkDeserialize` bound in them names the brand
    /// the type is deserialized at (a type without lifetime parameters is
    /// deserialized at `'fv`). `bound = ""` says the type's own bounds
    /// suffice, e.g. because a type parameter's trait implies both traits.
    bound: Option<Vec<WherePredicate>>,
}

fn extract_type_attrs(attrs: &[Attribute]) -> syn::Result<TypeAttrs> {
    syn::custom_keyword!(bound);

    let mut opts = TypeAttrs::default();

    for attr in attrs {
        if !attr.path().is_ident("starlark_pagable") {
            continue;
        }

        attr.parse_args_with(|input: ParseStream| {
            while !input.is_empty() {
                if input.peek(bound) {
                    input.parse::<bound>()?;
                    input.parse::<Token![=]>()?;
                    let s: LitStr = input.parse()?;
                    if opts.bound.is_some() {
                        return Err(input.error("`bound` was set twice"));
                    }
                    let predicates = s
                        .parse_with(Punctuated::<WherePredicate, Token![,]>::parse_terminated)
                        .map_err(|err| {
                            syn::Error::new(
                                s.span(),
                                format!("failed to parse `bound = \"...\"` as a comma-separated list of `where` predicates: {err}"),
                            )
                        })?;
                    opts.bound = Some(predicates.into_iter().collect());
                } else {
                    return Err(input.error("expected `bound = \"...\"` at the type level"));
                }
                if input.is_empty() {
                    break;
                }
                input.parse::<Token![,]>()?;
            }
            Ok(())
        })?;
    }

    Ok(opts)
}

/// Build the impl-target type and the trailing where-clause: the impl is generic over the
/// type's own parameters, and the where clause is the type's own where clause with `extra`
/// predicates appended.
fn gen_target_ty(
    name: &Ident,
    generics: &Generics,
    extra: &[WherePredicate],
) -> (TokenStream, TokenStream) {
    let (_, ty_generics, _) = generics.split_for_impl();
    let where_clause = build_where_clause(generics, extra);
    (quote! { #name #ty_generics }, where_clause)
}

/// Which of the two traits an impl is being generated for; the auto-synthesized bounds
/// require the same trait of the field types.
#[derive(Clone, Copy)]
enum DerivedTrait<'a> {
    Serialize,
    /// At this brand.
    Deserialize(&'a Lifetime),
}

impl DerivedTrait<'_> {
    fn bound(self) -> TokenStream {
        match self {
            DerivedTrait::Serialize => quote! { starlark::pagable::StarlarkSerialize },
            DerivedTrait::Deserialize(brand) => {
                quote! { starlark::pagable::StarlarkDeserialize<#brand> }
            }
        }
    }
}

/// Pick the effective per-impl bound predicates: user's `bound = "..."`
/// if provided (override semantics, matching serde's container-level
/// `#[serde(bound = "...")]`), otherwise the auto-synthesized bounds
/// from [`compute_auto_bounds`].
fn effective_bounds(
    input: &DeriveInput,
    attrs: &TypeAttrs,
    derived: DerivedTrait,
) -> syn::Result<Vec<WherePredicate>> {
    if let Some(bound) = &attrs.bound {
        return Ok(bound.clone());
    }
    compute_auto_bounds(input, derived)
}

/// Combine the type's own where-clause predicates with any extra
/// predicates and emit a `where ...` token stream (or nothing if both
/// are empty).
fn build_where_clause(generics: &Generics, extra: &[WherePredicate]) -> TokenStream {
    let own = generics
        .where_clause
        .as_ref()
        .map(|w| w.predicates.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    if own.is_empty() && extra.is_empty() {
        return quote! {};
    }
    let predicates = own.iter().chain(extra.iter());
    quote! { where #(#predicates,)* }
}

/// Compute the auto-synthesized per-field bounds for the derived impl.
///
/// Walks every non-`skip`ped field's type and, for each:
/// - generic type parameter `T` referenced (directly or transitively)
/// - associated-type projection `T::Assoc` referenced
///
/// emits `T: StarlarkSerialize` or `T: StarlarkDeserialize<'brand>`, for the
/// trait being derived.
///
/// Carve-outs: `PhantomData<T>` is skipped (its impls don't depend on
/// `T`), as are fields marked `#[starlark_pagable(skip)]`.
///
/// Cases that need extra bounds (e.g. a `SmallMap<K, V>` whose `K` needs
/// `SmallMapKeyDeserialize` for the deserialize impl) are not handled here —
/// use the explicit `#[starlark_pagable(bound = "...")]` escape hatch for those.
fn compute_auto_bounds(
    input: &DeriveInput,
    derived: DerivedTrait,
) -> syn::Result<Vec<WherePredicate>> {
    let all_type_params: HashSet<Ident> = input
        .generics
        .type_params()
        .map(|p| p.ident.clone())
        .collect();

    let mut visitor = AutoBoundVisitor {
        all_type_params,
        type_params_used: HashSet::new(),
        associated_types_used: Vec::new(),
    };

    let mut visit_field = |field: &Field| -> syn::Result<()> {
        let attrs = extract_field_attrs(&field.attrs)?;
        if attrs.skip || (attrs.serialize_with.is_some() && attrs.deserialize_with.is_some()) {
            return Ok(());
        }
        visitor.visit_type(&field.ty);
        Ok(())
    };

    match &input.data {
        Data::Struct(data) => {
            for field in &data.fields {
                visit_field(field)?;
            }
        }
        Data::Enum(data) => {
            for variant in &data.variants {
                for field in &variant.fields {
                    visit_field(field)?;
                }
            }
        }
        Data::Union(_) => return Ok(Vec::new()),
    }

    let bound = derived.bound();
    let mut predicates: Vec<WherePredicate> = Vec::new();
    for ident in &visitor.type_params_used {
        predicates.push(parse_quote! { #ident: #bound });
    }
    for path in &visitor.associated_types_used {
        predicates.push(parse_quote! { #path: #bound });
    }
    Ok(predicates)
}

/// Visitor that collects generic type parameters and associated-type
/// projections referenced in field types.
struct AutoBoundVisitor {
    all_type_params: HashSet<Ident>,
    type_params_used: HashSet<Ident>,
    associated_types_used: Vec<TypePath>,
}

impl AutoBoundVisitor {
    fn visit_type(&mut self, ty: &Type) {
        match ty {
            Type::Path(tp) => self.visit_type_path(tp),
            Type::Reference(tr) => self.visit_type(&tr.elem),
            Type::Group(tg) => self.visit_type(&tg.elem),
            Type::Paren(tp) => self.visit_type(&tp.elem),
            Type::Array(ta) => self.visit_type(&ta.elem),
            Type::Slice(ts) => self.visit_type(&ts.elem),
            Type::Ptr(tp) => self.visit_type(&tp.elem),
            Type::Tuple(tt) => {
                for elem in &tt.elems {
                    self.visit_type(elem);
                }
            }
            _ => {}
        }
    }

    fn visit_type_path(&mut self, tp: &TypePath) {
        // PhantomData<T> doesn't impose any bound on T.
        if let Some(seg) = tp.path.segments.last()
            && seg.ident == "PhantomData"
        {
            return;
        }

        // Direct type parameter: `T` (single segment, no args).
        if tp.qself.is_none() && tp.path.leading_colon.is_none() && tp.path.segments.len() == 1 {
            let seg = &tp.path.segments[0];
            if matches!(seg.arguments, PathArguments::None)
                && self.all_type_params.contains(&seg.ident)
            {
                self.type_params_used.insert(seg.ident.clone());
                return;
            }
        }

        // Associated-type projection: `T::Assoc` (multi-segment, first is
        // a type parameter, no qself).
        if tp.qself.is_none() && tp.path.leading_colon.is_none() && tp.path.segments.len() >= 2 {
            let first = &tp.path.segments[0];
            if matches!(first.arguments, PathArguments::None)
                && self.all_type_params.contains(&first.ident)
            {
                self.associated_types_used.push(tp.clone());
                return;
            }
        }

        // Otherwise: not a recognised type-param/projection itself —
        // recurse into any generic arguments (e.g. `Option<T>`, `Vec<T>`).
        for seg in &tp.path.segments {
            if let PathArguments::AngleBracketed(args) = &seg.arguments {
                for arg in &args.args {
                    if let GenericArgument::Type(arg_ty) = arg {
                        self.visit_type(arg_ty);
                    }
                }
            }
        }
    }
}

fn extract_field_attrs(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    syn::custom_keyword!(pagable);
    syn::custom_keyword!(skip);
    syn::custom_keyword!(serialize_with);
    syn::custom_keyword!(deserialize_with);

    let mut opts = FieldAttrs::default();

    for attr in attrs {
        if !attr.path().is_ident("starlark_pagable") {
            continue;
        }

        attr.parse_args_with(|input: ParseStream| {
            while !input.is_empty() {
                if input.peek(pagable) {
                    input.parse::<pagable>()?;
                    if opts.pagable {
                        return Err(input.error("`pagable` was set twice"));
                    }
                    opts.pagable = true;
                } else if input.peek(skip) {
                    input.parse::<skip>()?;
                    if opts.skip {
                        return Err(input.error("`skip` was set twice"));
                    }
                    opts.skip = true;
                    if input.peek(Token![=]) {
                        input.parse::<Token![=]>()?;
                        let s: LitStr = input.parse()?;
                        opts.skip_expr = Some(s.value());
                    }
                } else if input.peek(serialize_with) {
                    input.parse::<serialize_with>()?;
                    input.parse::<Token![=]>()?;
                    let s: LitStr = input.parse()?;
                    if opts.serialize_with.is_some() {
                        return Err(input.error("`serialize_with` was set twice"));
                    }
                    opts.serialize_with = Some(s.value());
                } else if input.peek(deserialize_with) {
                    input.parse::<deserialize_with>()?;
                    input.parse::<Token![=]>()?;
                    let s: LitStr = input.parse()?;
                    if opts.deserialize_with.is_some() {
                        return Err(input.error("`deserialize_with` was set twice"));
                    }
                    opts.deserialize_with = Some(s.value());
                } else {
                    return Err(input.error(
                        "expected `pagable`, `skip[ = \"expr\"]`, `serialize_with = \"path\"`, or `deserialize_with = \"path\"`",
                    ));
                }
                if input.is_empty() {
                    break;
                }
                input.parse::<Token![,]>()?;
            }
            Ok(())
        })?;
    }

    Ok(opts)
}

fn field_ident(i: usize, field: &syn::Field) -> proc_macro2::TokenStream {
    match &field.ident {
        Some(name) => quote! { #name },
        None => {
            let idx = Index::from(i);
            quote! { #idx }
        }
    }
}

pub fn derive_starlark_pagable(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ser = derive_starlark_serialize_impl(&input);
    let de = derive_starlark_deserialize_impl(&input);
    match (ser, de) {
        (Ok(ser), Ok(de)) => quote! { #ser #de }.into(),
        (Err(err), _) | (_, Err(err)) => err.to_compile_error().into(),
    }
}

pub fn derive_starlark_serialize(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match derive_starlark_serialize_impl(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn derive_starlark_serialize_impl(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let type_attrs = extract_type_attrs(&input.attrs)?;
    let (impl_generics, _, _) = input.generics.split_for_impl();
    let bounds = effective_bounds(input, &type_attrs, DerivedTrait::Serialize)?;
    let (target_ty, where_clause) = gen_target_ty(name, &input.generics, &bounds);

    let body = match &input.data {
        syn::Data::Struct(data) => gen_serialize_fields(&data.fields)?,
        syn::Data::Enum(data) => gen_serialize_enum(name, data)?,
        syn::Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                input,
                "StarlarkSerialize derive does not support unions",
            ));
        }
    };

    Ok(quote! {
        impl #impl_generics starlark::pagable::StarlarkSerialize for #target_ty #where_clause {
            fn starlark_serialize(
                &self,
                ctx: &mut dyn starlark::pagable::StarlarkSerializeContext,
            ) -> starlark::Result<()> {
                #body
                Ok(())
            }
        }
    })
}

fn gen_serialize_fields(fields: &Fields) -> syn::Result<proc_macro2::TokenStream> {
    let mut stmts = Vec::new();

    for (i, field) in fields.iter().enumerate() {
        let attrs = extract_field_attrs(&field.attrs)?;
        let ident = field_ident(i, field);

        if attrs.skip {
            // Referenced to suppress "unused field" lints for skipped fields.
            stmts.push(quote_spanned! { field.span()=>
                let _ = &self.#ident;
            });
            continue;
        }

        let stmt = if let Some(fn_path) = &attrs.serialize_with {
            let path: syn::Path = syn::parse_str(fn_path).map_err(|e| {
                syn::Error::new(field.span(), format!("invalid `serialize_with` path: {e}"))
            })?;
            quote_spanned! { field.span()=>
                #path(&self.#ident, ctx)?;
            }
        } else if attrs.pagable {
            quote_spanned! { field.span()=>
                pagable::PagableSerialize::pagable_serialize(&self.#ident, ctx.pagable())?;
            }
        } else {
            quote_spanned! { field.span()=>
                starlark::pagable::StarlarkSerialize::starlark_serialize(&self.#ident, ctx)?;
            }
        };
        stmts.push(stmt);
    }

    Ok(quote! { #(#stmts)* })
}

pub fn derive_starlark_deserialize(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match derive_starlark_deserialize_impl(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn derive_starlark_deserialize_impl(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let type_attrs = extract_type_attrs(&input.attrs)?;
    let (brand, generics) = deserialize_brand(input)?;
    let (impl_generics, _, _) = generics.split_for_impl();
    let bounds = effective_bounds(input, &type_attrs, DerivedTrait::Deserialize(&brand))?;
    let (target_ty, where_clause) = gen_target_ty(name, &input.generics, &bounds);

    let body = match &input.data {
        syn::Data::Struct(data) => gen_deserialize_struct(name, &data.fields)?,
        syn::Data::Enum(data) => gen_deserialize_enum(name, data)?,
        syn::Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                input,
                "StarlarkDeserialize derive does not support unions",
            ));
        }
    };

    Ok(quote! {
        impl #impl_generics starlark::pagable::StarlarkDeserialize<#brand> for #target_ty #where_clause {
            fn starlark_deserialize(
                ctx: &mut dyn starlark::pagable::StarlarkDeserializeContext<'_, #brand>,
            ) -> starlark::Result<Self> {
                #body
            }
        }
    })
}

fn gen_skip_value(
    attrs: &FieldAttrs,
    ty: &syn::Type,
    span: proc_macro2::Span,
) -> syn::Result<proc_macro2::TokenStream> {
    if let Some(expr_str) = &attrs.skip_expr {
        let expr: TokenStream = expr_str.parse()?;
        Ok(quote_spanned! { span=> #expr })
    } else {
        Ok(quote_spanned! { span=>
            <#ty as ::std::default::Default>::default()
        })
    }
}

fn gen_deserialize_struct(name: &Ident, fields: &Fields) -> syn::Result<proc_macro2::TokenStream> {
    match fields {
        Fields::Named(named) => {
            let mut field_inits = Vec::new();
            for (i, field) in named.named.iter().enumerate() {
                let attrs = extract_field_attrs(&field.attrs)?;
                let ident = field.ident.as_ref().unwrap();
                let ty = &field.ty;
                let field_name_str = field_ident(i, field).to_string();

                let value = if attrs.skip {
                    gen_skip_value(&attrs, ty, field.span())?
                } else if let Some(fn_path) = &attrs.deserialize_with {
                    let path: syn::Path = syn::parse_str(fn_path).map_err(|e| {
                        syn::Error::new(
                            field.span(),
                            format!("invalid `deserialize_with` path: {e}"),
                        )
                    })?;
                    quote_spanned! { field.span()=>
                        #path(ctx)?
                    }
                } else if attrs.pagable {
                    quote_spanned! { field.span()=>
                        pagable::PagableDeserialize::pagable_deserialize(ctx.pagable())?
                    }
                } else {
                    quote_spanned! { field.span()=>
                        starlark::pagable::starlark_deserialize_field::<#ty>(ctx, #field_name_str)?
                    }
                };
                field_inits.push(quote! { #ident: #value });
            }
            Ok(quote! {
                Ok(#name { #(#field_inits,)* })
            })
        }
        Fields::Unnamed(unnamed) => {
            let mut field_values = Vec::new();
            for (i, field) in unnamed.unnamed.iter().enumerate() {
                let attrs = extract_field_attrs(&field.attrs)?;
                let ty = &field.ty;
                let field_name_str = field_ident(i, field).to_string();

                let value = if attrs.skip {
                    gen_skip_value(&attrs, ty, field.span())?
                } else if let Some(fn_path) = &attrs.deserialize_with {
                    let path: syn::Path = syn::parse_str(fn_path).map_err(|e| {
                        syn::Error::new(
                            field.span(),
                            format!("invalid `deserialize_with` path: {e}"),
                        )
                    })?;
                    quote_spanned! { field.span()=>
                        #path(ctx)?
                    }
                } else if attrs.pagable {
                    quote_spanned! { field.span()=>
                        pagable::PagableDeserialize::pagable_deserialize(ctx.pagable())?
                    }
                } else {
                    quote_spanned! { field.span()=>
                        starlark::pagable::starlark_deserialize_field::<#ty>(ctx, #field_name_str)?
                    }
                };
                field_values.push(value);
            }
            Ok(quote! {
                Ok(#name(#(#field_values,)*))
            })
        }
        Fields::Unit => Ok(quote! { Ok(#name) }),
    }
}

/// Generate `StarlarkSerialize` body for an enum.
///
/// Wire format: `u8` discriminant tag (variant index, 0..n) followed by the
/// variant's payload fields in declaration order. Variant field attributes
/// (`#[starlark_pagable(pagable|skip|skip = "...")]`) apply as with structs.
fn gen_serialize_enum(
    enum_name: &Ident,
    data: &syn::DataEnum,
) -> syn::Result<proc_macro2::TokenStream> {
    if data.variants.len() > 255 {
        return Err(syn::Error::new_spanned(
            enum_name,
            "StarlarkSerialize derive does not support enums with more than 255 variants",
        ));
    }

    if data.variants.is_empty() {
        return Ok(quote! {});
    }

    let mut arms = Vec::new();
    for (idx, variant) in data.variants.iter().enumerate() {
        let tag = idx as u8;
        let variant_name = &variant.ident;

        match &variant.fields {
            Fields::Unit => {
                arms.push(quote! {
                    #enum_name::#variant_name => {
                        <u8 as pagable::PagableSerialize>::pagable_serialize(&#tag, ctx.pagable())?;
                    }
                });
            }
            Fields::Unnamed(unnamed) => {
                let bindings: Vec<_> = (0..unnamed.unnamed.len())
                    .map(|i| Ident::new(&format!("f{i}"), proc_macro2::Span::call_site()))
                    .collect();
                let mut stmts = Vec::new();
                for (binding, field) in bindings.iter().zip(unnamed.unnamed.iter()) {
                    let attrs = extract_field_attrs(&field.attrs)?;
                    if attrs.skip {
                        stmts.push(quote_spanned! { field.span()=> let _ = #binding; });
                        continue;
                    }
                    if let Some(fn_path) = &attrs.serialize_with {
                        let path: syn::Path = syn::parse_str(fn_path).map_err(|e| {
                            syn::Error::new(
                                field.span(),
                                format!("invalid `serialize_with` path: {e}"),
                            )
                        })?;
                        stmts.push(quote_spanned! { field.span()=>
                            #path(#binding, ctx)?;
                        });
                    } else if attrs.pagable {
                        stmts.push(quote_spanned! { field.span()=>
                            pagable::PagableSerialize::pagable_serialize(#binding, ctx.pagable())?;
                        });
                    } else {
                        stmts.push(quote_spanned! { field.span()=>
                            starlark::pagable::StarlarkSerialize::starlark_serialize(#binding, ctx)?;
                        });
                    }
                }
                arms.push(quote! {
                    #enum_name::#variant_name(#(#bindings),*) => {
                        <u8 as pagable::PagableSerialize>::pagable_serialize(&#tag, ctx.pagable())?;
                        #(#stmts)*
                    }
                });
            }
            Fields::Named(named) => {
                let bindings: Vec<_> = named
                    .named
                    .iter()
                    .map(|f| f.ident.as_ref().unwrap().clone())
                    .collect();
                let mut stmts = Vec::new();
                for (binding, field) in bindings.iter().zip(named.named.iter()) {
                    let attrs = extract_field_attrs(&field.attrs)?;
                    if attrs.skip {
                        stmts.push(quote_spanned! { field.span()=> let _ = #binding; });
                        continue;
                    }
                    if let Some(fn_path) = &attrs.serialize_with {
                        let path: syn::Path = syn::parse_str(fn_path).map_err(|e| {
                            syn::Error::new(
                                field.span(),
                                format!("invalid `serialize_with` path: {e}"),
                            )
                        })?;
                        stmts.push(quote_spanned! { field.span()=>
                            #path(#binding, ctx)?;
                        });
                    } else if attrs.pagable {
                        stmts.push(quote_spanned! { field.span()=>
                            pagable::PagableSerialize::pagable_serialize(#binding, ctx.pagable())?;
                        });
                    } else {
                        stmts.push(quote_spanned! { field.span()=>
                            starlark::pagable::StarlarkSerialize::starlark_serialize(#binding, ctx)?;
                        });
                    }
                }
                arms.push(quote! {
                    #enum_name::#variant_name { #(#bindings),* } => {
                        <u8 as pagable::PagableSerialize>::pagable_serialize(&#tag, ctx.pagable())?;
                        #(#stmts)*
                    }
                });
            }
        }
    }

    Ok(quote! {
        match self {
            #(#arms)*
        }
    })
}

/// Generate `StarlarkDeserialize` body for an enum.
fn gen_deserialize_enum(
    enum_name: &Ident,
    data: &syn::DataEnum,
) -> syn::Result<proc_macro2::TokenStream> {
    let enum_name_str = enum_name.to_string();

    // An uninhabited enum has no serialized form, so data claiming to hold one is corrupt.
    if data.variants.is_empty() {
        return Ok(quote! {
            Err(starlark::__derive_refs::PagableError::UninhabitedType {
                type_name: #enum_name_str,
            }
            .into())
        });
    }

    let mut arms = Vec::new();
    for (idx, variant) in data.variants.iter().enumerate() {
        let tag = idx as u8;
        let variant_name = &variant.ident;

        match &variant.fields {
            Fields::Unit => {
                arms.push(quote! {
                    #tag => Ok(#enum_name::#variant_name),
                });
            }
            Fields::Unnamed(unnamed) => {
                let mut values = Vec::new();
                for (i, field) in unnamed.unnamed.iter().enumerate() {
                    let attrs = extract_field_attrs(&field.attrs)?;
                    let ty = &field.ty;
                    let field_name_str = field_ident(i, field).to_string();
                    let v = if attrs.skip {
                        gen_skip_value(&attrs, ty, field.span())?
                    } else if let Some(fn_path) = &attrs.deserialize_with {
                        let path: syn::Path = syn::parse_str(fn_path).map_err(|e| {
                            syn::Error::new(
                                field.span(),
                                format!("invalid `deserialize_with` path: {e}"),
                            )
                        })?;
                        quote_spanned! { field.span()=>
                            #path(ctx)?
                        }
                    } else if attrs.pagable {
                        quote_spanned! { field.span()=>
                            pagable::PagableDeserialize::pagable_deserialize(ctx.pagable())?
                        }
                    } else {
                        quote_spanned! { field.span()=>
                            starlark::pagable::starlark_deserialize_field::<#ty>(ctx, #field_name_str)?
                        }
                    };
                    values.push(v);
                }
                arms.push(quote! {
                    #tag => Ok(#enum_name::#variant_name(#(#values),*)),
                });
            }
            Fields::Named(named) => {
                let mut inits = Vec::new();
                for (i, field) in named.named.iter().enumerate() {
                    let attrs = extract_field_attrs(&field.attrs)?;
                    let ident = field.ident.as_ref().unwrap();
                    let ty = &field.ty;
                    let field_name_str = field_ident(i, field).to_string();
                    let v = if attrs.skip {
                        gen_skip_value(&attrs, ty, field.span())?
                    } else if let Some(fn_path) = &attrs.deserialize_with {
                        let path: syn::Path = syn::parse_str(fn_path).map_err(|e| {
                            syn::Error::new(
                                field.span(),
                                format!("invalid `deserialize_with` path: {e}"),
                            )
                        })?;
                        quote_spanned! { field.span()=>
                            #path(ctx)?
                        }
                    } else if attrs.pagable {
                        quote_spanned! { field.span()=>
                            pagable::PagableDeserialize::pagable_deserialize(ctx.pagable())?
                        }
                    } else {
                        quote_spanned! { field.span()=>
                            starlark::pagable::starlark_deserialize_field::<#ty>(ctx, #field_name_str)?
                        }
                    };
                    inits.push(quote! { #ident: #v });
                }
                arms.push(quote! {
                    #tag => Ok(#enum_name::#variant_name { #(#inits),* }),
                });
            }
        }
    }

    Ok(quote! {
        let tag = <u8 as pagable::PagableDeserialize>::pagable_deserialize(ctx.pagable())?;
        match tag {
            #(#arms)*
            _ => Err(starlark::__derive_refs::PagableError::InvalidVariantTag {
                enum_name: #enum_name_str,
                tag,
            }
            .into()),
        }
    })
}
