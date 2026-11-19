/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Proc macros for pagable typetag support.
//!
//! This module provides attribute macros similar to the `typetag` crate but
//! for pagable serialization.

use proc_macro2::Span;
use proc_macro2::TokenStream;
use quote::format_ident;
use quote::quote;
use quote::quote_spanned;
use syn::Data;
use syn::DeriveInput;
use syn::GenericParam;
use syn::Ident;
use syn::ItemImpl;
use syn::ItemTrait;
use syn::Path;
use syn::spanned::Spanned;

/// Generate code for a trait definition with `#[pagable_typetag]`.
///
/// This generates:
/// - A trait-specific registration struct
/// - A static registry for the trait
/// - `inventory::collect!` for the registration struct
/// - `PagableBoxDeserialize` impl for `dyn Trait`
pub fn typetag_trait(item: ItemTrait) -> syn::Result<TokenStream> {
    let trait_name = &item.ident;
    let trait_vis = &item.vis;
    let registration_struct_name = Ident::new(
        &format!("__PagableRegistration_{}", trait_name),
        Span::call_site(),
    );
    let accumulator_name = format_ident!("__PAGABLE_GENERIC_ACC_{}", trait_name);

    Ok(quote! {
        #item

        // Per-trait wrapper around TypetagRegistration to satisfy inventory orphan rules.
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        #trait_vis struct #registration_struct_name(
            pub pagable::typetag::TypetagRegistration<dyn #trait_name>
        );

        impl dyn #trait_name {
            #[doc(hidden)]
            #trait_vis const fn __pagable_wrap_registration(
                reg: pagable::typetag::TypetagRegistration<dyn #trait_name>,
            ) -> #registration_struct_name {
                #registration_struct_name(reg)
            }

            fn __pagable_registry() -> &'static pagable::typetag::TypetagRegistry<dyn #trait_name> {
                static REGISTRY: std::sync::OnceLock<
                    pagable::typetag::TypetagRegistry<dyn #trait_name>
                > = std::sync::OnceLock::new();
                REGISTRY.get_or_init(|| {
                    let generic_entries = #accumulator_name.drain();
                    pagable::typetag::TypetagRegistry::from_inventory_and_generic(
                        pagable::__internal::inventory::iter::<#registration_struct_name>
                            .into_iter()
                            .map(|r| &r.0),
                        generic_entries,
                    )
                })
            }

            // Registrations for generic impls can live in other modules or
            // crates, where the accumulator static is not nameable directly;
            // they reach it through the trait, like `__pagable_wrap_registration`.
            #[doc(hidden)]
            #trait_vis fn __pagable_generic_accumulator()
            -> &'static pagable::typetag::GenericTypetagAccumulator<dyn #trait_name> {
                &#accumulator_name
            }
        }

        pagable::__internal::inventory::collect!(#registration_struct_name);

        // --- Generic typetag section infrastructure ---

        #[doc(hidden)]
        #[expect(non_upper_case_globals, reason = "generated from the trait name")]
        #trait_vis static #accumulator_name: pagable::typetag::GenericTypetagAccumulator<dyn #trait_name> =
            pagable::typetag::GenericTypetagAccumulator::new();

        impl<'de> pagable::PagableBoxDeserialize<'de> for dyn #trait_name {
            fn deserialize_box<D: pagable::PagableDeserializer<'de> + ?Sized>(
                deserializer: &mut D,
            ) -> pagable::Result<Box<Self>> {
                <dyn #trait_name>::__pagable_registry()
                    .deserialize_tagged(deserializer.as_dyn())
            }

            fn deserialize_arc_payload<D: pagable::PagableDeserializer<'de> + ?Sized>(
                deserializer: &mut D,
            ) -> pagable::Result<std::sync::Arc<Self>> {
                <dyn #trait_name>::__pagable_registry()
                    .deserialize_tagged_arc_payload(deserializer.as_dyn())
            }
        }

        // Write `tag + body` for a borrowed or boxed `dyn Trait`. The Arc-specific
        // override writes `tag + canonical concrete Arc` to preserve allocation identity.
        //
        // Allowed because `PagableTagged` doesn't have `PagableSerialize` as
        // a supertrait — otherwise Rust would auto-synthesize this impl and
        // hit E0371.
        impl pagable::PagableSerialize for dyn #trait_name {
            fn pagable_serialize(
                &self,
                serializer: &mut dyn pagable::PagableSerializer,
            ) -> pagable::Result<()> {
                pagable::typetag::PagableTagged::serialize_tagged(self, serializer)
            }

            fn pagable_serialize_arc_payload(
                self: std::sync::Arc<Self>,
                serializer: &mut dyn pagable::PagableSerializer,
            ) -> pagable::Result<()> {
                pagable::typetag::PagableTagged::serialize_tagged_arc_payload(self, serializer)
            }
        }
    })
}

