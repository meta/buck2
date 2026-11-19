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
use std::fmt::Display;
use std::sync::Arc;

use allocative::Allocative;
use buck2_core::configuration::transition::id::TransitionId;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_core::execution_types::execution_group::NamedExecutionGroup;
use buck2_core::plugins::PluginKind;
use buck2_core::plugins::PluginKindSet;
use buck2_util::strong_hasher::Blake3StrongHasher;
use buck2_util::strong_hasher::StrongHash128;
use dupe::Dupe;
use itertools::Either;
use pagable::Pagable;
use strong_hash::StrongHash;

use crate::attrs::attr_type::any::AnyAttrType;
use crate::attrs::attr_type::arg::ArgAttrType;
use crate::attrs::attr_type::bool::BoolAttrType;
use crate::attrs::attr_type::configuration_dep::ConfigurationDepAttrType;
use crate::attrs::attr_type::configuration_dep::ConfigurationDepKind;
use crate::attrs::attr_type::configured_dep::ExplicitConfiguredDepAttrType;
use crate::attrs::attr_type::dep::DepAttrTransition;
use crate::attrs::attr_type::dep::DepAttrType;
use crate::attrs::attr_type::dict::DictAttrType;
use crate::attrs::attr_type::enumeration::EnumAttrType;
use crate::attrs::attr_type::int::IntAttrType;
use crate::attrs::attr_type::label::LabelAttrType;
use crate::attrs::attr_type::list::ListAttrType;
use crate::attrs::attr_type::metadata::MetadataAttrType;
use crate::attrs::attr_type::one_of::OneOfAttrType;
use crate::attrs::attr_type::option::OptionAttrType;
use crate::attrs::attr_type::plugin_dep::PluginDepAttrType;
use crate::attrs::attr_type::query::QueryAttrType;
use crate::attrs::attr_type::source::SourceAttrType;
use crate::attrs::attr_type::split_transition_dep::SplitTransitionDepAttrType;
use crate::attrs::attr_type::string::StringAttrType;
use crate::attrs::attr_type::target_modifiers::TargetModifiersAttrType;
use crate::attrs::attr_type::transition_dep::TransitionDepAttrType;
use crate::attrs::attr_type::tuple::TupleAttrType;
use crate::attrs::attr_type::visibility::VisibilityAttrType;
use crate::attrs::attr_type::within_view::WithinViewAttrType;
use crate::provider_id_set::ProviderIdSet;

pub mod any;
pub mod any_matches;
pub mod arg;
pub mod attr_config;
pub mod attr_like;
pub mod bool;
pub mod configuration_dep;
pub mod configured_dep;
pub mod default_only;
pub mod dep;
pub mod dict;
pub mod enumeration;
pub mod int;
pub mod label;
pub mod list;
pub mod metadata;
pub mod one_of;
pub mod option;
pub mod plugin_dep;
pub mod query;
pub mod source;
pub mod split_transition_dep;
pub mod string;
pub mod target_modifiers;
pub mod transition_dep;
pub mod tuple;
pub mod visibility;
pub mod within_view;

#[derive(Clone, Dupe, Debug, Hash, Pagable, Eq, PartialEq, Allocative)]
pub struct AttrType(pub Arc<AttrTypeInner2>);

#[derive(Debug, Hash, Pagable, Eq, PartialEq, Allocative)]
pub struct AttrTypeInner2 {
    pub inner: AttrTypeInner,
    /// Attribute may have queries.
    ///
    /// These are either:
    /// * `attrs.query(...)`
    /// * `attrs.arg()`
    /// * collection of those e.g. `attrs.list(attrs.query(...))`
    pub may_have_queries: bool,
    /// Identity-grade content hash of the type: equal fingerprints identify
    /// types that coerce every value identically.
    pub fingerprint: StrongHash128,
}

