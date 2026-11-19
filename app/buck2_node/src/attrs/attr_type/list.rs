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
use buck2_util::arc_str::ArcSlice;
use display_container::fmt_container;
use pagable::Pagable;
use serde::Serializer;
use serde_json::Value;
use serde_json::to_value;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::fields::Children;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::schema::encode_kind;
use crate::attrs::attr_type::AttrType;
use crate::attrs::attr_type::any_matches::AnyMatches;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::configure::TraverseConfigured;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::render::any_matches_seq;
use crate::attrs::coerced::render::fmt_seq;
use crate::attrs::coerced::render::json_seq;
use crate::attrs::coerced::render::ser_seq;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::semantics::mismatch;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::configured_attr::ConfiguredAttr;
use crate::attrs::display::AttrDisplayWithContext;
use crate::attrs::display::AttrDisplayWithContextExt;
use crate::attrs::fmt_context::AttrFmtContext;
use crate::attrs::json::ToJsonWithContext;

#[derive(Debug, Hash, Pagable, Eq, PartialEq, Allocative, StrongHash)]
pub struct ListAttrType {
    pub inner: AttrType,
}

impl ListAttrType {
    pub(crate) fn new(inner: AttrType) -> Self {
        Self { inner }
    }

    pub(crate) fn fmt_with_arg(&self, f: &mut fmt::Formatter<'_>, arg: &str) -> fmt::Result {
        write!(f, "attrs.list({}{})", self.inner, arg)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Allocative, Pagable, StrongHash)]
#[pagable(bound = "C: Eq + Pagable")]
pub struct ListLiteral<C: Eq>(pub ArcSlice<C>);

impl<C: Eq> Default for ListLiteral<C> {
    fn default() -> Self {
        Self(ArcSlice::new([]))
    }
}

impl<C: Eq> Deref for ListLiteral<C> {
    type Target = ArcSlice<C>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C: Eq + AttrDisplayWithContext> AttrDisplayWithContext for ListLiteral<C> {
    fn fmt(&self, ctx: &AttrFmtContext, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_container(f, "[", "]", self.0.iter().map(|v| v.as_display(ctx)))
    }
}

impl<C: Eq> FromIterator<C> for ListLiteral<C> {
    fn from_iter<T: IntoIterator<Item = C>>(iter: T) -> Self {
        ListLiteral(ArcSlice::from_iter(iter))
    }
}

impl<C: Eq + AnyMatches> AnyMatches for ListLiteral<C> {
    fn any_matches(
        &self,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        for v in self.0.iter() {
            if v.any_matches(filter)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl<C: Eq + ToJsonWithContext> ToJsonWithContext for ListLiteral<C> {
    fn to_json(&self, ctx: &AttrFmtContext) -> buck2_error::Result<Value> {
        Ok(to_value(
            self.iter()
                .map(|c| c.to_json(ctx))
                .collect::<Result<Vec<_>, _>>()?,
        )?)
    }
}

impl Kind for kinds::List {
    type Fields = Children;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_list(&mut self, items: &[AttrToken]) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::List>(self, items)
    }
}

impl Semantics for kinds::List {
    const CONCAT: Option<&'static str> = Some("list");

    fn traverse<'a>(
        items: View<'a, Self>,
        Traverse { v, t, pkg, tr }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        let item_t = t.list_item().ok_or_else(|| mismatch(v, t))?;
        for item in items.iter() {
            item.traverse(item_t, pkg, tr)?;
        }
        Ok(())
    }

    fn configure(
        items: View<'_, Self>,
        Configure { v, t, ctx, name }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        let item_t = t.list_item().ok_or_else(|| mismatch(v, t))?;
        ResultMaybeCompatible::Compatible(ConfiguredAttr::List(ListLiteral(
            items
                .iter()
                .map(|item| item.configure(item_t, ctx, name))
                .collect::<ResultMaybeCompatible<Vec<_>>>()?
                .into(),
        )))
    }

    fn traverse_configured(
        items: View<'_, Self>,
        TraverseConfigured {
            v,
            t,
            pkg,
            ctx,
            tr,
            name,
        }: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        let item_t = t.list_item().ok_or_else(|| mismatch(v, t))?;
        for item in items.iter() {
            item.traverse_configured(item_t, pkg, ctx, tr, name)?;
        }
        ResultMaybeCompatible::Compatible(())
    }

    fn to_json(
        items: View<'_, Self>,
        ToJson { ctx }: ToJson<'_>,
    ) -> buck2_error::Result<serde_json::Value> {
        json_seq(items, ctx)
    }

    fn fmt(items: View<'_, Self>, Fmt { ctx, f }: Fmt<'_, '_>) -> std::fmt::Result {
        fmt_seq(items, "[", "]", ctx, f)
    }

    fn any_matches(
        items: View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        any_matches_seq(items, filter)
    }

    fn serialize<S: Serializer>(
        items: View<'_, Self>,
        SerializeKind { ctx, ser }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        ser_seq(items, ctx, ser)
    }
}