/// Generate code for an impl block for a struct or impl declaration for a struct with `#[pagable_typetag]`.
///
/// This generates:
/// - `PagableTypeTag` impl for the concrete type
/// - `inventory::submit!` to register the type with the trait-specific registration struct
fn typetag_struct(
    item: TokenStream,
    self_ty: &syn::Type,
    trait_path: &syn::Path,
    type_ident: &str,
) -> TokenStream {
    quote! {
        #item

        // Implement PagableTypeTag for the concrete type. The tag matches
        // the `PagableStableName` scheme (`module_path` + identifier;
        // `module_path!` expands at the definition site), so a persisted tag
        // never depends on which registration form a type uses and never
        // collides between same-named types in different modules.
        impl pagable::typetag::PagableTypeTag for #self_ty {
            fn pagable_type_tag_static() -> &'static str {
                concat!(module_path!(), "::", #type_ident)
            }
        }

        // Direct (non-wrapped) typetag registrations deliberately do NOT emit
        // a `PagableRegisteredFor` impl: that marker is only meaningful when
        // paired with a wrapper type. Use `register_typetag!(Wrapper<T> as dyn Trait)`
        // to register `T` for a specific wrapper.

        // Submit to inventory for automatic registration,
        // using the per-trait wrapper struct generated by #[pagable_typetag] on the trait.
        pagable::__internal::inventory::submit! {
            <dyn #trait_path>::__pagable_wrap_registration(
                pagable::typetag::TypetagRegistration {
                    tag: <#self_ty as pagable::typetag::PagableTypeTag>::pagable_type_tag_static,
                    deserialize: |deserializer| {
                        let value: #self_ty =
                            pagable::PagableDeserialize::pagable_deserialize(deserializer)?;
                        Ok(Box::new(value) as Box<dyn #trait_path>)
                    },
                    deserialize_arc_payload: |deserializer| {
                        let value: std::sync::Arc<#self_ty> =
                            pagable::PagableDeserialize::pagable_deserialize(deserializer)?;
                        let value: std::sync::Arc<dyn #trait_path> = value;
                        Ok(value)
                    },
                }
            )
        }
    }
}