#[derive(Debug, Hash, Pagable, Eq, PartialEq, Allocative, StrongHash)]
pub enum AttrTypeInner {
    Any(AnyAttrType),
    Arg(ArgAttrType),
    ConfigurationDep(ConfigurationDepAttrType),
    ConfiguredDep(ExplicitConfiguredDepAttrType),
    Bool(BoolAttrType),
    Int(IntAttrType),
    Dep(DepAttrType),
    Dict(DictAttrType),
    List(ListAttrType),
    Tuple(TupleAttrType),
    OneOf(OneOfAttrType),
    Option(OptionAttrType),
    PluginDep(PluginDepAttrType),
    Query(QueryAttrType),
    Source(SourceAttrType),
    SplitTransitionDep(SplitTransitionDepAttrType),
    TransitionDep(TransitionDepAttrType),
    String(StringAttrType),
    Enum(EnumAttrType),
    Label(LabelAttrType),
    Visibility(VisibilityAttrType),
    WithinView(WithinViewAttrType),
    Metadata(MetadataAttrType),
    TargetModifiers(TargetModifiersAttrType),
}

/// Merkle composition: a type's stored fingerprint stands in for its whole
/// subtree, so nested constructors never re-walk their children.
impl StrongHash for AttrType {
    fn strong_hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.fingerprint.strong_hash(state);
    }
}

impl AttrType {
    fn of(inner: AttrTypeInner, may_have_queries: bool) -> Self {
        let mut hasher = Blake3StrongHasher::new();
        inner.strong_hash(&mut hasher);
        Self(Arc::new(AttrTypeInner2 {
            inner,
            may_have_queries,
            fingerprint: hasher.finalize128(),
        }))
    }

    /// Identity-grade content hash: equal fingerprints identify types that
    /// coerce every value identically.
    pub fn fingerprint(&self) -> StrongHash128 {
        self.0.fingerprint
    }

    pub fn fmt_with_default(
        &self,
        f: &mut fmt::Formatter<'_>,
        default: Option<&str>,
    ) -> fmt::Result {
        let mut attr = |s| match default {
            None => write!(f, "attrs.{s}()"),
            Some(default) => write!(f, "attrs.{s}(default={default})"),
        };
        let arg = || match default {
            None => String::new(),
            Some(x) => format!(", default={x}"),
        };

        match &self.0.inner {
            AttrTypeInner::Any(_) => attr("any"),
            AttrTypeInner::Arg(_) => attr("arg"),
            AttrTypeInner::ConfigurationDep(_) => attr("configuration_dep"),
            AttrTypeInner::ConfiguredDep(_) => attr("configured_dep"),
            AttrTypeInner::PluginDep(_) => attr("plugin_dep"),
            AttrTypeInner::Bool(_) => attr("bool"),
            AttrTypeInner::Int(_) => attr("int"),
            AttrTypeInner::Dep(_) => attr("dep"),
            AttrTypeInner::Query(_) => attr("query"),
            AttrTypeInner::Dict(x) => x.fmt_with_arg(f, &arg()),
            AttrTypeInner::List(x) => x.fmt_with_arg(f, &arg()),
            AttrTypeInner::Tuple(x) => x.fmt_with_arg(f, &arg()),
            AttrTypeInner::OneOf(x) => x.fmt_with_arg(f, &arg()),
            AttrTypeInner::Option(x) => x.fmt_with_arg(f, &arg()),
            AttrTypeInner::Enum(x) => x.fmt_with_arg(f, &arg()),
            AttrTypeInner::Source(_) => attr("source"),
            AttrTypeInner::SplitTransitionDep(_) => attr("split_transition_dep"),
            AttrTypeInner::TransitionDep(_) => attr("transition_dep"),
            AttrTypeInner::String(_) => attr("string"),
            AttrTypeInner::Label(_) => attr("label"),
            AttrTypeInner::Visibility(_) => attr("visibility"),
            AttrTypeInner::WithinView(_) => attr("within_view"),
            AttrTypeInner::Metadata(_) => attr("metadata"),
            AttrTypeInner::TargetModifiers(_) => attr("modifiers"),
        }
    }

