/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::fmt;

use allocative::Allocative;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use pagable::Pagable;
use serde::Serializer;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::fields::Child;
use crate::attrs::arena::fields::U32;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::schema::encode_kind;
use crate::attrs::attr_type::AttrType;
use crate::attrs::attr_type::AttrTypeInner;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::configure::TraverseConfigured;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::JsonKey;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::semantics::mismatch;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::configured_attr::ConfiguredAttr;
use crate::attrs::display::AttrDisplayWithContext;
use crate::attrs::serialize::AttrSerializeWithContext;

#[derive(Debug, Eq, PartialEq, Hash, Pagable, Allocative, StrongHash)]
pub struct OneOfAttrType {
    pub xs: Vec<AttrType>,
}

impl OneOfAttrType {
    pub fn new(xs: Vec<AttrType>) -> OneOfAttrType {
        Self { xs }
    }

    pub fn fmt_with_arg(&self, f: &mut fmt::Formatter<'_>, arg: &str) -> fmt::Result {
        write!(f, "attrs.one_of(")?;
        for (i, x) in self.xs.iter().enumerate() {
            if i != 0 {
                write!(f, ", ")?;
            }
            write!(f, "{x}")?;
        }
        write!(f, "{arg})")
    }

    pub(crate) fn any_supports_concat(&self) -> bool {
        self.xs.iter().any(AttrType::supports_concat)
    }

    pub(crate) fn get(&self, i: u32) -> buck2_error::Result<&AttrType> {
        self.xs
            .get(i as usize)
            .with_internal_error(|| format!("Oneof index ({i}) out of bounds (internal error)"))
    }
}

impl Kind for kinds::OneOf {
    type Fields = (U32, Child);
}

impl CoercedAttrArenaBuilder {
    pub fn encode_one_of(&mut self, idx: u32, inner: AttrToken) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::OneOf>(self, (idx, inner))
    }
}

impl Semantics for kinds::OneOf {
    fn traverse<'a>(
        (idx, inner): View<'a, Self>,
        Traverse { v, t, pkg, tr }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        match &t.non_optional().0.inner {
            AttrTypeInner::OneOf(ty) => {
                let item_t = ty
                    .xs
                    .get(idx as usize)
                    .ok_or_else(|| internal_error!("invalid oneof index"))?;
                inner.traverse(item_t, pkg, tr)
            }
            _ => Err(mismatch(v, t)),
        }
    }

    fn configure(
        (idx, inner): View<'_, Self>,
        Configure { v, t, ctx, name }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::OneOf(ty) => {
                let item_t = match ty.xs.get(idx as usize) {
                    Some(ty) => ty,
                    None => return internal_error!("invalid oneof index").into(),
                };
                let configured = inner.configure(item_t, ctx, name)?;
                ResultMaybeCompatible::Compatible(ConfiguredAttr::OneOf(Box::new(configured), idx))
            }
            _ => mismatch(v, t).into(),
        }
    }

    fn traverse_configured(
        (idx, inner): View<'_, Self>,
        TraverseConfigured {
            v,
            t,
            pkg,
            ctx,
            tr,
            name,
        }: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        match &t.non_optional().0.inner {
            AttrTypeInner::OneOf(ty) => {
                let item_t = match ty.xs.get(idx as usize) {
                    Some(ty) => ty,
                    None => return internal_error!("invalid oneof index").into(),
                };
                inner.traverse_configured(item_t, pkg, ctx, tr, name)
            }
            _ => mismatch(v, t).into(),
        }
    }

    fn to_json(
        (_, inner): View<'_, Self>,
        ToJson { ctx }: ToJson<'_>,
    ) -> buck2_error::Result<serde_json::Value> {
        inner.to_json(ctx)
    }

    fn fmt((_, inner): View<'_, Self>, Fmt { ctx, f }: Fmt<'_, '_>) -> std::fmt::Result {
        AttrDisplayWithContext::fmt(&inner, ctx, f)
    }

    fn any_matches(
        (_, inner): View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        inner.any_matches(filter)
    }

    fn json_key<'a>(
        (_, inner): View<'a, Self>,
        JsonKey { ctx }: JsonKey<'_>,
    ) -> buck2_error::Result<Cow<'a, str>> {
        inner.json_key(ctx)
    }

    fn serialize<S: Serializer>(
        (_, inner): View<'_, Self>,
        SerializeKind { ctx, ser }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        inner.serialize_with_ctx(ctx, ser)
    }
}
