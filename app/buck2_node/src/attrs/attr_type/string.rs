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
use std::ops::Deref;

use allocative::Allocative;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_util::arc_str::ArcStr;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serialize;
use serde::Serializer;
use serde_json::to_value;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::fields::Text;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::schema::encode_kind;
use crate::attrs::attr_type::AttrTypeInner;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::configure::TraverseConfigured;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::JsonKey;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::render::fmt_text;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::semantics::mismatch;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::configured_attr::ConfiguredAttr;
use crate::attrs::display::AttrDisplayWithContext;
use crate::attrs::fmt_context::AttrFmtContext;

#[derive(
    Debug, Pagable, Eq, PartialEq, Hash, Allocative, Clone, Copy, Dupe, StrongHash
)]
pub struct StringAttrType;

#[derive(
    Default, Debug, Eq, PartialEq, Hash, Clone, Dupe, Allocative, Serialize, Pagable, StrongHash
)]
#[serde(transparent)]
pub struct StringLiteral(pub ArcStr);

impl Deref for StringLiteral {
    type Target = ArcStr;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AttrDisplayWithContext for StringLiteral {
    fn fmt(&self, ctx: &AttrFmtContext, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if ctx.options.exclude_quotes {
            f.write_str(self.0.as_str())
        } else {
            write!(f, "\"{}\"", self.0)
        }
    }
}

impl Kind for kinds::String {
    type Fields = Text;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_string(&mut self, s: &str) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::String>(self, s)
    }
}

impl Semantics for kinds::String {
    const CONCAT: Option<&'static str> = Some("string");

    fn traverse<'a>(_: View<'a, Self>, _: Traverse<'_, 'a>) -> buck2_error::Result<()> {
        Ok(())
    }

    fn configure(
        s: View<'_, Self>,
        Configure { v, t, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::String(_) | AttrTypeInner::Any(_) => ResultMaybeCompatible::Compatible(
                ConfiguredAttr::String(StringLiteral(ArcStr::from(s))),
            ),
            _ => mismatch(v, t).into(),
        }
    }

    fn traverse_configured(
        _: View<'_, Self>,
        _: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        ResultMaybeCompatible::Compatible(())
    }

    fn to_json(s: View<'_, Self>, _: ToJson<'_>) -> buck2_error::Result<serde_json::Value> {
        Ok(to_value(s)?)
    }

    fn fmt(s: View<'_, Self>, Fmt { ctx, f }: Fmt<'_, '_>) -> std::fmt::Result {
        fmt_text(s, ctx, f)
    }

    fn any_matches(
        s: View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        filter(s)
    }

    fn json_key<'a>(s: View<'a, Self>, _: JsonKey<'_>) -> buck2_error::Result<Cow<'a, str>> {
        Ok(Cow::Borrowed(s))
    }

    fn serialize<S: Serializer>(
        s: View<'_, Self>,
        SerializeKind { ser, .. }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        ser.serialize_str(s)
    }
}