    pub fn any() -> Self {
        Self::of(AttrTypeInner::Any(AnyAttrType), false)
    }

    /// This type with its `Option` layers removed. Values do not record
    /// optionality, so a value's kind is read against this.
    pub(crate) fn non_optional(&self) -> &AttrType {
        let mut t = self;
        while let AttrTypeInner::Option(opt) = &t.0.inner {
            t = &opt.inner;
        }
        t
    }

    /// The item type of a list value of this type, or `None` if lists do
    /// not fit it. Values do not record that they were coerced as `Any`
    /// either: an `Any` holds literal containers whose items are `Any`.
    pub(crate) fn list_item(&self) -> Option<&AttrType> {
        let t = self.non_optional();
        match &t.0.inner {
            AttrTypeInner::Any(_) => Some(t),
            AttrTypeInner::List(ty) => Some(&ty.inner),
            _ => None,
        }
    }

    /// The key and value types of a dict value of this type, or `None` if
    /// dicts do not fit it; `Any` as for [`Self::list_item`].
    pub(crate) fn dict_items(&self) -> Option<(&AttrType, &AttrType)> {
        let t = self.non_optional();
        match &t.0.inner {
            AttrTypeInner::Any(_) => Some((t, t)),
            AttrTypeInner::Dict(ty) => Some((&ty.key, &ty.value)),
            _ => None,
        }
    }

    /// The item types of a tuple value of this type and length, or `None`
    /// if such tuples do not fit it; `Any` as for [`Self::list_item`].
    pub(crate) fn tuple_items(&self, len: usize) -> Option<impl Iterator<Item = &AttrType>> {
        let t = self.non_optional();
        match &t.0.inner {
            AttrTypeInner::Any(_) => Some(Either::Left(std::iter::repeat_n(t, len))),
            AttrTypeInner::Tuple(ty) if ty.xs.len() == len => Some(Either::Right(ty.xs.iter())),
            _ => None,
        }
    }

    /// An arg attribute. Args are similar to strings, but have built in support
    /// for string parameter macros and make variables. Command line
    /// builders used in rule implementations use args (and so an arg attribute
    /// can be directly added to them).
    ///
    /// Takes in an anon_target_compatible flag, which indicates whether the arg
    /// can be passed into anon targets. There is a slight memory hit when using
    /// this flag.
    pub fn arg(anon_target_compatible: bool) -> Self {
        Self::arg_in_group(anon_target_compatible, None, true)
    }

    /// An arg attribute whose `$(exe)`, `$(location_exec)` and `$(location_toolchain)` macros
    /// refer to dependencies in the given named execution group, or in the default group
    /// without one. `ungrouped` is whether the `attrs.arg()` names no execution group.
    pub fn arg_in_group(
        anon_target_compatible: bool,
        execution_group: Option<NamedExecutionGroup>,
        ungrouped: bool,
    ) -> Self {
        Self::of(
            AttrTypeInner::Arg(ArgAttrType {
                anon_target_compatible,
                execution_group,
                ungrouped,
            }),
            true,
        )
    }

    pub fn enumeration(variants: Vec<String>) -> buck2_error::Result<Self> {
        Ok(Self::of(
            AttrTypeInner::Enum(EnumAttrType::new(variants)?),
            false,
        ))
    }

    pub fn bool() -> Self {
        Self::of(AttrTypeInner::Bool(BoolAttrType), false)
    }

    pub fn int() -> Self {
        Self::of(AttrTypeInner::Int(IntAttrType), false)
    }

    pub fn configuration_dep(t: ConfigurationDepKind) -> Self {
        Self::of(
            AttrTypeInner::ConfigurationDep(ConfigurationDepAttrType(t)),
            false,
        )
    }

