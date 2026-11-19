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
use std::fmt::Debug;
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

#[derive(Clone, Debug, Eq, PartialEq, Hash, Pagable, Allocative, StrongHash)]
pub struct TupleAttrType {
    pub xs: Vec<AttrType>,
}

impl TupleAttrType {
    pub fn new(xs: Vec<AttrType>) -> Self {
        Self { xs }
    }

    pub(crate) fn fmt_with_arg(&self, f: &mut fmt::Formatter<'_>, arg: &str) -> fmt::Result {
        write!(f, "attrs.tuple(")?;
        for (i, x) in self.xs.iter().enumerate() {
            if i != 0 {
                write!(f, ", ")?;
            }
            write!(f, "{x}")?;
        }
        write!(f, "{arg})")
    }
}

#[derive(
    Debug, Clone, Eq, PartialEq, Hash, Allocative, Default, Pagable, StrongHash
)]
pub struct TupleLiteral<C: Eq>(pub ArcSlice<C>);

impl<C: Eq> Deref for TupleLiteral<C> {
    type Target = ArcSlice<C>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C: Eq + AttrDisplayWithContext> AttrDisplayWithContext for TupleLiteral<C> {
    fn fmt(&self, ctx: &AttrFmtContext, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_container(f, "(", ")", self.0.iter().map(|v| v.as_display(ctx)))
    }
}

impl<C: Eq> FromIterator<C> for TupleLiteral<C> {
    fn from_iter<T: IntoIterator<Item = C>>(iter: T) -> Self {
        TupleLiteral(ArcSlice::from_iter(iter))
    }
}

impl<C: Eq + AnyMatches> AnyMatches for TupleLiteral<C> {
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

impl<C: Eq + ToJsonWithContext> ToJsonWithContext for TupleLiteral<C> {
    fn to_json(&self, ctx: &AttrFmtContext) -> buck2_error::Result<Value> {
        Ok(to_value(
            self.iter()
                .map(|c| c.to_json(ctx))
                .collect::<Result<Vec<_>, _>>()?,
        )?)
    }
}

impl Kind for kinds::Tuple {
    type Fields = Children;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_tuple(&mut self, items: &[AttrToken]) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::Tuple>(self, items)
    }
}

impl Semantics for kinds::Tuple {
    fn traverse<'a>(
        items: View<'a, Self>,
        Traverse { v, t, pkg, tr }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        let item_ts = t.tuple_items(items.len()).ok_or_else(|| mismatch(v, t))?;
        for (item, item_t) in items.iter().zip(item_ts) {
            item.traverse(item_t, pkg, tr)?;
        }
        Ok(())
    }

    fn configure(
        items: View<'_, Self>,
        Configure { v, t, ctx, name }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        let item_ts = t.tuple_items(items.len()).ok_or_else(|| mismatch(v, t))?;
        let configured = items
            .iter()
            .zip(item_ts)
            .map(|(item, item_t)| item.configure(item_t, ctx, name))
            .collect::<ResultMaybeCompatible<_>>()?;
        ResultMaybeCompatible::Compatible(ConfiguredAttr::Tuple(TupleLiteral(configured)))
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
        let item_ts = t.tuple_items(items.len()).ok_or_else(|| mismatch(v, t))?;
        for (item, item_t) in items.iter().zip(item_ts) {
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
        fmt_seq(items, "(", ")", ctx, f)
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
