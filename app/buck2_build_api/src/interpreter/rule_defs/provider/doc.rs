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
use starlark::docs::DocFunction;
use starlark::docs::DocItem;
use starlark::docs::DocMember;
use starlark::docs::DocProperty;
use starlark::docs::DocString;
use starlark::docs::DocType;
use starlark::environment::GlobalsBuilder;
use starlark::typing::Ty;

/// Source for provider member documentation.
///
/// This enum represents the two mutually exclusive ways to obtain
/// documentation for a provider's members (fields and methods).
#[expect(
    clippy::large_enum_variant,
    reason = "made once for a provider's documentation and consumed at once"
)]
pub enum ProviderMembersSource<'a> {
    /// The members that the provider's type documents: those of its methods module.
    /// Used when `#[internal_provider(..., methods = custom_methods)]` is specified.
    FromType(DocType),

    /// Use documentation derived from struct fields.
    /// Used when no custom methods builder is provided.
    FromFields {
        fields: &'a [&'a str],
        field_docs: &'a [Option<DocString>],
        field_types: &'a [Ty],
    },
}

/// The documentation of a provider type: a type, with the fields of a provider as its members
/// and the function that `creator` sets, if there is one, as its constructor.
pub fn provider_callable_documentation(
    creator: Option<for<'a> fn(&'a mut GlobalsBuilder)>,
    members_source: ProviderMembersSource<'_>,
    self_ty: Ty,
    overall: &Option<DocString>,
) -> DocItem {
    let members = match members_source {
        ProviderMembersSource::FromType(ty_doc) => ty_doc
            .members
            .into_iter()
            .map(|(name, member)| {
                let prop = match member {
                    DocMember::Property(p) => p,
                    DocMember::Function(f) => DocProperty {
                        docs: f.docs,
                        typ: f.ret.typ,
                    },
                };
                (name, prop)
            })
            .collect::<Vec<_>>(),
        ProviderMembersSource::FromFields {
            fields,
            field_docs,
            field_types,
        } => itertools::izip!(fields.iter(), field_docs.iter(), field_types.iter())
            .map(|(name, docs, return_type)| {
                let prop = DocProperty {
                    docs: docs.clone(),
                    typ: return_type.clone(),
                };
                (name.to_string(), prop)
            })
            .collect::<Vec<_>>(),
    };

    DocItem::Type(DocType {
        docs: overall.clone(),
        ty: self_ty,
        members: members
            .into_iter()
            .map(|(a, b)| (a, DocMember::Property(b)))
            .collect(),
        constructor: creator.map(creator_documentation),
    })
}

/// The documentation of the one function that a provider's creator sets.
fn creator_documentation(creator: for<'a> fn(&'a mut GlobalsBuilder)) -> DocFunction {
    let docs = GlobalsBuilder::new().with(creator).build().documentation();
    match docs.members.into_iter().exactly_one() {
        Ok((_, DocItem::Member(DocMember::Function(function)))) => function,
        // A function that says `as_type` documents itself as a type that it is the constructor
        // of.
        Ok((
            _,
            DocItem::Type(DocType {
                constructor: Some(function),
                ..
            }),
        )) => function,
        _ => panic!("a provider's creator sets one function"),
    }
}
