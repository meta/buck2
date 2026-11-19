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
use std::fmt::Display;
use std::hash::Hasher;

use allocative::Allocative;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::execution_types::execution_group::NamedExecutionGroup;
use buck2_core::plugins::PluginKindSet;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::provider::label::ProvidersLabelMaybeConfigured;
use buck2_util::size_assert;
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
use crate::attrs::attr_type::attr_like::AttrLike;
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
use crate::attrs::configured_traversal::ConfiguredAttrTraversal;
use crate::attrs::strong_hash_without_config::StrongHashWithoutConfig;
use crate::attrs::traversal::CoercedAttrTraversal;
use crate::provider_id_set::ProviderIdSet;

/// How configuration is changed when configuring a dep.
#[derive(
    Debug, Eq, PartialEq, Hash, Clone, Dupe, Allocative, Pagable, StrongHash
)]
pub enum DepAttrTransition {
    /// No transition.
    ///
    /// May participate in plugin propagation
    Identity(PluginKindSet),
    /// Transition to execution platform.
    Exec,
    /// Transition to toolchain.
    Toolchain,
    /// Transition to the execution platform of a named execution group.
    ExecInGroup(NamedExecutionGroup),
    /// Transition to toolchain, with the execution platform of a named execution group.
    ToolchainInGroup(NamedExecutionGroup),
}

/// A dep attribute accepts a target label and will resolve to the provider collection from that label's analysis.
#[derive(
    Debug, Eq, PartialEq, Hash, Clone, Dupe, Allocative, Pagable, StrongHash
)]
pub struct DepAttrType {
    /// The set of providers that are required to be available, during attr resolution we'll verify that these
    /// are present on each attribute value.
    pub required_providers: ProviderIdSet,
    pub transition: DepAttrTransition,
}

size_assert::words_of_type!(DepAttrType, 3);

#[derive(Clone, Debug, Eq, PartialEq, Hash, Allocative, Pagable, StrongHash)]
pub struct DepAttr<T: ProvidersLabelMaybeConfigured + AttrLike> {
    // NOTE: This stores DepAttrType per dep, but ConfiguredAttr (where DepAttr
    // is used) is transient—created on demand and destroyed immediately after
    // use. Sharing via Arc or removing it won't reduce retained memory.
    pub attr_type: DepAttrType,
    pub label: T,
}

impl<T: ProvidersLabelMaybeConfigured + AttrLike> Display for DepAttr<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.label, f)
    }
}

impl DepAttr<ConfiguredProvidersLabel> {
    pub(crate) fn traverse(
        &self,
        traversal: &mut dyn ConfiguredAttrTraversal,
    ) -> buck2_error::Result<()> {
        match &self.attr_type.transition {
            DepAttrTransition::Identity(plugins) if plugins.is_empty() => {
                traversal.dep(&self.label)
            }
            DepAttrTransition::Identity(plugins) => {
                traversal.dep_with_plugins(&self.label, plugins)
            }
            DepAttrTransition::Exec => traversal.exec_dep(&self.label),
            DepAttrTransition::Toolchain => traversal.toolchain_dep(&self.label),
            DepAttrTransition::ExecInGroup(group) => {
                traversal.exec_dep_in_group(&self.label, group)
            }
            DepAttrTransition::ToolchainInGroup(group) => {
                traversal.toolchain_dep_in_group(&self.label, group)
            }
        }
    }
}

impl StrongHashWithoutConfig for DepAttr<ConfiguredProvidersLabel> {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H) {
        let Self { attr_type, label } = self;
        attr_type.strong_hash(state);
        label.strong_hash_without_config(state);
    }
}

impl DepAttr<ProvidersLabel> {
    pub fn traverse<'a>(
        label: &'a ProvidersLabel,
        attr_type: &DepAttrType,
        traversal: &mut dyn CoercedAttrTraversal<'a>,
    ) -> buck2_error::Result<()> {
        match &attr_type.transition {
            DepAttrTransition::Identity(..) => traversal.dep(label),
            DepAttrTransition::Exec | DepAttrTransition::ExecInGroup(_) => {
                traversal.exec_dep(label)
            }
            DepAttrTransition::Toolchain | DepAttrTransition::ToolchainInGroup(_) => {
                traversal.toolchain_dep(label)
            }
        }
    }
}

impl DepAttrType {
    pub fn new(required_providers: ProviderIdSet, transition: DepAttrTransition) -> Self {
        Self {
            required_providers,
            transition,
        }
    }

    pub(crate) fn configure(
        &self,
        label: &ProvidersLabel,
        ctx: &dyn AttrConfigurationContext,
    ) -> buck2_error::Result<ConfiguredAttr> {
        let configured_label = match &self.transition {
            DepAttrTransition::Identity(..) => ctx.configure_target(label),
            DepAttrTransition::Exec => ctx.configure_exec_target(label)?,
            DepAttrTransition::Toolchain => ctx.configure_toolchain_target(label),
            DepAttrTransition::ExecInGroup(group) => {
                ctx.configure_exec_target_in_group(label, group)?
            }
            DepAttrTransition::ToolchainInGroup(group) => {
                ctx.configure_toolchain_target_in_group(label, group)?
            }
        };
        Ok(ConfiguredAttr::Dep(Box::new(DepAttr {
            attr_type: self.dupe(),
            label: configured_label,
        })))
    }
}

impl Kind for kinds::Dep {
    type Fields = Label;
}

impl CoercedAttrArenaBuilder {
    pub fn encode_dep(&mut self, l: &ProvidersLabel) -> buck2_error::Result<AttrToken> {
        encode_kind::<kinds::Dep>(self, l)
    }
}

impl Semantics for kinds::Dep {
    fn traverse<'a>(
        l: View<'a, Self>,
        Traverse { v, t, tr, .. }: Traverse<'_, 'a>,
    ) -> buck2_error::Result<()> {
        match &t.non_optional().0.inner {
            AttrTypeInner::Dep(ty) => DepAttr::<ProvidersLabel>::traverse(l, ty, tr),
            _ => Err(mismatch(v, t)),
        }
    }

    fn configure(
        l: View<'_, Self>,
        Configure { v, t, ctx, .. }: Configure<'_, '_>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        match &t.non_optional().0.inner {
            AttrTypeInner::Dep(ty) => ResultMaybeCompatible::Compatible(ty.configure(l, ctx)?),
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
