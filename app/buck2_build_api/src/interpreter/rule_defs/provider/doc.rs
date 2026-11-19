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

pub fn provider_callable_documentation(
    creator: Option<for<'a> fn(&'a mut GlobalsBuilder)>,
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

    let ctor = match creator {
        Some(creator) => {
            let docs = GlobalsBuilder::new().with(creator).build().documentation();
            if docs.members.len() == 1 {
                match docs.members.into_iter().next() {
                    Some((name, DocItem::Member(DocMember::Function(x)))) => Some((name, x)),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    };

    match ctor {
        None => DocItem::Type(DocType {
            docs: overall.clone(),
            ty: self_ty,
            members: members
                .into_iter()
                .map(|(a, b)| (a.to_owned(), DocMember::Property(b)))
                .collect(),
            constructor: None,
        }),
        Some((_name, DocFunction { docs, params, ret })) => {
            let summary = if let Some(x) = &docs {
                x.summary.clone()
            } else if let Some(x) = &overall {
                x.summary.clone()
            } else {
                "A provider that can be constructed and have its fields accessed. Returned by rules."
                    .to_owned()
            };
            let mut details = vec![
                docs.as_ref().and_then(|x| x.details.clone()),
                if docs.is_some() {
                    overall.as_ref().map(|x| x.summary.clone())
                } else {
                    None
                },
                overall.as_ref().and_then(|x| x.details.clone()),
                Some("Provides a number of fields that can be accessed:".to_owned()),
            ];
            for (name, member) in members {
                let typ = member.typ.to_string();
                let description = member.docs.map_or_else(
                    || "field".to_owned(),
                    |x| x.summary + &x.details.unwrap_or_default(),
                );
                details.push(Some(format!("* `{name}: {typ}` - {description}")));
            }
            let docs = Some(DocString {
                summary,
                details: Some(details.iter().flatten().join("\n\n")),
                examples: None,
            });
            DocItem::Member(DocMember::Function(DocFunction { docs, params, ret }))
        }
    }
}
