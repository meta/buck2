/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Rendering of coerced values: display, JSON and substring search.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Display;

use buck2_core::provider::label::ProvidersLabel;
use display_container::fmt_container;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeMap;
use serde::ser::SerializeSeq;
use serde_json::to_value;

use crate::attrs::arena::CoercedAttrRef;
use crate::attrs::arena::Refs;
use crate::attrs::arena::Values;
use crate::attrs::arena::schema::View;
use crate::attrs::coerced::kinds::ValueVisitor;
use crate::attrs::coerced::kinds::dispatch_value;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::display::AttrDisplayWithContext;
use crate::attrs::display::AttrDisplayWithContextExt;
use crate::attrs::fmt_context::AttrFmtContext;
use crate::attrs::serialize::AttrSerializeWithContext;

impl<'a> CoercedAttrRef<'a> {
    /// The value's JSON rendering.
    pub fn to_json(
        &self,
        ctx: &crate::attrs::fmt_context::AttrFmtContext,
    ) -> buck2_error::Result<serde_json::Value> {
        dispatch_value(*self, ToJson { ctx })
    }

    /// Whether any leaf's string form matches the filter.
    pub fn any_matches(
        &self,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        dispatch_value(*self, MatchesAny { filter })
    }

    /// The value's rendering when it is used as a JSON object key.
    ///
    /// Errors for values that render as anything but a string.
    pub(crate) fn json_key(&self, ctx: &AttrFmtContext) -> buck2_error::Result<Cow<'a, str>> {
        dispatch_value(*self, JsonKey { ctx })
    }
}

/// `as_display` for owned [`CoercedAttrRef`]s: the standard adapter borrows its
/// value, which cannot outlive a closure mapping `Copy` views.
pub(crate) struct AttrRefDisplay<'a, 'c> {
    pub(crate) ctx: &'c AttrFmtContext,
    pub(crate) r: CoercedAttrRef<'a>,
}

impl Display for AttrRefDisplay<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        AttrDisplayWithContext::fmt(&self.r, self.ctx, f)
    }
}

impl crate::attrs::display::AttrDisplayWithContext for CoercedAttrRef<'_> {
    fn fmt(
        &self,
        ctx: &crate::attrs::fmt_context::AttrFmtContext,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        dispatch_value(*self, Fmt { ctx, f })
    }
}

impl std::fmt::Debug for CoercedAttrRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CoercedAttrRef({})", self.as_display_no_ctx())
    }
}

/// A value rendered in place of its parent, so nested values serialize without
/// an intermediate tree.
pub(super) struct AttrRefSerialize<'a> {
    pub(super) attr: CoercedAttrRef<'a>,
    pub(super) ctx: &'a AttrFmtContext,
}

impl Serialize for AttrRefSerialize<'_> {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.attr.serialize_with_ctx(self.ctx, s)
    }
}

impl AttrSerializeWithContext for CoercedAttrRef<'_> {
    fn serialize_with_ctx<S>(&self, ctx: &AttrFmtContext, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        dispatch_value(*self, SerializeKind { ctx, ser: s })
    }
}

pub(crate) fn ser_err<S: Serializer>(e: buck2_error::Error) -> S::Error {
    serde::ser::Error::custom(format!("{e}"))
}

/// Keyed values collect into a `BTreeMap` to match `serde_json::Map`: keys
/// emit in sorted order, and a repeated key keeps its last value.
pub(crate) fn ser_keyed<'a, S: Serializer>(
    entries: impl Iterator<Item = (Cow<'a, str>, CoercedAttrRef<'a>)>,
    ctx: &'a AttrFmtContext,
    s: S,
) -> Result<S::Ok, S::Error> {
    let sorted: BTreeMap<Cow<'a, str>, CoercedAttrRef<'a>> = entries.collect();
    let mut map = s.serialize_map(Some(sorted.len()))?;
    for (k, v) in sorted {
        map.serialize_entry(&k, &AttrRefSerialize { attr: v, ctx })?;
    }
    map.end()
}

