/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The attribute value handed to query callbacks.

use serde::Serialize;
use serde::Serializer;

use crate::attrs::arena::CoercedAttrRef;
use crate::attrs::display::AttrDisplayWithContext;
use crate::attrs::fmt_context::AttrFmtContext;
use crate::attrs::serialize::AttrSerializeWithContext;
use crate::attrs::special_attr::SpecialAttr;

/// An attribute value handed to a callback: a normal attr, which the package
/// arena stores, or a special attr, which is built per call and served as
/// built — nothing reads a special attr's identity, so nothing is retained.
#[derive(Debug)]
pub enum AttrView<'a> {
    Normal(CoercedAttrRef<'a>),
    Special(SpecialAttr),
}

impl AttrView<'_> {
    pub fn any_matches(
        &self,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        match self {
            AttrView::Normal(r) => r.any_matches(filter),
            AttrView::Special(a) => a.any_matches(filter),
        }
    }
}

impl crate::attrs::display::AttrDisplayWithContext for AttrView<'_> {
    fn fmt(&self, ctx: &AttrFmtContext, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttrView::Normal(r) => AttrDisplayWithContext::fmt(r, ctx, f),
            AttrView::Special(a) => AttrDisplayWithContext::fmt(a, ctx, f),
        }
    }
}

impl AttrSerializeWithContext for AttrView<'_> {
    fn serialize_with_ctx<S>(&self, ctx: &AttrFmtContext, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            AttrView::Normal(r) => r.serialize_with_ctx(ctx, s),
            AttrView::Special(a) => a.to_json().map_err(serde::ser::Error::custom)?.serialize(s),
        }
    }
}