    /// A TargetLabel attribute optionally with a specific provider/providers
    /// that are expected.
    ///
    /// If `required_providers` is non-empty, the dependency must return those providers
    /// from its implementation function. Otherwise an error will result at resolution time.
    pub fn dep(required_providers: ProviderIdSet, plugin_kinds: PluginKindSet) -> Self {
        Self::of(
            AttrTypeInner::Dep(DepAttrType::new(
                required_providers,
                DepAttrTransition::Identity(plugin_kinds),
            )),
            false,
        )
    }

    /// An execution dependency attribute optionally with a specific provider/providers
    /// that are expected.
    ///
    /// If `required_providers` is non-empty, the dependency must return those providers
    /// from its implementation function. Otherwise an error will result at resolution time.
    pub fn exec_dep(required_providers: ProviderIdSet) -> Self {
        Self::of(
            AttrTypeInner::Dep(DepAttrType::new(
                required_providers,
                DepAttrTransition::Exec,
            )),
            false,
        )
    }

    /// An execution dependency attribute in the given execution group.
    pub fn exec_dep_in_group(required_providers: ProviderIdSet, group: ExecutionGroup) -> Self {
        let transition = match group {
            ExecutionGroup::Default => DepAttrTransition::Exec,
            ExecutionGroup::Named(group) => DepAttrTransition::ExecInGroup(group),
        };
        Self::of(
            AttrTypeInner::Dep(DepAttrType::new(required_providers, transition)),
            false,
        )
    }

    /// A toolchain dependency attribute whose toolchain uses the execution platform of the
    /// given execution group.
    pub fn toolchain_dep_in_group(
        required_providers: ProviderIdSet,
        group: ExecutionGroup,
    ) -> Self {
        let transition = match group {
            ExecutionGroup::Default => DepAttrTransition::Toolchain,
            ExecutionGroup::Named(group) => DepAttrTransition::ToolchainInGroup(group),
        };
        Self::of(
            AttrTypeInner::Dep(DepAttrType::new(required_providers, transition)),
            false,
        )
    }

    /// Calls `f` for every named execution group a dep within this attribute belongs to.
    pub fn for_each_named_execution_group(&self, f: &mut dyn FnMut(&NamedExecutionGroup)) {
        match &self.0.inner {
            AttrTypeInner::Dep(DepAttrType {
                transition:
                    DepAttrTransition::ExecInGroup(group) | DepAttrTransition::ToolchainInGroup(group),
                ..
            }) => f(group),
            AttrTypeInner::Arg(ArgAttrType {
                execution_group: Some(group),
                ..
            }) => f(group),
            AttrTypeInner::List(x) => x.inner.for_each_named_execution_group(f),
            AttrTypeInner::Option(x) => x.inner.for_each_named_execution_group(f),
            AttrTypeInner::Dict(x) => {
                x.key.for_each_named_execution_group(f);
                x.value.for_each_named_execution_group(f);
            }
            AttrTypeInner::Tuple(x) => {
                x.xs.iter()
                    .for_each(|x| x.for_each_named_execution_group(f))
            }
            AttrTypeInner::OneOf(x) => {
                x.xs.iter()
                    .for_each(|x| x.for_each_named_execution_group(f))
            }
            _ => {}
        }
    }

