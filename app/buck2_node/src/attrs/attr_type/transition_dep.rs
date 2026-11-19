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
use buck2_error::internal_error;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serialize;
use serde::Serializer;
use serde_json::to_value;
use strong_hash::StrongHash;

use crate::attrs::arena::AttrToken;
use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::fields::Label;
use crate::attrs::arena::fields::OptLabel;
use crate::attrs::arena::schema::Kind;
use crate::attrs::arena::schema::View;
use crate::attrs::arena::schema::encode_kind;
use crate::attrs::attr_type::AttrTypeInner;
use crate::attrs::attr_type::configuration_dep::ConfigurationDepKind;
use crate::attrs::coerced::configure::Configure;
use crate::attrs::coerced::kinds;
use crate::attrs::coerced::render::Fmt;
use crate::attrs::coerced::render::MatchesAny;
use crate::attrs::coerced::render::SerializeKind;
use crate::attrs::coerced::render::ToJson;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::coerced::semantics::mismatch;
use crate::attrs::coerced::traverse::Traverse;
use crate::attrs::configuration_context::AttrConfigurationContext;
use crate::attrs::configured_attr::ConfiguredAttr;
use crate::attrs::configured_traversal::ConfiguredAttrTraversal;
use crate::attrs::strong_hash_without_config::StrongHashWithoutConfig;
use crate::attrs::traversal::CoercedAttrTraversal;
use crate::provider_id_set::ProviderIdSet;

#[derive(Debug, Pagable, PartialEq, Eq, Hash, Allocative, StrongHash)]
pub struct TransitionDepAttrType {
    pub required_providers: ProviderIdSet,
    pub transition: Option<Arc<TransitionId>>,
}

impl TransitionDepAttrType {
    pub fn new(required_providers: ProviderIdSet, transition: Option<Arc<TransitionId>>) -> Self {
        TransitionDepAttrType {
            required_providers,
            transition,
        }
    }

    /// `dynamic` is the value's own transition target, present exactly when
    /// the attr type fixes no transition.
    pub(crate) fn configure(
        &self,
        dep: &ProvidersLabel,
        dynamic: Option<&ProvidersLabel>,
        ctx: &dyn AttrConfigurationContext,
    ) -> buck2_error::Result<ConfiguredAttr> {
        let transition = self.transition(dynamic)?;
        Ok(ConfiguredAttr::TransitionDep(Box::new(
            ConfiguredTransitionDep {
                dep: ctx.configure_transition_target(dep, &transition)?,
                required_providers: self.required_providers.dupe(),
            },
        )))
    }

    pub(crate) fn traverse<'a>(
        &self,
        dep: &'a ProvidersLabel,
        dynamic: Option<&ProvidersLabel>,
        traversal: &mut dyn CoercedAttrTraversal<'a>,
    ) -> buck2_error::Result<()> {
        let transition = self.transition(dynamic)?;
        if let TransitionId::Target(label) = &**transition {
            traversal.configuration_dep(label, ConfigurationDepKind::Transition)?;
        }
        traversal.transition_dep(dep, &transition)
    }

    fn transition(
        &self,
        dynamic: Option<&ProvidersLabel>,
    ) -> buck2_error::Result<Cow<'_, Arc<TransitionId>>> {
        match (&self.transition, dynamic) {
            (Some(t), _) => Ok(Cow::Borrowed(t)),
            (None, Some(label)) => Ok(Cow::Owned(Arc::new(TransitionId::Target(label.dupe())))),
            (None, None) => Err(internal_error!(
                "transition dep value without a transition in either the attr type or the value"
            )),
        }
    }
}

#[derive(Hash, StrongHash, PartialEq, Eq, Debug, Clone, Allocative, Pagable)]
pub struct ConfiguredTransitionDep {
    pub dep: ConfiguredProvidersLabel,
    pub required_providers: ProviderIdSet,
}

impl StrongHashWithoutConfig for ConfiguredTransitionDep {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H) {
        let Self {
            dep,
            required_providers,
        } = self;
        dep.strong_hash_without_config(state);
        required_providers.strong_hash(state);
    }
}

impl Display for ConfiguredTransitionDep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.dep, f)
    }
}

impl ConfiguredTransitionDep {
    pub(crate) fn to_json(&self) -> buck2_error::Result<serde_json::Value> {
        Ok(serde_json::to_value(self.dep.to_string())?)
    }

    pub(crate) fn any_matches(
        &self,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        filter(&self.dep.to_string())
    }

    pub(crate) fn traverse(
        &self,
        traversal: &mut dyn ConfiguredAttrTraversal,
    ) -> buck2_error::Result<()> {
        traversal.dep(&self.dep)
    }
}

#[derive(
    derive_more::Display,
    Debug,
    Hash,
    StrongHash,
    PartialEq,
    Eq,
    Clone,
    Allocative,
    Pagable
)]
#[display("{}", dep)]
pub struct CoercedTransitionDep {
    pub dep: ProvidersLabel,
    /// `Some` iff the transition in the attr type is `None`
    ///
    /// Stored as a `TransitionId`, but always a `TransitionId::Target` if set
    pub transition: Option<Arc<TransitionId>>,
}

impl CoercedTransitionDep {
    /// If there's a dynamic transition, return the target
    pub fn get_dynamic_transition(&self) -> Option<&ProvidersLabel> {
        match &**self.transition.as_ref()? {
            TransitionId::Target(t) => Some(t),
            TransitionId::MagicObject { .. } => unreachable!(),
        }
    }
}

impl Kind for kinds::TransitionDep {
    type Fields = (Label, OptLabel);
}

impl CoercedAttrArenaBuilder {
    pub fn encode_transition_dep(
        &mut self,
        dep: &ProvidersLabel,
        transition: Option<&ProvidersLabel>,
    ) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::TransitionDep>(self, (dep, transition))
    }
}

impl Semantics for kinds::TransitionDep {
    fn traverse<'a>(
        (dep, transition): View<'a, Self>,
        Traverse { v, t, tr, .. }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        match &t.non_optional().0.inner {
            AttrTypeInner::TransitionDep(ty) => ty.traverse(dep, transition, tr),
            _ => Err(mismatch(v, t)),
        }
    }

    fn configure(
        (dep, transition): View<'_, Self>,
        Configure { v, t, ctx, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::TransitionDep(ty) => {
                ResultMaybeCompatible::Compatible(ty.configure(dep, transition, ctx)?)
            }
            _ => mismatch(v, t).into(),
        }
    }

    fn to_json(
        (dep, transition): View<'_, Self>,
        _: ToJson<'_>,
    ) -> buck2_error::Result<serde_json::Value> {
        match transition {
            Some(tr) => Ok(to_value([dep.to_string(), tr.to_string()])?),
            None => Ok(to_value(dep.to_string())?),
        }
    }

    fn fmt((dep, _): View<'_, Self>, Fmt { f, .. }: Fmt<'_, '_>) -> std::fmt::Result {
        std::fmt::Display::fmt(dep, f)
    }

    fn any_matches(
        (dep, _): View<'_, Self>,
        MatchesAny { filter }: MatchesAny<'_>,
    ) -> buck2_error::Result<bool> {
        filter(&dep.to_string())
    }

    fn serialize<S: Serializer>(
        (dep, transition): View<'_, Self>,
        SerializeKind { ser, .. }: SerializeKind<'_, S>,
    ) -> Result<S::Ok, S::Error> {
        match transition {
            Some(tr) => [dep.to_string(), tr.to_string()].serialize(ser),
            None => ser.serialize_str(&dep.to_string()),
        }
    }
}
