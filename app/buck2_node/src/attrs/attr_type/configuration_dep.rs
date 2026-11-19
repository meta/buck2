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
use buck2_core::provider::label::ProvidersLabel;
use buck2_error::internal_error;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serializer;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::CoercedAttrRef;
use crate::attrs::arena::fields::Label;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::schema::encode_kind;
use crate::attrs::attr_type::AttrTypeInner;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::JsonKey;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::render::json_label;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::semantics::mismatch;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::configuration_context::AttrConfigurationContext;
use crate::attrs::configured_attr::ConfiguredAttr;

/// Describes where a configuration dep appears
#[repr(u8)]
#[derive(
    Debug, Copy, Clone, PartialEq, Eq, Hash, Pagable, Allocative, Dupe, StrongHash
)]
pub enum ConfigurationDepKind {
    SelectKey,
    CompatibilityAttribute,
    ConfiguredDepPlatform,
    Transition,
    DefaultTargetPlatform,
}

/// A configuration dep attribute accepts a target as a value. This is different from
/// a dep in that the values themselves never undergo configuration and appear as bare
/// unconfigured labels even in the configured node. While the values aren't configured,
/// the attribute still is and so selects are still resolved and other values in the
/// attribute could be configured (for example, a `attrs.dict(attrs.dep(), attrs.configuration_dep())`
/// would have the keys configured).
///
/// This is generally used for things that refer to configuration nodes (like platforms or constraints)
/// in attributes like `target_compatible_with` or `exec_compatible_with`.
///
/// They resolve to just the string form of the target and so aren't particularly useful to UDR
/// directly (they are used by the framework).
#[derive(
    Debug, Eq, PartialEq, Hash, Pagable, Allocative, Clone, Copy, Dupe, StrongHash
)]
pub struct ConfigurationDepAttrType(pub ConfigurationDepKind);

impl ConfigurationDepAttrType {
    pub(crate) fn configure(
        _ctx: &dyn AttrConfigurationContext,
        label: &ProvidersLabel,
    ) -> buck2_error::Result<ConfiguredAttr> {
        Ok(ConfiguredAttr::ConfigurationDep(label.dupe()))
    }
}

impl Kind for kinds::ConfigurationDep {
    type Fields = Label;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_configuration_dep(
        &mut self,
        l: &ProvidersLabel,
    ) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::ConfigurationDep>(self, l)
    }
}

impl Semantics for kinds::ConfigurationDep {
    fn traverse<'a>(
        l: View<'a, Self>,
        Traverse { v, t, tr, .. }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        match &t.non_optional().0.inner {
            AttrTypeInner::ConfigurationDep(ty) => tr.configuration_dep(l, ty.0),
            _ => Err(mismatch(v, t)),
        }
    }

    fn configure(
        l: View<'_, Self>,
        Configure { v, t, ctx, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::ConfigurationDep(_) => {
                ResultMaybeCompatible::Compatible(ConfigurationDepAttrType::configure(ctx, l)?)
            }
            _ => mismatch(v, t).into(),
        }
    }

    fn to_json(l: View<'_, Self>, _: ToJson<'_>) -> buck2_error::Result<serde_json::Value> {
        json_label(l)
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

impl<'a> CoercedAttrRef<'a> {
    /// The configuration-dep label, or `None` for an explicit `None` value.
    ///
    /// Errors on any other kind: only for attributes the coercer has already
    /// verified to be a (non-configurable) configuration dep.
    pub fn unpack_configuration_dep(&self) -> buck2_error::Result<Option<&'a ProvidersLabel>> {
        if self.downcast::<kinds::None>().is_some() {
            return Ok(None);
        }
        self.downcast::<kinds::ConfigurationDep>()
            .ok_or_else(|| {
                internal_error!("should be a configuration dep: verified by the attr coercer")
            })
            .map(Some)
    }
}