    /// Whether a dep within this attribute belongs to the default execution group: an exec dep
    /// or toolchain dep without a named execution group.
    pub fn uses_default_execution_group(&self) -> bool {
        match &self.0.inner {
            AttrTypeInner::Dep(DepAttrType {
                transition: DepAttrTransition::Exec | DepAttrTransition::Toolchain,
                ..
            }) => true,
            AttrTypeInner::List(x) => x.inner.uses_default_execution_group(),
            AttrTypeInner::Option(x) => x.inner.uses_default_execution_group(),
            AttrTypeInner::Dict(x) => {
                x.key.uses_default_execution_group() || x.value.uses_default_execution_group()
            }
            AttrTypeInner::Tuple(x) => x.xs.iter().any(|x| x.uses_default_execution_group()),
            AttrTypeInner::OneOf(x) => x.xs.iter().any(|x| x.uses_default_execution_group()),
            AttrTypeInner::Dep(_)
            | AttrTypeInner::Any(_)
            | AttrTypeInner::Arg(_)
            | AttrTypeInner::ConfigurationDep(_)
            | AttrTypeInner::ConfiguredDep(_)
            | AttrTypeInner::Bool(_)
            | AttrTypeInner::Int(_)
            | AttrTypeInner::PluginDep(_)
            | AttrTypeInner::Query(_)
            | AttrTypeInner::Source(_)
            | AttrTypeInner::SplitTransitionDep(_)
            | AttrTypeInner::TransitionDep(_)
            | AttrTypeInner::String(_)
            | AttrTypeInner::Enum(_)
            | AttrTypeInner::Label(_)
            | AttrTypeInner::Visibility(_)
            | AttrTypeInner::WithinView(_)
            | AttrTypeInner::Metadata(_)
            | AttrTypeInner::TargetModifiers(_) => false,
        }
    }

    /// A toolchain dependency attribute optionally with a specific provider/providers
    /// that are expected.
    ///
    /// If `required_providers` is non-empty, the dependency must return those providers
    /// from its implementation function. Otherwise an error will result at resolution time.
    pub fn toolchain_dep(required_providers: ProviderIdSet) -> Self {
        Self::of(
            AttrTypeInner::Dep(DepAttrType::new(
                required_providers,
                DepAttrTransition::Toolchain,
            )),
            false,
        )
    }

    /// An a dependency attribute which changes the configuration optionally with a specific
    /// provider/providers that are expected.
    ///
    /// If `required_providers` is non-empty, the dependency must return those providers
    /// from its implementation function. Otherwise an error will result at resolution time.
    pub fn transition_dep(
        required_providers: ProviderIdSet,
        cfg: Option<Arc<TransitionId>>,
    ) -> Self {
        Self::of(
            AttrTypeInner::TransitionDep(TransitionDepAttrType::new(required_providers, cfg)),
            false,
        )
    }

    pub fn configured_dep(required_providers: ProviderIdSet) -> Self {
        Self::of(
            AttrTypeInner::ConfiguredDep(ExplicitConfiguredDepAttrType { required_providers }),
            false,
        )
    }

    pub fn split_transition_dep(required_providers: ProviderIdSet, cfg: Arc<TransitionId>) -> Self {
        Self::of(
            AttrTypeInner::SplitTransitionDep(SplitTransitionDepAttrType::new(
                required_providers,
                cfg,
            )),
            false,
        )
    }

    pub fn plugin_dep(kind: PluginKind) -> Self {
        Self::of(
            AttrTypeInner::PluginDep(PluginDepAttrType::new(kind)),
            false,
        )
    }

    /// A dict attribute containing keys and values of the specified types.
    pub fn dict(key: AttrType, value: AttrType, sorted: bool) -> Self {
        let may_have_queries = key.0.may_have_queries || value.0.may_have_queries;
        Self::of(
            AttrTypeInner::Dict(DictAttrType::new(key, value, sorted)),
            may_have_queries,
        )
    }

    /// A list attribute containing items of some inner type.
    pub fn list(inner: AttrType) -> Self {
        let may_have_queries = inner.0.may_have_queries;
        Self::of(
            AttrTypeInner::List(ListAttrType::new(inner)),
            may_have_queries,
        )
    }

    pub fn tuple(xs: Vec<AttrType>) -> Self {
        let may_have_queries = xs.iter().any(|x| x.0.may_have_queries);
        Self::of(
            AttrTypeInner::Tuple(TupleAttrType::new(xs)),
            may_have_queries,
        )
    }