pub(crate) fn ser_seq<'a, S: Serializer>(
    items: Refs<'a, Values>,
    ctx: &'a AttrFmtContext,
    s: S,
) -> Result<S::Ok, S::Error> {
    let mut seq = s.serialize_seq(Some(items.len()))?;
    for v in items.iter() {
        seq.serialize_element(&AttrRefSerialize { attr: v, ctx })?;
    }
    seq.end()
}

/// `String` and `EnumVariant` render alike.
pub(crate) fn fmt_text(
    s: &str,
    ctx: &AttrFmtContext,
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    if ctx.options.exclude_quotes {
        f.write_str(s)
    } else {
        write!(f, "\"{s}\"")
    }
}

pub(crate) fn fmt_seq(
    items: Refs<'_, Values>,
    open: &str,
    close: &str,
    ctx: &AttrFmtContext,
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    fmt_container(
        f,
        open,
        close,
        items.iter().map(|v| AttrRefDisplay { ctx, r: v }),
    )
}

pub(crate) fn json_seq(
    items: Refs<'_, Values>,
    ctx: &AttrFmtContext,
) -> buck2_error::Result<serde_json::Value> {
    Ok(serde_json::Value::Array(
        items
            .iter()
            .map(|v| v.to_json(ctx))
            .collect::<buck2_error::Result<_>>()?,
    ))
}

pub(crate) fn any_matches_seq(
    items: Refs<'_, Values>,
    filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
) -> buck2_error::Result<bool> {
    for v in items.iter() {
        if v.any_matches(filter)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn json_label(l: &ProvidersLabel) -> buck2_error::Result<serde_json::Value> {
    Ok(to_value(l.to_string())?)
}

pub(crate) struct ToJson<'x> {
    pub(crate) ctx: &'x AttrFmtContext,
}

impl<'a> ValueVisitor<'a> for ToJson<'_> {
    type Out = buck2_error::Result<serde_json::Value>;
    fn visit<K: Semantics + 'static>(self, view: View<'a, K>) -> Self::Out {
        K::to_json(view, self)
    }
}

pub(crate) struct Fmt<'x, 'y> {
    pub(crate) ctx: &'x AttrFmtContext,
    pub(crate) f: &'x mut std::fmt::Formatter<'y>,
}

impl<'a> ValueVisitor<'a> for Fmt<'_, '_> {
    type Out = std::fmt::Result;
    fn visit<K: Semantics + 'static>(self, view: View<'a, K>) -> Self::Out {
        K::fmt(view, self)
    }
}

pub(crate) struct MatchesAny<'x> {
    pub(crate) filter: &'x dyn Fn(&str) -> buck2_error::Result<bool>,
}

impl<'a> ValueVisitor<'a> for MatchesAny<'_> {
    type Out = buck2_error::Result<bool>;
    fn visit<K: Semantics + 'static>(self, view: View<'a, K>) -> Self::Out {
        K::any_matches(view, self)
    }
}

pub(crate) struct JsonKey<'x> {
    pub(crate) ctx: &'x AttrFmtContext,
}

impl<'a> ValueVisitor<'a> for JsonKey<'_> {
    type Out = buck2_error::Result<Cow<'a, str>>;
    fn visit<K: Semantics + 'static>(self, view: View<'a, K>) -> Self::Out {
        K::json_key(view, self)
    }
}

/// [`AttrSerializeWithContext`] for a value: generic over the serializer.
pub(crate) struct SerializeKind<'x, S> {
    pub(crate) ctx: &'x AttrFmtContext,
    pub(crate) ser: S,
}

impl<'a, S: Serializer> ValueVisitor<'a> for SerializeKind<'_, S> {
    type Out = Result<S::Ok, S::Error>;
    fn visit<K: Semantics + 'static>(self, view: View<'a, K>) -> Self::Out {
        K::serialize(view, self)
    }
}
