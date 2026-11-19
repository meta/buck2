/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_build_api::artifact_groups::promise::PromiseArtifactAttr;
use buck2_core::package::PackageLabel;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_node::attrs::arena::CoercedAttrRef;
use buck2_node::attrs::attr_type::AttrType;
use buck2_node::attrs::attr_type::AttrTypeInner;
use buck2_node::attrs::attr_type::arg::ConfiguredStringWithMacros;
use buck2_node::attrs::attr_type::bool::BoolLiteral;
use buck2_node::attrs::attr_type::dep::DepAttr;
use buck2_node::attrs::attr_type::dict::DictLiteral;
use buck2_node::attrs::attr_type::list::ListLiteral;
use buck2_node::attrs::attr_type::string::StringLiteral;
use buck2_node::attrs::attr_type::tuple::TupleLiteral;
use buck2_node::attrs::coerced::CoercedAttrRefKind;
use buck2_node::attrs::configured_traversal::ConfiguredAttrTraversal;
use buck2_node::attrs::display::AttrDisplayWithContext;
use buck2_node::attrs::fmt_context::AttrFmtContext;
use buck2_node::attrs::json::ToJsonWithContext;
use buck2_node::attrs::serialize::AttrSerializeWithContext;
use buck2_util::arc_str::ArcStr;
use buck2_util::size_assert;
use pagable::Pagable;
use serde::Serialize;
use serde::Serializer;
use serde_json::to_value;

use crate::anon_target_attr_resolve::AnonTargetAttrTraversal;

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
pub enum AnonTargetAttr {
    Bool(BoolLiteral),
    Int(i64),
    // Note we store `String`, not `Arc<str>` here, because we store full attributes
    // in unconfigured target node, but configured target node is basically a pair
    // (reference to unconfigured target node, configuration).
    //
    // Configured attributes are created on demand and destroyed immediately after use.
    //
    // So when working with configured attributes with pay with CPU for string copies,
    // but don't increase total memory usage, because these string copies are short living.
    String(StringLiteral),
    EnumVariant(StringLiteral),
    List(ListLiteral<AnonTargetAttr>),
    Tuple(TupleLiteral<AnonTargetAttr>),
    Dict(DictLiteral<AnonTargetAttr>),
    None,
    OneOf(
        Box<Self>,
        // Index of matched oneof attr type variant.
        u32,
    ),
    Dep(Box<DepAttr<ConfiguredProvidersLabel>>),
    // Accepts any bound artifacts. Maps to `attr.source()`.
    Artifact(Artifact),
    // Accepts unresolved promise artifacts. Maps to `attr.source()`.
    PromiseArtifact(Box<PromiseArtifactAttr>),
    Arg(Box<ConfiguredStringWithMacros>),
    Label(ProvidersLabel),
}

// Anon target attrs live in the `AnonTarget` DICE key, which is retained for
// the daemon lifetime, and large anon targets (e.g. anonymous link groups)
// hold millions of these inline in lists and tuples — so every inline word
// here is paid at scale. Fat payloads must be boxed (`Dep`, `PromiseArtifact`,
// `Arg`).
size_assert::words_of_type!(AnonTargetAttr, 3);

impl AttrSerializeWithContext for AnonTargetAttr {
    fn serialize_with_ctx<S>(&self, ctx: &AttrFmtContext, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // TODO this is inefficient. We should impl Serialize and derive value from this instead.
        self.to_json(ctx)
            .map_err(|e| serde::ser::Error::custom(format!("{e}")))?
            .serialize(s)
    }
}

