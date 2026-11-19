/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutMap;
use dupe::Dupe;
use serde::Serialize;
use starlark::docs::DocItem;
use starlark::docs::DocMember;
use starlark::docs::DocModule;
use starlark::typing::Ty;

use crate::starlark_::StarlarkFilePath;

fn serialize_ty<S: serde::Serializer>(ty: &Ty, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&ty.to_string())
}

fn serialize_opt_ty<S: serde::Serializer>(ty: &Option<Ty>, s: S) -> Result<S::Ok, S::Error> {
    match ty {
        Some(ty) => serialize_ty(ty, s),
        None => s.serialize_none(),
    }
}

#[derive(Serialize)]
struct JsonDoc {
    id: JsonIdentifier,
    item: JsonDocItem,
    custom_attrs: BuckMutMap<String, String>,
}

#[derive(Serialize)]
struct JsonIdentifier {
    name: String,
    location: Option<JsonLocation>,
}

#[derive(Serialize)]
struct JsonLocation {
    path: String,
}

impl JsonDoc {
    fn from_starlark(doc: Doc) -> Self {
        Self {
            id: JsonIdentifier {
                name: doc.name,
                location: Some(JsonLocation { path: doc.location }),
            },
            item: JsonDocItem::from_starlark(doc.item),
            custom_attrs: BuckMutMap::default(),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JsonDocItem {
    Module(JsonDocModule),
    Object(JsonDocObject),
    Function(JsonDocFunction),
    Property(JsonDocProperty),
}

impl JsonDocItem {
    fn from_starlark(item: starlark::docs::DocItem) -> Self {
        match item {
            starlark::docs::DocItem::Module(m) => Self::Module(JsonDocModule::from_starlark(m)),
            starlark::docs::DocItem::Type(o) => Self::Object(JsonDocObject::from_starlark(o)),
            starlark::docs::DocItem::Member(starlark::docs::DocMember::Function(f)) => {
                Self::Function(JsonDocFunction::from_starlark(f))
            }
            starlark::docs::DocItem::Member(starlark::docs::DocMember::Property(p)) => {
                Self::Property(JsonDocProperty::from_starlark(p))
            }
        }
    }
}

#[derive(Serialize)]
struct JsonDocModule {
    docs: Option<JsonDocString>,
    members: BuckMutInsertionMap<String, JsonDocMember>,
}

impl JsonDocModule {
    fn from_starlark(m: starlark::docs::DocModule) -> Self {
        Self {
            docs: m.docs.map(JsonDocString::from_starlark),
            members: m
                .members
                .into_iter()
                .filter_map(|(k, v)| match v {
                    starlark::docs::DocItem::Member(v) => {
                        Some((k, JsonDocMember::from_starlark(v)))
                    }
                    _ => None,
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct JsonDocObject {
    docs: Option<JsonDocString>,
    members: BuckMutInsertionMap<String, JsonDocMember>,
}

impl JsonDocObject {
    fn from_starlark(o: starlark::docs::DocType) -> Self {
        Self {
            docs: o.docs.map(JsonDocString::from_starlark),
            members: o
                .members
                .into_iter()
                .map(|(k, v)| (k, JsonDocMember::from_starlark(v)))
                .collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JsonDocMember {
    Property(JsonDocProperty),
    Function(JsonDocFunction),
}

impl JsonDocMember {
    fn from_starlark(m: starlark::docs::DocMember) -> Self {
        match m {
            starlark::docs::DocMember::Property(p) => {
                Self::Property(JsonDocProperty::from_starlark(p))
            }
            starlark::docs::DocMember::Function(f) => {
                Self::Function(JsonDocFunction::from_starlark(f))
            }
        }
    }
}

/// A single property of an object. These are explicitly not functions (see [`DocMember`]).
#[derive(Serialize)]
struct JsonDocProperty {
    docs: Option<JsonDocString>,
    #[serde(rename = "type", serialize_with = "serialize_ty")]
    typ: Ty,
}

impl JsonDocProperty {
    fn from_starlark(p: starlark::docs::DocProperty) -> Self {
        Self {
            docs: p.docs.map(JsonDocString::from_starlark),
            typ: p.typ,
        }
    }
}

#[derive(Serialize)]
struct JsonDocFunction {
    docs: Option<JsonDocString>,
    params: Vec<JsonDocParam>,
    ret: JsonDocReturn,
    #[serde(serialize_with = "serialize_opt_ty")]
    as_type: Option<Ty>,
}

impl JsonDocFunction {
    fn from_starlark(f: starlark::docs::DocFunction) -> Self {
        Self {
            docs: f.docs.map(JsonDocString::from_starlark),
            params: f
                .params
                .fmt_params()
                .map(JsonDocParam::from_starlark)
                .collect(),
            ret: JsonDocReturn::from_starlark(f.ret),
            as_type: None,
        }
    }
}

#[derive(Serialize)]
struct JsonDocReturn {
    docs: Option<JsonDocString>,
    #[serde(rename = "type", serialize_with = "serialize_ty")]
    typ: Ty,
}

impl JsonDocReturn {
    fn from_starlark(ret: starlark::docs::DocReturn) -> Self {
        Self {
            docs: ret.docs.map(JsonDocString::from_starlark),
            typ: ret.typ,
        }
    }
}

/// A single parameter of a function.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JsonDocParam {
    Arg {
        name: String,
        docs: Option<JsonDocString>,
        #[serde(rename = "type", serialize_with = "serialize_ty")]
        typ: Ty,
        default_value: Option<String>,
    },
    OnlyNamedAfter,
    OnlyPosBefore,
    Args {
        name: String,
        docs: Option<JsonDocString>,
        #[serde(rename = "type", serialize_with = "serialize_ty")]
        tuple_elem_ty: Ty,
    },
    Kwargs {
        name: String,
        docs: Option<JsonDocString>,
        #[serde(rename = "type", serialize_with = "serialize_ty")]
        dict_value_ty: Ty,
    },
}

impl JsonDocParam {
    fn from_starlark(param: starlark::docs::FmtParam<&'_ starlark::docs::DocParam>) -> Self {
        match param {
            starlark::docs::FmtParam::Regular(starlark::docs::DocParam {
                name,
                docs,
                typ,
                default_value,
            }) => Self::Arg {
                name: name.clone(),
                docs: docs.clone().map(JsonDocString::from_starlark),
                typ: typ.dupe(),
                default_value: default_value.clone(),
            },
            starlark::docs::FmtParam::Slash => Self::OnlyPosBefore,
            starlark::docs::FmtParam::Star => Self::OnlyNamedAfter,
            starlark::docs::FmtParam::Args(starlark::docs::DocParam {
                name,
                docs,
                typ,
                default_value: _,
            }) => Self::Args {
                name: name.clone(),
                docs: docs.clone().map(JsonDocString::from_starlark),
                tuple_elem_ty: typ.dupe(),
            },
            starlark::docs::FmtParam::Kwargs(starlark::docs::DocParam {
                name,
                docs,
                typ,
                default_value: _,
            }) => Self::Kwargs {
                name: name.clone(),
                docs: docs.clone().map(JsonDocString::from_starlark),
                dict_value_ty: typ.dupe(),
            },
        }
    }
}

#[derive(Serialize)]
struct JsonDocString {
    summary: String,
    details: Option<String>,
}

impl JsonDocString {
    fn from_starlark(s: starlark::docs::DocString) -> Self {
        Self {
            summary: s.summary,
            details: s.details,
        }
    }
}

// Note(JakobDegen): The particular format of the output is not really by design, but mostly a
// historical accident.
pub(crate) fn to_json(docs: Vec<(StarlarkFilePath, DocModule)>) -> buck2_error::Result<String> {
    let docs: Vec<_> = docs
        .into_iter()
        .flat_map(|(p, d)| to_docs_list(&p, d))
        .map(JsonDoc::from_starlark)
        .collect();
    Ok(serde_json::to_string(&docs)?)
}

struct Doc {
    name: String,
    location: String,
    item: DocItem,
}

fn to_docs_list(import_path: &StarlarkFilePath, module_docs: DocModule) -> Vec<Doc> {
    // Do this so that we don't get the '@' in the display if we're printing targets from a
    // different cell root. i.e. `//foo:bar.bzl`, rather than `//foo:bar.bzl @ cell`
    let import_path_string = format!(
        "{}:{}",
        import_path.path().parent().unwrap(),
        import_path.path().path().file_name().unwrap()
    );

    let mut docs = vec![];

    if let Some(module_doc) = module_docs.docs {
        docs.push(Doc {
            name: import_path_string.clone(),
            location: import_path_string.clone(),
            item: DocItem::Module(DocModule {
                docs: Some(module_doc),
                members: Default::default(),
            }),
        });
    }
    docs.extend(module_docs.members.into_iter().filter_map(|(symbol, d)| {
        Some(Doc {
            name: symbol,
            location: import_path_string.clone(),
            item: match d.try_as_member_with_collapsed_object().ok()? {
                DocMember::Function(f) => DocItem::Member(DocMember::Function(f)),
                DocMember::Property(p) => DocItem::Member(DocMember::Property(p)),
            },
        })
    }));

    docs
}
