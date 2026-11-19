/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Formatter;
use std::ops::Deref;

use allocative::Allocative;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_error::buck2_error;
use buck2_error::internal_error;
use buck2_util::arc_str::ArcSlice;
use display_container::fmt_keyed_container;
use pagable::Pagable;
use serde::Serializer;
use serde_json::Value;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::fields::ChildPairs;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::schema::encode_kind;
use crate::attrs::attr_type::AttrType;
use crate::attrs::attr_type::any_matches::AnyMatches;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::configure::TraverseConfigured;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced::render::AttrRefDisplay;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::render::ser_err;
use crate::attrs::coerced::render::ser_keyed;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::semantics::mismatch;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::configured_attr::ConfiguredAttr;
use crate::attrs::display::AttrDisplayWithContext;
use crate::attrs::display::AttrDisplayWithContextExt;
use crate::attrs::fmt_context::AttrFmtContext;
use crate::attrs::json::ToJsonWithContext;

#[derive(Debug, Hash, Pagable, Eq, PartialEq, Allocative, StrongHash)]
pub struct DictAttrType {
    pub key: AttrType,
    pub value: AttrType,
    pub sorted: bool,
}

impl DictAttrType {
    pub fn new(key: AttrType, value: AttrType, sorted: bool) -> Self {
        Self { key, value, sorted }
    }

    pub(crate) fn fmt_with_arg(&self, f: &mut fmt::Formatter<'_>, arg: &str) -> fmt::Result {
        write!(
            f,
            "attrs.dict({}, {}, sorted={}{})",
            self.key,
            self.value,
            if self.sorted { "True" } else { "False" },
            arg
        )
    }
}

#[derive(
    Debug, Clone, Eq, PartialEq, Hash, Allocative, Default, Pagable, StrongHash
)]
pub struct DictLiteral<C: Eq>(pub ArcSlice<(C, C)>);

impl<C: Eq> Deref for DictLiteral<C> {
    type Target = ArcSlice<(C, C)>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C: Eq + AttrDisplayWithContext> AttrDisplayWithContext for DictLiteral<C> {
    fn fmt(&self, ctx: &AttrFmtContext, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_keyed_container(
            f,
            "{",
            "}",
            ": ",
            self.0
                .iter()
                .map(|(k, v)| (k.as_display(ctx), v.as_display(ctx))),
        )
    }
}

impl<C: Eq> FromIterator<(C, C)> for DictLiteral<C> {
    fn from_iter<T: IntoIterator<Item = (C, C)>>(iter: T) -> Self {
        DictLiteral(ArcSlice::from_iter(iter))
    }
}

impl<C: Eq + AnyMatches> AnyMatches for DictLiteral<C> {
    fn any_matches(
        &self,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        for (k, v) in self.0.iter() {
            if k.any_matches(filter)? || v.any_matches(filter)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl<C: Eq + ToJsonWithContext> ToJsonWithContext for DictLiteral<C> {
    fn to_json(&self, ctx: &AttrFmtContext) -> buck2_error::Result<Value> {
        let mut res: serde_json::Map<String, serde_json::Value> =
            serde_json::Map::with_capacity(self.len());
        for (k, v) in self.iter() {
            res.insert(
                k.to_json(ctx)?
                    .as_str()
                    .ok_or_else(|| {
                        // FIXME(JakobDegen): This can't actually error, we need to ban it or
                        // serialize it somehow
                        buck2_error!(
                            buck2_error::ErrorTag::Input,
                            "Cannot serialize dict attr with non-string key type"
                        )
                    })?
                    .to_owned(),
                v.to_json(ctx)?,
            );
        }
        Ok(res.into())
    }
}

impl Kind for kinds::Dict {
    type Fields = ChildPairs;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_dict(
        &mut self,
        pairs: &[(AttrToken, AttrToken)],
    ) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::Dict>(self, pairs)
    }
}

impl Semantics for kinds::Dict {
    const CONCAT: Option<&'static str> = Some("dict");

    fn traverse<'a>(
        pairs: View<'a, Self>,
        Traverse { v, t, pkg, tr }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        let (kt, vt) = t.dict_items().ok_or_else(|| mismatch(v, t))?;
        for (k, val) in pairs.iter() {
            k.traverse(kt, pkg, tr)?;
            val.traverse(vt, pkg, tr)?;
        }
        Ok(())
    }

    fn configure(
        pairs: View<'_, Self>,
        Configure { v, t, ctx, name }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        let (kt, vt) = t.dict_items().ok_or_else(|| mismatch(v, t))?;
        ResultMaybeCompatible::Compatible(ConfiguredAttr::Dict(DictLiteral(
            pairs
                .iter()
                .map(|(k, val)| {
                    let k2 = k.configure(kt, ctx, name)?;
                    let v2 = val.configure(vt, ctx, name)?;
                    ResultMaybeCompatible::Compatible((k2, v2))
                })
                .collect::<ResultMaybeCompatible<Vec<_>>>()?
                .into(),
        )))
    }

    fn traverse_configured(
        pairs: View<'_, Self>,
        TraverseConfigured {
            v,
            t,
            pkg,
            ctx,
            tr,
            name,
        }: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        let (kt, vt) = t.dict_items().ok_or_else(|| mismatch(v, t))?;
        for (k, val) in pairs.iter() {
            k.traverse_configured(kt, pkg, ctx, tr, name)?;
            val.traverse_configured(vt, pkg, ctx, tr, name)?;
        }
        ResultMaybeCompatible::Compatible(())
    }

    fn to_json(
        pairs: View<'_, Self>,
        ToJson { ctx }: ToJson<'_>,
    ) -> buck2_error::Result<serde_json::Value> {
        let mut res = serde_json::Map::with_capacity(pairs.len());
        for (k, val) in pairs.iter() {
            res.insert(
                k.to_json(ctx)?
                    .as_str()
                    .ok_or_else(|| internal_error!("dict keys must render to JSON strings"))?
                    .to_owned(),
                val.to_json(ctx)?,
            );
        }
        Ok(serde_json::Value::Object(res))
    }

    fn fmt(pairs: View<'_, Self>, Fmt { ctx, f }: Fmt<'_, '_>) -> std::fmt::Result {
        fmt_keyed_container(
            f,
            "{",
            "}",
            ": ",
            pairs
                .iter()
                .map(|(k, val)| (AttrRefDisplay { ctx, r: k }, AttrRefDisplay { ctx, r: val })),
        )
    }

    fn any_matches(
        pairs: View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        for (k, val) in pairs.iter() {
            if k.any_matches(filter)? || val.any_matches(filter)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn serialize<S: Serializer>(
        pairs: View<'_, Self>,
        SerializeKind { ctx, ser }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        ser_keyed(
            pairs
                .iter()
                .map(|(k, val)| Ok((k.json_key(ctx)?, val)))
                .collect::<buck2_error::Result<Vec<_>>>()
                .map_err(ser_err::<S>)?
                .into_iter(),
            ctx,
            ser,
        )
    }
}
