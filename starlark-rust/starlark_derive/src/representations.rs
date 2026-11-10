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

//! `#[derive(StarlarkRepresentations)]`: a type's view over its representations.

use quote::quote;

use crate::any_lifetime::derive_provides_static_type_for;
use crate::v_lifetime::find_v_lifetime;

pub(crate) fn derive_starlark_representations(
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    match derive_starlark_representations_impl(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// The `Rep` of a variant's `ValueTyped<'v, Rep>`.
fn representation_of(ty: &syn::Type) -> syn::Result<syn::Type> {
    let err =
        || syn::Error::new_spanned(ty, "each variant holds a `ValueTyped<'v, Representation>`");
    let syn::Type::Path(path) = ty else {
        return Err(err());
    };
    let last = path.path.segments.last().ok_or_else(err)?;
    if last.ident != "ValueTyped" {
        return Err(err());
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return Err(err());
    };
    let mut types = args.args.iter().filter_map(|arg| match arg {
        syn::GenericArgument::Type(t) => Some(t.clone()),
        _ => None,
    });
    match (types.next(), types.next()) {
        (Some(rep), None) => Ok(rep),
        _ => Err(err()),
    }
}

fn derive_starlark_representations_impl(
    input: syn::DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    let span = input.ident.span();
    let syn::Data::Enum(data) = &input.data else {
        return Err(syn::Error::new(
            span,
            "`StarlarkRepresentations` is derived for an enum with one `ValueTyped` variant per \
             representation",
        ));
    };
    let lt = find_v_lifetime(&input.generics)?
        .ok_or_else(|| syn::Error::new(span, "the enum needs the `'v` lifetime parameter"))?
        .clone();
    let mut variants: Vec<(&syn::Ident, syn::Type)> = Vec::new();
    for variant in &data.variants {
        let fields = match &variant.fields {
            syn::Fields::Unnamed(fields) => fields.unnamed.iter().collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let [field] = fields.as_slice() else {
            return Err(syn::Error::new_spanned(
                variant,
                "each variant holds one `ValueTyped<'v, Representation>`",
            ));
        };
        variants.push((&variant.ident, representation_of(&field.ty)?));
    }
    let Some((_, canonical)) = variants.first() else {
        return Err(syn::Error::new(
            span,
            "a type has at least one representation",
        ));
    };

    let provides_static_type = derive_provides_static_type_for(&input)?;
    let ident = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    let variant_idents: Vec<&syn::Ident> = variants.iter().map(|(v, _)| *v).collect();
    let reps: Vec<&syn::Type> = variants.iter().map(|(_, r)| r).collect();

    Ok(quote! {
        #provides_static_type

        impl #impl_generics std::fmt::Display for #ident #type_generics #where_clause {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    #( #ident::#variant_idents(x) => std::fmt::Display::fmt(x.as_ref(), f), )*
                }
            }
        }

        #(
            impl #impl_generics std::convert::From<starlark::values::ValueTyped<#lt, #reps>>
                for #ident #type_generics #where_clause
            {
                #[inline]
                fn from(x: starlark::values::ValueTyped<#lt, #reps>) -> Self {
                    #ident::#variant_idents(x)
                }
            }
        )*

        impl #impl_generics starlark::values::type_repr::StarlarkTypeRepr
            for #ident #type_generics #where_clause
        {
            type Canonical = #canonical;

            fn starlark_type_repr() -> starlark::typing::Ty {
                <Self as starlark::values::StarlarkType<#lt>>::get_type_starlark_repr()
            }
        }

        impl #impl_generics starlark::__derive_refs::ViewOfRepresentations<#lt>
            for #ident #type_generics #where_clause
        {
            fn from_value(value: starlark::values::Value<#lt>) -> std::option::Option<Self> {
                #(
                    if let Some(x) = starlark::values::ValueTyped::<#reps>::new(value) {
                        return std::option::Option::Some(#ident::#variant_idents(x));
                    }
                )*
                std::option::Option::None
            }
        }

        impl #impl_generics starlark::values::UnpackValue<#lt> for #ident #type_generics #where_clause {
            type Type = Self;
            type Error = std::convert::Infallible;

            #[inline]
            fn unpack(
                _value: starlark::values::ValueOf<#lt, Self>,
                get: Self,
            ) -> std::result::Result<Self, Self::Error> {
                std::result::Result::Ok(get)
            }
        }
    })
}
