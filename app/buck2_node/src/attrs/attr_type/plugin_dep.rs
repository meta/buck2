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

use allocative::Allocative;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::plugins::PluginKind;
use buck2_core::target::label::label::TargetLabel;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serializer;
use serde_json::to_value;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::fields::Target;
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
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::semantics::mismatch;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::configured_attr::ConfiguredAttr;

#[derive(
    Debug, Eq, PartialEq, Hash, Pagable, Allocative, Clone, Dupe, StrongHash
)]
pub struct PluginDepAttrType {
    kind: PluginKind,
}

impl PluginDepAttrType {
    pub fn new(kind: PluginKind) -> Self {
        Self { kind }
    }

    pub fn kind(&self) -> &PluginKind {
        &self.kind
    }
}

impl Kind for kinds::PluginDep {
    type Fields = Target;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_plugin_dep(&mut self, l: &TargetLabel) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::PluginDep>(self, l)
    }
}

impl Semantics for kinds::PluginDep {
    fn traverse<'a>(
        l: View<'a, Self>,
        Traverse { v, t, tr, .. }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        match &t.non_optional().0.inner {
            AttrTypeInner::PluginDep(ty) => tr.plugin_dep(l, ty.kind()),
            _ => Err(mismatch(v, t)),
        }
    }

    fn configure(
        l: View<'_, Self>,
        Configure { v, t, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::PluginDep(ty) => ResultMaybeCompatible::Compatible(
                ConfiguredAttr::PluginDep(l.dupe(), ty.kind().dupe()),
            ),
            _ => mismatch(v, t).into(),
        }
    }

    fn traverse_configured(
        l: View<'_, Self>,
        TraverseConfigured { v, t, tr, .. }: TraverseConfigured<'_, '_>,
    ) -> ResultMaybeCompatible<()> {
        match &t.non_optional().0.inner {
            AttrTypeInner::PluginDep(ty) => {
                if let Err(e) = tr.plugin_dep(l, ty.kind()) {
                    return e.into();
                }
            }
            _ => return mismatch(v, t).into(),
        }
        ResultMaybeCompatible::Compatible(())
    }

    fn to_json(l: View<'_, Self>, _: ToJson<'_>) -> buck2_error::Result<serde_json::Value> {
        Ok(to_value(l.to_string())?)
    }

    fn fmt(l: View<'_, Self>, Fmt { f, .. }: Fmt<'_, '_>) -> std::fmt::Result {
        write!(f, "\"{l}\"")
    }

    fn any_matches(
        l: View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        filter(&l.to_string())
    }

    fn json_key<'a>(l: View<'a, Self>, _: JsonKey<'_>) -> buck2_error::Result<Cow<'a, str>> {
        Ok(Cow::Owned(l.to_string()))
    }

    fn serialize<S: Serializer>(
        l: View<'_, Self>,
        SerializeKind { ser, .. }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&l.to_string())
    }
}