/// Generate code for a generic registration with `#[pagable_typetag]` —
/// either a generic impl block of the dyn trait, or a generic struct
/// definition with the dyn trait as the attribute argument (used when the
/// struct only implements the dyn trait through a blanket impl, so there is
/// no impl block to annotate).
///
/// Concrete typetag impls register with `inventory::submit!`, but generic impls
/// cannot submit a registration for `Wrapper<T>` in the abstract: the registry
/// needs one entry for each concrete monomorphized type that is actually used,
/// such as `Wrapper<Foo>` or `Wrapper<Bar>`. This generator therefore emits a
/// two-part registration path for generic impls:
///
/// - A normal generic helper function builds a `TypetagRegistration` for the
///   concrete `Self` and pushes it into the trait's `GenericTypetagAccumulator`.
/// - A retained generic anchor emits a pointer to that helper into the
///   global linker section for each monomorphized instantiation.
///
/// On first dyn deserialization, the trait macro calls a runtime helper that
/// iterates the global function-pointer section once and calls every emitted
/// helper. That populates every trait accumulator with exactly the generic
/// instantiations that were monomorphized into the binary. The registry then
/// drains its trait's entries and merges them with the non-generic inventory
/// registrations.
fn typetag_generic_struct(
    item: TokenStream,
    generics: &syn::Generics,
    self_ty: &syn::Type,
    trait_path: &syn::Path,
    span: Span,
) -> syn::Result<TokenStream> {
    if let syn::Type::Path(type_path) = self_ty {
        type_path.path.segments.last().ok_or_else(|| {
            syn::Error::new_spanned(self_ty, "could not extract type name for pagable_typetag")
        })?;
    } else {
        return Err(syn::Error::new_spanned(
            self_ty,
            "pagable_typetag requires a simple type path",
        ));
    }

    // The caller has already added the `PagableStableName` predicate to
    // `generics`; reusing them propagates it to every generated item, which
    // all name the monomorphization via its stable name.
    let (impl_generics, _ty_generics, where_clause) = generics.split_for_impl();
    let generic_args: Vec<_> = generics
        .params
        .iter()
        .filter_map(|param| match param {
            GenericParam::Type(param) => Some(&param.ident),
            GenericParam::Const(param) => Some(&param.ident),
            GenericParam::Lifetime(_) => None,
        })
        .collect();

    Ok(quote_spanned! { span =>
        #item

        const _: () = {
            impl #impl_generics pagable::typetag::PagableTagged for #self_ty #where_clause {
                fn pagable_type_tag(&self) -> &'static str {
                    // Keep the monomorphized anchor and its emitted section
                    // record linked until `#[used(linker)]` is stable in the
                    // supported toolchains.
                    core::hint::black_box(
                        __pagable_registration_anchor::<#(#generic_args),*>
                            as extern "C" fn(),
                    );
                    <Self as pagable::typetag::PagableStableName>::pagable_stable_name()
                }
                fn pagable_serialize_body(
                    &self,
                    serializer: &mut dyn pagable::PagableSerializer,
                ) -> pagable::Result<()> {
                    <Self as pagable::PagableSerialize>::pagable_serialize(self, serializer)
                }

                // Mirrors the blanket impl for `PagableTypeTag` types, which
                // this manual impl bypasses.
                fn serialize_tagged_arc_payload(
                    self: std::sync::Arc<Self>,
                    serializer: &mut dyn pagable::PagableSerializer,
                ) -> pagable::Result<()> {
                    let tag = pagable::typetag::PagableTagged::pagable_type_tag(&*self);
                    pagable::__internal::serde::Serialize::serialize(&tag, serializer.serde())?;
                    serializer.serialize_arc(&self)
                }
            }

            #[doc(hidden)]
            extern "C" fn __pagable_do_register #impl_generics () #where_clause {
                <dyn #trait_path>::__pagable_generic_accumulator().push(pagable::typetag::TypetagRegistration {
                    tag: <#self_ty as pagable::typetag::PagableStableName>::pagable_stable_name,
                    deserialize: |deserializer| {
                        let value: #self_ty =
                            pagable::PagableDeserialize::pagable_deserialize(deserializer)?;
                        Ok(Box::new(value) as Box<dyn #trait_path>)
                    },
                    deserialize_arc_payload: |deserializer| {
                        let value: std::sync::Arc<#self_ty> =
                            pagable::PagableDeserialize::pagable_deserialize(deserializer)?;
                        let value: std::sync::Arc<dyn #trait_path> = value;
                        Ok(value)
                    },
                });
            }

            #[doc(hidden)]
            #[inline(never)]
            extern "C" fn __pagable_registration_anchor #impl_generics () #where_clause {
                pagable::__pagable_emit_generic_typetag_registration!(
                    __pagable_do_register::<#(#generic_args),*>
                );
            }
        };
    })
}