impl AttrDisplayWithContext for AnonTargetAttr {
    fn fmt(&self, ctx: &AttrFmtContext, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnonTargetAttr::Bool(v) => {
                write!(f, "{v}")
            }
            AnonTargetAttr::Int(v) => {
                write!(f, "{v}")
            }
            AnonTargetAttr::String(v) | AnonTargetAttr::EnumVariant(v) => {
                AttrDisplayWithContext::fmt(v, ctx, f)
            }
            AnonTargetAttr::List(list) => AttrDisplayWithContext::fmt(list, ctx, f),
            AnonTargetAttr::Tuple(v) => AttrDisplayWithContext::fmt(v, ctx, f),
            AnonTargetAttr::Dict(v) => AttrDisplayWithContext::fmt(v, ctx, f),
            AnonTargetAttr::None => write!(f, "None"),
            AnonTargetAttr::OneOf(box l, _) => AttrDisplayWithContext::fmt(l, ctx, f),
            AnonTargetAttr::Dep(e) => write!(f, "\"{e}\""),
            AnonTargetAttr::Artifact(e) => write!(f, "\"{e}\""),
            AnonTargetAttr::Arg(e) => write!(f, "\"{e}\""),
            AnonTargetAttr::PromiseArtifact(e) => write!(f, "\"{e}\""),
            AnonTargetAttr::Label(e) => write!(f, "\"{e}\""),
        }
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum AnonTargetAttrError {
    #[error("Inconsistent number of elements in tuple")]
    InconsistentTupleLength,
}

impl ToJsonWithContext for AnonTargetAttr {
    fn to_json(&self, ctx: &AttrFmtContext) -> buck2_error::Result<serde_json::Value> {
        match self {
            AnonTargetAttr::Bool(v) => Ok(to_value(v)?),
            AnonTargetAttr::Int(v) => Ok(to_value(v)?),
            AnonTargetAttr::String(v) | AnonTargetAttr::EnumVariant(v) => Ok(to_value(v)?),
            AnonTargetAttr::List(list) => list.to_json(ctx),
            AnonTargetAttr::Tuple(list) => list.to_json(ctx),
            AnonTargetAttr::Dict(dict) => dict.to_json(ctx),
            AnonTargetAttr::None => Ok(serde_json::Value::Null),
            AnonTargetAttr::OneOf(box l, _) => l.to_json(ctx),
            AnonTargetAttr::Dep(e) => Ok(to_value(e.to_string())?),
            AnonTargetAttr::Artifact(e) => Ok(to_value(e.to_string())?),
            AnonTargetAttr::Arg(e) => Ok(to_value(e.to_string())?),
            AnonTargetAttr::PromiseArtifact(e) => Ok(to_value(e.to_string())?),
            AnonTargetAttr::Label(v) => Ok(to_value(v)?),
        }
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub(crate) enum AnonTargetFromCoercedAttrError {
    #[error("Anon targets do not support default values for `{0}`, specify `{1}` explicitly")]
    DefaultAttrTypeNotSupported(String, String),
}

impl AnonTargetAttr {
    /// Traverses the anon target attribute and calls the traverse for every encountered target label (in deps, sources, or other places).
    pub fn traverse(
        &self,
        pkg: PackageLabel,
        traversal: &mut dyn ConfiguredAttrTraversal,
    ) -> buck2_error::Result<()> {
        match self {
            AnonTargetAttr::Bool(_) => Ok(()),
            AnonTargetAttr::Int(_) => Ok(()),
            AnonTargetAttr::String(_) => Ok(()),
            AnonTargetAttr::EnumVariant(_) => Ok(()),
            AnonTargetAttr::List(list) => {
                for v in list.iter() {
                    v.traverse(pkg, traversal)?;
                }
                Ok(())
            }
            AnonTargetAttr::Tuple(list) => {
                for v in list.iter() {
                    v.traverse(pkg, traversal)?;
                }
                Ok(())
            }
            AnonTargetAttr::Dict(dict) => {
                for (k, v) in dict.iter() {
                    k.traverse(pkg, traversal)?;
                    v.traverse(pkg, traversal)?;
                }
                Ok(())
            }
            AnonTargetAttr::None => Ok(()),
            AnonTargetAttr::OneOf(l, _) => l.traverse(pkg, traversal),
            AnonTargetAttr::Dep(dep) => traversal.dep(&dep.label),
            AnonTargetAttr::Artifact(_) => Ok(()),
            AnonTargetAttr::Arg(e) => e.string_with_macros.traverse(traversal, pkg),
            AnonTargetAttr::PromiseArtifact(..) => Ok(()),
            AnonTargetAttr::Label(_) => Ok(()),
        }
    }

    pub(crate) fn traverse_anon_attr(
        &self,
        traversal: &mut dyn AnonTargetAttrTraversal,
    ) -> buck2_error::Result<()> {
        match self {
            AnonTargetAttr::Bool(_) => Ok(()),
            AnonTargetAttr::Int(_) => Ok(()),
            AnonTargetAttr::String(_) => Ok(()),
            AnonTargetAttr::EnumVariant(_) => Ok(()),
            AnonTargetAttr::List(list) => {
                for v in list.iter() {
                    v.traverse_anon_attr(traversal)?;
                }
                Ok(())
            }
            AnonTargetAttr::Tuple(list) => {
                for v in list.iter() {
                    v.traverse_anon_attr(traversal)?;
                }
                Ok(())
            }
            AnonTargetAttr::Dict(dict) => {
                for (k, v) in dict.iter() {
                    k.traverse_anon_attr(traversal)?;
                    v.traverse_anon_attr(traversal)?;
                }
                Ok(())
            }
            AnonTargetAttr::None => Ok(()),
            AnonTargetAttr::OneOf(l, _) => l.traverse_anon_attr(traversal),
            AnonTargetAttr::Dep(_) => Ok(()),
            AnonTargetAttr::Artifact(_) => Ok(()),
            AnonTargetAttr::Arg(_) => Ok(()),
            AnonTargetAttr::PromiseArtifact(promise_artifact) => {
                traversal.promise_artifact(promise_artifact)
            }
            AnonTargetAttr::Label(_) => Ok(()),
        }
    }

    pub fn _unpack_list(&self) -> Option<&[AnonTargetAttr]> {
        match self {
            AnonTargetAttr::List(list) => Some(list),
            _ => None,
        }
    }

    // We need this method for attr defaults in anon targets, which are
    // served as arena views like every other attribute value. Note that
    // non-primitives, such as dep/source, will result in an error.
    pub(crate) fn from_attr_ref(
        attr_name: &str,
        r: CoercedAttrRef<'_>,
        ty: &AttrType,
    ) -> buck2_error::Result<AnonTargetAttr> {
        let unsupported = || -> buck2_error::Error {
            AnonTargetFromCoercedAttrError::DefaultAttrTypeNotSupported(
                ty.to_string(),
                attr_name.to_owned(),
            )
            .into()
        };
        Ok(match (&ty.0.inner, r.kind()) {
            // `Any`: structure comes from the value alone.
            (AttrTypeInner::Any(_), kind) => match kind {
                CoercedAttrRefKind::None => AnonTargetAttr::None,
                CoercedAttrRefKind::Bool(v) => AnonTargetAttr::Bool(BoolLiteral(v)),
                CoercedAttrRefKind::Int(v) => AnonTargetAttr::Int(v),
                CoercedAttrRefKind::String(v) => {
                    AnonTargetAttr::String(StringLiteral(ArcStr::from(v)))
                }
                CoercedAttrRefKind::List(items) => AnonTargetAttr::List(
                    items
                        .iter()
                        .map(|v| AnonTargetAttr::from_attr_ref(attr_name, v, ty))
                        .collect::<buck2_error::Result<_>>()?,
                ),
                CoercedAttrRefKind::Tuple(items) => AnonTargetAttr::Tuple(
                    items
                        .iter()
                        .map(|v| AnonTargetAttr::from_attr_ref(attr_name, v, ty))
                        .collect::<buck2_error::Result<_>>()?,
                ),
                CoercedAttrRefKind::Dict(pairs) => AnonTargetAttr::Dict(
                    pairs
                        .iter()
                        .map(|(k, v)| {
                            let k2 = AnonTargetAttr::from_attr_ref(attr_name, k, ty)?;
                            let v2 = AnonTargetAttr::from_attr_ref(attr_name, v, ty)?;
                            buck2_error::Ok((k2, v2))
                        })
                        .collect::<buck2_error::Result<_>>()?,
                ),
                _ => return Err(unsupported()),
            },
            (AttrTypeInner::Bool(_), CoercedAttrRefKind::Bool(v)) => {
                AnonTargetAttr::Bool(BoolLiteral(v))
            }
            (AttrTypeInner::Int(_), CoercedAttrRefKind::Int(v)) => AnonTargetAttr::Int(v),
            (AttrTypeInner::String(_), CoercedAttrRefKind::String(v)) => {
                AnonTargetAttr::String(StringLiteral(ArcStr::from(v)))
            }
            (AttrTypeInner::Enum(_), CoercedAttrRefKind::EnumVariant(v)) => {
                AnonTargetAttr::EnumVariant(StringLiteral(ArcStr::from(v)))
            }
            (AttrTypeInner::List(t), CoercedAttrRefKind::List(items)) => AnonTargetAttr::List(
                items
                    .iter()
                    .map(|v| AnonTargetAttr::from_attr_ref(attr_name, v, &t.inner))
                    .collect::<buck2_error::Result<_>>()?,
            ),
            (AttrTypeInner::Tuple(t), CoercedAttrRefKind::Tuple(items)) => {
                if items.len() != t.xs.len() {
                    return Err(AnonTargetAttrError::InconsistentTupleLength.into());
                }
                AnonTargetAttr::Tuple(
                    items
                        .iter()
                        .zip(&t.xs)
                        .map(|(v, vt)| AnonTargetAttr::from_attr_ref(attr_name, v, vt))
                        .collect::<buck2_error::Result<_>>()?,
                )
            }
            (AttrTypeInner::Dict(t), CoercedAttrRefKind::Dict(pairs)) => AnonTargetAttr::Dict(
                pairs
                    .iter()
                    .map(|(k, v)| {
                        let k2 = AnonTargetAttr::from_attr_ref(attr_name, k, &t.key)?;
                        let v2 = AnonTargetAttr::from_attr_ref(attr_name, v, &t.value)?;
                        buck2_error::Ok((k2, v2))
                    })
                    .collect::<buck2_error::Result<_>>()?,
            ),
            (AttrTypeInner::Option(_), CoercedAttrRefKind::None) => AnonTargetAttr::None,
            (AttrTypeInner::Option(t), _) => AnonTargetAttr::from_attr_ref(attr_name, r, &t.inner)?,
            (AttrTypeInner::OneOf(t), CoercedAttrRefKind::OneOf((i, l))) => {
                let item_ty = &t.xs[i as usize];
                let inner = AnonTargetAttr::from_attr_ref(attr_name, l, item_ty)?;
                AnonTargetAttr::OneOf(Box::new(inner), i)
            }
            _ => return Err(unsupported()),
        })
    }
}
