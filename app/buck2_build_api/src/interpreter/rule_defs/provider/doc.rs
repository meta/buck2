/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use starlark::docs::DocItem;
use starlark::docs::DocMember;
use starlark::docs::DocProperty;
use starlark::docs::DocString;
use starlark::docs::DocType;
use starlark::environment::Methods;
use starlark::typing::Ty;

/// Source for provider member documentation.
///
/// This enum represents the two mutually exclusive ways to obtain
/// documentation for a provider's members (fields and methods).
pub enum ProviderMembersSource<'a> {
    /// Extract documentation from the provider's methods table.
    /// Used when `#[internal_provider(..., methods = custom_methods)]` is specified.
    FromMethods(&'static Methods),

    /// Use documentation derived from struct fields.
    /// Used when no custom methods builder is provided.
    FromFields {
        fields: &'a [&'a str],
        field_docs: &'a [Option<DocString>],
        field_types: &'a [Ty],
    },
}

/// The documentation of a provider type: a type, with the fields of a provider as its members.
pub fn provider_callable_documentation(
    members_source: ProviderMembersSource<'_>,
    self_ty: Ty,
    overall: &Option<DocString>,
) -> DocItem {
    let members = match members_source {
        ProviderMembersSource::FromMethods(methods) => {
            let methods_doc = methods.documentation(self_ty.clone());

            methods_doc
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
                .collect::<Vec<_>>()
        }
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
        constructor: None,
    })
}