/// Main entry point for the `#[pagable_typetag]` attribute macro.
///
/// Supports three forms:
/// - `#[pagable_typetag]` on a trait definition
/// - `#[pagable_typetag]` on an impl block (concrete or generic)
/// - `#[pagable_typetag(TraitName)]` on a struct definition; a generic struct
///   registers every monomorphization for `dyn TraitName`
pub fn pagable_typetag_impl(
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    // Check if we have a trait path attribute (for struct declarations)
    let trait_path: Option<Path> = if attr.is_empty() {
        None
    } else {
        match syn::parse::<Path>(attr) {
            Ok(path) => Some(path),
            Err(err) => return err.to_compile_error().into(),
        }
    };

    // If we have a trait path try to parse as a type definition
    if let Some(trait_path) = trait_path {
        let type_item = match syn::parse::<DeriveInput>(item) {
            Ok(input) if !matches!(input.data, Data::Union(_)) => input,
            _ => {
                return syn::Error::new(
                    Span::call_site(),
                    "pagable_typetag with a trait argument can only be applied to struct or enum definitions",
                )
                .to_compile_error()
                .into();
            }
        };
        let type_name = &type_item.ident;

        let has_generics = type_item
            .generics
            .params
            .iter()
            .any(|param| matches!(param, GenericParam::Type(_) | GenericParam::Const(_)));
        if has_generics {
            // Generic type: register every monomorphization for the
            // given dyn trait. This is the type-level counterpart of
            // the generic impl form below, for traits the type only
            // implements through a blanket impl (so there is no impl
            // block to annotate).
            if type_item
                .generics
                .params
                .iter()
                .any(|param| matches!(param, GenericParam::Lifetime(_)))
            {
                return syn::Error::new_spanned(
                    &type_item.generics,
                    "pagable_typetag does not support lifetime parameters",
                )
                .to_compile_error()
                .into();
            }

            let (_impl_generics, ty_generics, _where_clause) = type_item.generics.split_for_impl();
            let self_ty: syn::Type = syn::parse_quote!(#type_name #ty_generics);

            // Generics for the generated items only — the type
            // definition itself is re-emitted untouched. Defaults are
            // stripped because they are invalid in impl position.
            let mut generics = type_item.generics.clone();
            for param in &mut generics.params {
                match param {
                    GenericParam::Type(param) => param.default = None,
                    GenericParam::Const(param) => param.default = None,
                    GenericParam::Lifetime(_) => {}
                }
            }
            generics
                .make_where_clause()
                .predicates
                .push(syn::parse_quote! { #self_ty: pagable::typetag::PagableStableName });

            let span = type_item.span();
            let item_tokens = quote_spanned! { span => #type_item };
            return match typetag_generic_struct(item_tokens, &generics, &self_ty, &trait_path, span)
            {
                Ok(tokens) => tokens.into(),
                Err(err) => err.to_compile_error().into(),
            };
        }

        let type_ident = type_name.to_string();
        let self_ty: syn::Type = syn::parse_quote!(#type_name);
        let item_tokens = quote! { #type_item };
        return typetag_struct(item_tokens, &self_ty, &trait_path, &type_ident).into();
    }

    // Try to parse as a trait or impl block
    if let Ok(trait_item) = syn::parse::<ItemTrait>(item.clone()) {
        match typetag_trait(trait_item) {
            Ok(tokens) => tokens.into(),
            Err(err) => err.to_compile_error().into(),
        }
    } else if let Ok(impl_item) = syn::parse::<ItemImpl>(item) {
        let self_ty = (*impl_item.self_ty).clone();
        let trait_path = match impl_item.trait_.as_ref() {
            Some((path, _)) => path.clone(),
            None => {
                return syn::Error::new_spanned(
                    &impl_item,
                    "pagable_typetag requires a trait impl",
                )
                .to_compile_error()
                .into();
            }
        };

        let has_generics = impl_item
            .generics
            .params
            .iter()
            .any(|param| matches!(param, GenericParam::Type(_) | GenericParam::Const(_)));

        if has_generics {
            // See `typetag_generic_struct` for the full link-section
            // registration design used for generic impls.
            // Generic impl: emit a helper plus a link-section entry. The
            // trait-level registry builder runs those entries to collect the
            // monomorphized concrete registrations.
            //
            // The trait's `PagableTagged` supertrait obligation now includes
            // the stable-name requirement, so add it to the user's impl
            // rather than making every caller spell it out.
            let span = impl_item.span();
            let mut impl_item = impl_item;
            impl_item
                .generics
                .make_where_clause()
                .predicates
                .push(syn::parse_quote! { #self_ty: pagable::typetag::PagableStableName });
            let item_tokens = quote_spanned! { span => #impl_item };
            return match typetag_generic_struct(
                item_tokens,
                &impl_item.generics,
                &self_ty,
                &trait_path,
                span,
            ) {
                Ok(tokens) => tokens.into(),
                Err(err) => err.to_compile_error().into(),
            };
        }

        let type_ident = if let syn::Type::Path(type_path) = &self_ty {
            match type_path.path.segments.last() {
                Some(seg) => seg.ident.to_string(),
                None => {
                    return syn::Error::new_spanned(
                        &self_ty,
                        "could not extract type name for pagable_typetag",
                    )
                    .to_compile_error()
                    .into();
                }
            }
        } else {
            return syn::Error::new_spanned(
                &self_ty,
                "pagable_typetag requires a simple type path",
            )
            .to_compile_error()
            .into();
        };
        let item_tokens = quote! { #impl_item };
        typetag_struct(item_tokens, &self_ty, &trait_path, &type_ident).into()
    } else {
        syn::Error::new(
            Span::call_site(),
            "pagable_typetag can only be applied to trait definitions, impl blocks, or struct definitions (with trait argument)",
        )
        .to_compile_error()
        .into()
    }
}