    pub fn one_of(xs: Vec<AttrType>) -> Self {
        let may_have_queries = xs.iter().any(|x| x.0.may_have_queries);
        Self::of(
            AttrTypeInner::OneOf(OneOfAttrType::new(xs)),
            may_have_queries,
        )
    }

    pub fn option(value: AttrType) -> Self {
        let may_have_queries = value.0.may_have_queries;
        Self::of(
            AttrTypeInner::Option(OptionAttrType::new(value)),
            may_have_queries,
        )
    }

    pub fn query() -> Self {
        Self::of(
            AttrTypeInner::Query(QueryAttrType::new(DepAttrType::new(
                ProviderIdSet::EMPTY,
                DepAttrTransition::Identity(PluginKindSet::EMPTY),
            ))),
            true,
        )
    }

    // A file attribute. This will accept paths or targets like
    /// `//some:target[inner]`. When contained within a list, one item may
    /// expand to multiple (e.g. an output group or a lazy glob).
    pub fn source(allow_directory: bool) -> Self {
        Self::of(
            AttrTypeInner::Source(SourceAttrType { allow_directory }),
            false,
        )
    }

    /// A string attribute. For flags passed to a command, an arg() attr is
    /// preferred to support macro and make variable substitution.
    pub fn string() -> Self {
        Self::of(AttrTypeInner::String(StringAttrType), false)
    }

    pub fn label() -> Self {
        Self::of(AttrTypeInner::Label(LabelAttrType), false)
    }

    pub fn visibility() -> Self {
        Self::of(AttrTypeInner::Visibility(VisibilityAttrType), false)
    }

    pub fn within_view() -> Self {
        Self::of(AttrTypeInner::WithinView(WithinViewAttrType), false)
    }

    pub fn metadata() -> Self {
        Self::of(AttrTypeInner::Metadata(MetadataAttrType), false)
    }

    pub fn target_modifiers() -> Self {
        Self::of(
            AttrTypeInner::TargetModifiers(TargetModifiersAttrType),
            false,
        )
    }

    /// Used when we first detect that concatenation is going to happen for an attr
    /// while loading a build file. Returning false here will make us provide an error
    /// during the loading phase at the point that the concatenation happens.
    ///
    /// In some cases, we can't detect that the concatenation isn't allowed at this
    /// point and can only provide an error when performing the actual concatenation.
    pub fn supports_concat(&self) -> bool {
        match &self.0.inner {
            AttrTypeInner::Bool(_)
            | AttrTypeInner::Query(_)
            | AttrTypeInner::Source(_)
            | AttrTypeInner::ConfigurationDep(_)
            | AttrTypeInner::ConfiguredDep(_)
            | AttrTypeInner::PluginDep(_)
            | AttrTypeInner::Int(_)
            | AttrTypeInner::Dep(_)
            | AttrTypeInner::Tuple(_)
            | AttrTypeInner::SplitTransitionDep(_)
            | AttrTypeInner::TransitionDep(_)
            | AttrTypeInner::Label(_)
            | AttrTypeInner::Enum(_)
            | AttrTypeInner::Visibility(_)
            | AttrTypeInner::WithinView(_)
            | AttrTypeInner::TargetModifiers(_)
            | AttrTypeInner::Metadata(_) => false,
            AttrTypeInner::Any(_)
            | AttrTypeInner::Arg(_)
            | AttrTypeInner::Dict(_)
            | AttrTypeInner::List(_)
            | AttrTypeInner::String(_) => true,
            AttrTypeInner::Option(inner) => inner.inner.supports_concat(),
            // Reject if none of the inner types support concat. Mismatched types are rejected later.
            AttrTypeInner::OneOf(inner) => inner.any_supports_concat(),
        }
    }
}

/// Invariant: All these displays look like function calls, so follow the pattern `attrs.foo(...)`.
impl Display for AttrType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_with_default(f, None)
    }
}
