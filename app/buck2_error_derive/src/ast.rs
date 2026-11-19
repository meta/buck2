/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// This code is adapted from https://github.com/dtolnay/thiserror licensed under Apache-2.0 or MIT.

use proc_macro2::Span;
use syn::Data;
use syn::DataEnum;
use syn::DataStruct;
use syn::DeriveInput;
use syn::Error;
use syn::Fields;
use syn::Generics;
use syn::Ident;
use syn::Index;
use syn::Member;
use syn::Result;
use syn::Type;

use crate::attr;
use crate::attr::Attrs;
use crate::generics::ParamsInScope;

pub enum Input<'a> {
    Struct(Struct<'a>),
    Enum(Enum<'a>),
}

pub struct Struct<'a> {
    pub attrs: Attrs<'a>,
    pub ident: Ident,
    pub generics: &'a Generics,
    pub fields: Vec<Field<'a>>,
}

pub struct Enum<'a> {
    pub attrs: Attrs<'a>,
    pub ident: Ident,
    pub generics: &'a Generics,
    pub variants: Vec<Variant<'a>>,
}

pub struct Variant<'a> {
    pub original: &'a syn::Variant,
    pub attrs: Attrs<'a>,
    pub ident: Ident,
    pub fields: Vec<Field<'a>>,
}

pub struct Field<'a> {
    pub original: &'a syn::Field,
    pub attrs: Attrs<'a>,
    pub member: Member,
    pub ty: &'a Type,
    pub contains_generic: bool,
}

impl<'a> Input<'a> {
    pub fn from_syn(node: &'a DeriveInput) -> Result<Self> {
        match &node.data {
            Data::Struct(data) => Struct::from_syn(node, data).map(Input::Struct),
            Data::Enum(data) => Enum::from_syn(node, data).map(Input::Enum),
            Data::Union(_) => Err(Error::new_spanned(
                node,
                "union as errors are not supported",
            )),
        }
    }
}

impl<'a> Struct<'a> {
    fn from_syn(node: &'a DeriveInput, data: &'a DataStruct) -> Result<Self> {
        let mut attrs = attr::get(&node.attrs)?;
        let scope = ParamsInScope::new(&node.generics);
        let span = attrs.span().unwrap_or_else(Span::call_site);
        let fields = Field::multiple_from_syn(&data.fields, &scope, span)?;
        if let Some(display) = &mut attrs.display {
            display.expand_shorthand(&fields);
        }
        Ok(Struct {
            attrs,
            ident: node.ident.clone(),
            generics: &node.generics,
            fields,
        })
    }
}

impl<'a> Enum<'a> {
    fn from_syn(node: &'a DeriveInput, data: &'a DataEnum) -> Result<Self> {
        let attrs = attr::get(&node.attrs)?;
        let scope = ParamsInScope::new(&node.generics);
        let span = attrs.span().unwrap_or_else(Span::call_site);
        let variants = data
            .variants
            .iter()
            .map(|node| {
                let mut variant = Variant::from_syn(node, &scope, span)?;
                if let display @ None = &mut variant.attrs.display {
                    *display = attrs.display.clone();
                }
                if let Some(display) = &mut variant.attrs.display {
                    display.expand_shorthand(&variant.fields);
                } else if variant.attrs.transparent.is_none() {
                    variant.attrs.transparent = attrs.transparent;
                }
                Ok(variant)
            })
            .collect::<Result<_>>()?;
        Ok(Enum {
            attrs,
            ident: node.ident.clone(),
            generics: &node.generics,
            variants,
        })
    }
}

impl<'a> Variant<'a> {
    fn from_syn(node: &'a syn::Variant, scope: &ParamsInScope<'a>, span: Span) -> Result<Self> {
        let attrs = attr::get(&node.attrs)?;
        let span = attrs.span().unwrap_or(span);
        Ok(Variant {
            original: node,
            attrs,
            ident: node.ident.clone(),
            fields: Field::multiple_from_syn(&node.fields, scope, span)?,
        })
    }
}

impl<'a> Field<'a> {
    fn multiple_from_syn(
        fields: &'a Fields,
        scope: &ParamsInScope<'a>,
        span: Span,
    ) -> Result<Vec<Self>> {
        fields
            .iter()
            .enumerate()
            .map(|(i, field)| Field::from_syn(i, field, scope, span))
            .collect()
    }

    fn from_syn(
        i: usize,
        node: &'a syn::Field,
        scope: &ParamsInScope<'a>,
        span: Span,
    ) -> Result<Self> {
        Ok(Field {
            original: node,
            attrs: attr::get(&node.attrs)?,
            member: node.ident.clone().map_or_else(
                || {
                    Member::Unnamed(Index {
                        index: i as u32,
                        span,
                    })
                },
                Member::Named,
            ),
            ty: &node.ty,
            contains_generic: scope.intersects(&node.ty),
        })
    }
}

impl Attrs<'_> {
    pub fn span(&self) -> Option<Span> {
        if let Some(display) = &self.display {
            Some(display.fmt.span())
        } else if let Some(transparent) = &self.transparent {
            Some(transparent.span)
        } else {
            None
        }
    }
}
