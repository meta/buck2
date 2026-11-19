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
use std::fmt::Display;
use std::hash::Hasher;
use std::sync::Arc;

use allocative::Allocative;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::configuration::transition::id::TransitionId;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_hash::BuckFixedSortedMap;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serializer;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
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
use crate::attrs::strong_hash_without_config::StrongHashWithoutConfig;
use crate::provider_id_set::ProviderIdSet;

#[derive(Debug, Pagable, PartialEq, Eq, Hash, Allocative, StrongHash)]
pub struct SplitTransitionDepAttrType {
    pub required_providers: ProviderIdSet,
    pub transition: Arc<TransitionId>,
}

impl SplitTransitionDepAttrType {
    pub fn new(required_providers: ProviderIdSet, transition: Arc<TransitionId>) -> Self {
        SplitTransitionDepAttrType {
            required_providers,
            transition,
        }
    }

    pub(crate) fn configure(
        &self,
        dep: &ProvidersLabel,
        ctx: &dyn AttrConfigurationContext,
    ) -> buck2_error::Result<ConfiguredAttr> {
        let configured_providers = ctx.configure_split_transition_target(dep, &self.transition)?;
        Ok(ConfiguredAttr::SplitTransitionDep(Box::new(
            ConfiguredSplitTransitionDep {
                deps: configured_providers,
                required_providers: self.required_providers.dupe(),
            },
        )))
    }
}

#[derive(Hash, StrongHash, PartialEq, Eq, Debug, Clone, Allocative, Pagable)]
pub struct ConfiguredSplitTransitionDep {
    pub deps: BuckFixedSortedMap<String, ConfiguredProvidersLabel>,
    pub required_providers: ProviderIdSet,
}

impl StrongHashWithoutConfig for ConfiguredSplitTransitionDep {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H) {
        let Self {
            deps,
            required_providers,
        } = self;
        (deps.len() as u64).strong_hash(state);
        deps.iter().for_each(|(key, label)| {
            key.strong_hash(state);
            label.strong_hash_without_config(state);
        });
        required_providers.strong_hash(state);
    }
}

impl Display for ConfiguredSplitTransitionDep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{")?;
        for (i, dep) in self.deps.iter().enumerate() {
            if i != 0 {
                write!(f, ", ")?;
            }
            write!(f, "{:?}: {}", dep.0, dep.1)?;
        }
        write!(f, "}}")?;
        Ok(())
    }
}

impl ConfiguredSplitTransitionDep {
    pub(crate) fn to_json(&self) -> buck2_error::Result<serde_json::Value> {
        let mut map = serde_json::Map::with_capacity(self.deps.len());
        for (label, target) in &self.deps {
            map.insert(label.clone(), serde_json::to_value(target.to_string())?);
        }
        Ok(serde_json::Value::Object(map))
    }

    pub(crate) fn any_matches(
        &self,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        for (label, target) in &self.deps {
            if filter(label)? || filter(&target.to_string())? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl Kind for kinds::SplitTransitionDep {
    type Fields = Label;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_split_transition_dep(
        &mut self,
        l: &ProvidersLabel,
    ) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::SplitTransitionDep>(self, l)
    }
}

impl Semantics for kinds::SplitTransitionDep {
    fn traverse<'a>(
        l: View<'a, Self>,
        Traverse { v, t, tr, .. }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        match &t.non_optional().0.inner {
            AttrTypeInner::SplitTransitionDep(ty) => tr.split_transition_dep(l, &ty.transition),
            _ => Err(mismatch(v, t)),
        }
    }

    fn configure(
        l: View<'_, Self>,
        Configure { v, t, ctx, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::SplitTransitionDep(ty) => {
                ResultMaybeCompatible::Compatible(ty.configure(l, ctx)?)
            }
            _ => mismatch(v, t).into(),
        }
    }

    fn to_json(l: View<'_, Self>, _: ToJson<'_>) -> buck2_error::Result<serde_json::Value> {
        json_label(l)
    }

    fn fmt(l: View<'_, Self>, Fmt { f, .. }: Fmt<'_, '_>) -> std::fmt::Result {
        std::fmt::Display::fmt(l, f)
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
