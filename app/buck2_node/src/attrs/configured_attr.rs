/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::fmt::Display;
use std::hash::Hasher;

use allocative::Allocative;
use buck2_core::package::PackageLabel;
use buck2_core::package::source_path::SourcePathRef;
use buck2_core::plugins::PluginKind;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::buck2_error;
use buck2_error::internal_error;
use buck2_hash::BuckMutInsertionMap;
use buck2_util::arc_str::ArcStr;
use buck2_util::size_assert;
use pagable::Pagable;
use serde::Serialize;
use serde::Serializer;
use starlark_map::small_map;
use strong_hash::StrongHash;

use super::attr_type::arg::ConfiguredStringWithMacros;
use crate::attrs::attr_type::AttrType;
use crate::attrs::attr_type::AttrTypeInner;
use crate::attrs::attr_type::attr_config::source_file_display;
use crate::attrs::attr_type::bool::BoolLiteral;
use crate::attrs::attr_type::configured_dep::ConfiguredExplicitConfiguredDep;
use crate::attrs::attr_type::dep::DepAttr;
use crate::attrs::attr_type::dict::DictLiteral;
use crate::attrs::attr_type::list::ListLiteral;
use crate::attrs::attr_type::one_of::OneOfAttrType;
use crate::attrs::attr_type::query::QueryAttr;
use crate::attrs::attr_type::split_transition_dep::ConfiguredSplitTransitionDep;
use crate::attrs::attr_type::string::StringLiteral;
use crate::attrs::attr_type::transition_dep::ConfiguredTransitionDep;
use crate::attrs::attr_type::tuple::TupleLiteral;
use crate::attrs::coerced_path::CoercedPath;
use crate::attrs::configured_traversal::ConfiguredAttrTraversal;
use crate::attrs::display::AttrDisplayWithContext;
use crate::attrs::display::AttrDisplayWithContextExt;
use crate::attrs::fmt_context::AttrFmtContext;
use crate::attrs::json::ToJsonWithContext;
use crate::attrs::serialize::AttrSerializeWithContext;
use crate::attrs::strong_hash_without_config::StrongHashWithoutConfig;
use crate::metadata::map::MetadataMap;
use crate::modifiers::CfgModifiers;
use crate::visibility::VisibilitySpecification;
use crate::visibility::WithinViewSpecification;

#[derive(Eq, PartialEq, Hash, StrongHash, Clone, Allocative, Debug, Pagable)]
pub enum ConfiguredAttr {
    Bool(BoolLiteral),
    Int(i64),
    // Note we store `String`, not `Arc<str>` here, because we store full attributes
    // in unconfigured target node, but configured target node is basically a pair
    // (reference to unconfigured target node, configuration).
    //
    // Configured attributes are created on demand and destroyed immediately after use.
    //
    // So when working with configured attributes with pay with CPU for string copies,
    // but don't increase total memory usage, because these string copies are short living.
    String(StringLiteral),
    // Like String, but drawn from a set of variants, so doesn't support concat
    EnumVariant(StringLiteral),
    List(ListLiteral<ConfiguredAttr>),
    Tuple(TupleLiteral<ConfiguredAttr>),
    Dict(DictLiteral<ConfiguredAttr>),
    None,
    // NOTE: unlike deps, labels are not traversed, as they are typically used in lieu of deps in
    // cases that would cause cycles.
    OneOf(
        Box<Self>,
        // Index of matched oneof attr type variant.
        u32,
    ),
    Visibility(VisibilitySpecification),
    WithinView(WithinViewSpecification),
    ExplicitConfiguredDep(Box<ConfiguredExplicitConfiguredDep>),
    SplitTransitionDep(Box<ConfiguredSplitTransitionDep>),
    TransitionDep(Box<ConfiguredTransitionDep>),
    ConfigurationDep(ProvidersLabel),
    // Note: Despite being named `PluginDep`, this doesn't really act like a dep but rather like a
    // label
    PluginDep(TargetLabel, PluginKind),
    Dep(Box<DepAttr<ConfiguredProvidersLabel>>),
    SourceLabel(ConfiguredProvidersLabel),
    // NOTE: unlike deps, labels are not traversed, as they are typically used in lieu of deps in
    // cases that would cause cycles.
    Label(ConfiguredProvidersLabel),
    Arg(ConfiguredStringWithMacros),
    Query(Box<QueryAttr<ConfiguredProvidersLabel>>),
    SourceFile(CoercedPath),
    Metadata(MetadataMap),
    TargetModifiers(CfgModifiers),
}

impl StrongHashWithoutConfig for ConfiguredAttr {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Bool(value) => {
                "Bool".strong_hash(state);
                value.strong_hash(state);
            }
            Self::Int(value) => {
                "Int".strong_hash(state);
                value.strong_hash(state);
            }
            Self::String(value) => {
                "String".strong_hash(state);
                value.strong_hash(state);
            }
            Self::EnumVariant(value) => {
                "EnumVariant".strong_hash(state);
                value.strong_hash(state);
            }
            Self::List(values) => {
                "List".strong_hash(state);
                (values.len() as u64).strong_hash(state);
                values
                    .iter()
                    .for_each(|value| value.strong_hash_without_config(state));
            }
            Self::Tuple(values) => {
                "Tuple".strong_hash(state);
                (values.len() as u64).strong_hash(state);
                values
                    .iter()
                    .for_each(|value| value.strong_hash_without_config(state));
            }
            Self::Dict(values) => {
                "Dict".strong_hash(state);
                (values.len() as u64).strong_hash(state);
                values.iter().for_each(|(key, value)| {
                    key.strong_hash_without_config(state);
                    value.strong_hash_without_config(state);
                });
            }
            Self::None => "None".strong_hash(state),
            Self::OneOf(value, index) => {
                "OneOf".strong_hash(state);
                value.strong_hash_without_config(state);
                index.strong_hash(state);
            }
            Self::Visibility(value) => {
                "Visibility".strong_hash(state);
                value.strong_hash(state);
            }
            Self::WithinView(value) => {
                "WithinView".strong_hash(state);
                value.strong_hash(state);
            }
            Self::ExplicitConfiguredDep(dep) => {
                "ExplicitConfiguredDep".strong_hash(state);
                dep.strong_hash_without_config(state);
            }
            Self::SplitTransitionDep(dep) => {
                "SplitTransitionDep".strong_hash(state);
                dep.strong_hash_without_config(state);
            }
            Self::TransitionDep(dep) => {
                "TransitionDep".strong_hash(state);
                dep.strong_hash_without_config(state);
            }
            Self::ConfigurationDep(dep) => {
                "ConfigurationDep".strong_hash(state);
                dep.strong_hash(state);
            }
            Self::PluginDep(dep, kind) => {
                "PluginDep".strong_hash(state);
                dep.strong_hash(state);
                kind.strong_hash(state);
            }
            Self::Dep(dep) => {
                "Dep".strong_hash(state);
                dep.strong_hash_without_config(state);
            }
            Self::SourceLabel(label) => {
                "SourceLabel".strong_hash(state);
                label.strong_hash_without_config(state);
            }
            Self::Label(label) => {
                "Label".strong_hash(state);
                label.strong_hash_without_config(state);
            }
            Self::Arg(arg) => {
                "Arg".strong_hash(state);
                arg.strong_hash_without_config(state);
            }
            Self::Query(query) => {
                "Query".strong_hash(state);
                query.strong_hash_without_config(state);
            }
            Self::SourceFile(source) => {
                "SourceFile".strong_hash(state);
                source.strong_hash(state);
            }
            Self::Metadata(metadata) => {
                "Metadata".strong_hash(state);
                metadata.strong_hash(state);
            }
            Self::TargetModifiers(modifiers) => {
                "TargetModifiers".strong_hash(state);
                modifiers.strong_hash(state);
            }
        }
    }
}

// `ConfiguredAttr` is built per access rather than stored per target, so its
// size matters less than the arena encoding's, yet we should keep it reasonable.
size_assert::words_of_type!(ConfiguredAttr, 4);

impl AttrSerializeWithContext for ConfiguredAttr {
    fn serialize_with_ctx<S>(&self, ctx: &AttrFmtContext, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // TODO this is inefficient. We should impl Serialize and derive value from this instead.
        self.to_json(ctx)
            .map_err(|e| serde::ser::Error::custom(format!("{e}")))?
            .serialize(s)
    }
}

impl AttrDisplayWithContext for ConfiguredAttr {
    fn fmt(&self, ctx: &AttrFmtContext, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfiguredAttr::Bool(v) => {
                write!(f, "{v}")
            }
            ConfiguredAttr::Int(v) => {
                write!(f, "{v}")
            }
            ConfiguredAttr::String(v) | ConfiguredAttr::EnumVariant(v) => {
                AttrDisplayWithContext::fmt(v, ctx, f)
            }
            ConfiguredAttr::List(list) => AttrDisplayWithContext::fmt(list, ctx, f),
            ConfiguredAttr::Tuple(v) => AttrDisplayWithContext::fmt(v, ctx, f),
            ConfiguredAttr::Dict(v) => AttrDisplayWithContext::fmt(v, ctx, f),
            ConfiguredAttr::None => write!(f, "None"),
            ConfiguredAttr::OneOf(box l, _) => AttrDisplayWithContext::fmt(l, ctx, f),
            ConfiguredAttr::Visibility(v) => Display::fmt(v, f),
            ConfiguredAttr::WithinView(v) => Display::fmt(v, f),
            ConfiguredAttr::ExplicitConfiguredDep(e) => Display::fmt(e, f),
            ConfiguredAttr::SplitTransitionDep(e) => Display::fmt(e, f),
            ConfiguredAttr::TransitionDep(e) => Display::fmt(e, f),
            ConfiguredAttr::ConfigurationDep(e) => write!(f, "\"{e}\""),
            ConfiguredAttr::PluginDep(e, _) => write!(f, "\"{e}\""),
            ConfiguredAttr::Dep(e) => write!(f, "\"{e}\""),
            ConfiguredAttr::SourceLabel(e) => write!(f, "\"{e}\""),
            ConfiguredAttr::Label(e) => write!(f, "\"{e}\""),
            ConfiguredAttr::Arg(e) => write!(f, "\"{e}\""),
            ConfiguredAttr::Query(e) => write!(f, "\"{}\"", e.query.query),
            ConfiguredAttr::SourceFile(e) => write!(f, "\"{}\"", source_file_display(ctx, e)),
            ConfiguredAttr::Metadata(m) => write!(f, "{m}"),
            ConfiguredAttr::TargetModifiers(m) => write!(f, "{m}"),
        }
    }
}

impl ConfiguredAttr {
    /// Traverses the configured attribute and calls the traverse for every encountered target label (in deps, sources, or other places).
    pub fn traverse(
        &self,
        pkg: PackageLabel,
        traversal: &mut dyn ConfiguredAttrTraversal,
    ) -> buck2_error::Result<()> {
        match self {
            ConfiguredAttr::Bool(_) => Ok(()),
            ConfiguredAttr::Int(_) => Ok(()),
            ConfiguredAttr::String(_) => Ok(()),
            ConfiguredAttr::EnumVariant(_) => Ok(()),
            ConfiguredAttr::List(list) => {
                for v in list.iter() {
                    v.traverse(pkg, traversal)?;
                }
                Ok(())
            }
            ConfiguredAttr::Tuple(list) => {
                for v in list.iter() {
                    v.traverse(pkg, traversal)?;
                }
                Ok(())
            }
            ConfiguredAttr::Dict(dict) => {
                for (k, v) in dict.iter() {
                    k.traverse(pkg, traversal)?;
                    v.traverse(pkg, traversal)?;
                }
                Ok(())
            }
            ConfiguredAttr::None => Ok(()),
            ConfiguredAttr::OneOf(l, _) => l.traverse(pkg, traversal),
            ConfiguredAttr::Visibility(..) => Ok(()),
            ConfiguredAttr::WithinView(..) => Ok(()),
            ConfiguredAttr::ExplicitConfiguredDep(dep) => dep.as_ref().traverse(traversal),
            ConfiguredAttr::SplitTransitionDep(deps) => {
                for target in deps.deps.values() {
                    traversal.dep(target)?;
                }
                Ok(())
            }
            ConfiguredAttr::TransitionDep(dep) => dep.traverse(traversal),
            ConfiguredAttr::ConfigurationDep(dep) => traversal.configuration_dep(dep),
            ConfiguredAttr::PluginDep(dep, kind) => traversal.plugin_dep(dep, kind),
            ConfiguredAttr::Dep(dep) => dep.traverse(traversal),
            ConfiguredAttr::SourceLabel(dep) => traversal.dep(dep),
            ConfiguredAttr::Label(label) => traversal.label(label),
            ConfiguredAttr::Arg(arg) => arg.string_with_macros.traverse(traversal, pkg),
            ConfiguredAttr::Query(query) => query.traverse(traversal),
            ConfiguredAttr::SourceFile(source) => {
                for x in source.inputs() {
                    traversal.input(SourcePathRef::new(pkg, x))?;
                }
                Ok(())
            }
            ConfiguredAttr::Metadata(..) => Ok(()),
            ConfiguredAttr::TargetModifiers(..) => Ok(()),
        }
    }

    fn concat_not_supported(&self, attr_ty: &'static str) -> buck2_error::Error {
        buck2_error!(
            buck2_error::ErrorTag::Input,
            "addition not supported for these attribute type `{}` and value `{}`",
            attr_ty,
            self.as_display_no_ctx()
        )
    }

    fn unpack_oneof(self) -> buck2_error::Result<(Self, u32)> {
        match self {
            ConfiguredAttr::OneOf(first, first_i) => Ok((*first, first_i)),
            t => Err(internal_error!(
                "expecting oneof variant, got: {}`",
                t.as_display_no_ctx(),
            )),
        }
    }

    fn unpack_oneof_i(self, expected_i: u32, oneof: &OneOfAttrType) -> buck2_error::Result<Self> {
        let (first, i) = self.unpack_oneof()?;
        if i != expected_i {
            let first_t = oneof.get(expected_i)?;
            let next_t = oneof.get(i)?;
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Cannot concatenate values coerced/configured \
                to different oneof variants: `{first_t}` and `{next_t}`"
            ));
        }
        Ok(first)
    }

    fn concat_oneof(
        self,
        items: &mut dyn Iterator<Item = buck2_error::Result<Self>>,
        oneof: &OneOfAttrType,
    ) -> buck2_error::Result<Self> {
        let (first, first_i) = self.unpack_oneof()?;
        let attr = first.concat(
            &oneof.xs[first_i as usize],
            &mut items.map(|v| v?.unpack_oneof_i(first_i, oneof)),
        )?;
        Ok(ConfiguredAttr::OneOf(Box::new(attr), first_i))
    }

    /// Used for concatting the configured result of concatted selects. For most types this isn't allowed (it
    /// should be unreachable as concat-ability is checked during coercion and the type would've returned false from `AttrType::supports_concat`).
    /// This is used when a select() is added to another value, like `select(<...>) + select(<...>)` or `select(<...>) + [...]`.
    pub(crate) fn concat(
        self,
        attr_type: &AttrType,
        items: &mut dyn Iterator<Item = buck2_error::Result<Self>>,
    ) -> buck2_error::Result<Self> {
        match &attr_type.0.inner {
            AttrTypeInner::OneOf(xs) => self.concat_oneof(items, xs),
            AttrTypeInner::Option(opt) => {
                // Because if attr type is `option(oneof([list(string), ...])`,
                // value type is `oneof(list(...))`, without indication it is an option.
                self.concat(&opt.inner, items)
            }
            _ => match self {
                ConfiguredAttr::OneOf(..) => Err(internal_error!(
                    "Inconsistent attr value (`{}`) and attr type (`{}`)",
                    self.as_display_no_ctx(),
                    attr_type
                )),
                ConfiguredAttr::List(list) => {
                    let mut res = list.to_vec();
                    for x in items {
                        match x? {
                            ConfiguredAttr::List(list2) => {
                                res.extend(list2.iter().cloned());
                            }
                            attr => return Err(attr.concat_not_supported("list")),
                        }
                    }
                    Ok(ConfiguredAttr::List(ListLiteral(res.into())))
                }
                ConfiguredAttr::Dict(left) => {
                    let mut res = BuckMutInsertionMap::new();
                    for (k, v) in left.iter().cloned() {
                        res.insert(k, v);
                    }
                    for x in items {
                        match x? {
                            ConfiguredAttr::Dict(right) => {
                                for (k, v) in right.iter().cloned() {
                                    match res.entry(k) {
                                        small_map::Entry::Vacant(e) => {
                                            e.insert(v);
                                        }
                                        small_map::Entry::Occupied(e) => {
                                            return Err(buck2_error!(
                                                buck2_error::ErrorTag::Input,
                                                "got same key in both sides of dictionary concat (key `{}`)",
                                                e.key().as_display_no_ctx()
                                            ));
                                        }
                                    }
                                }
                            }
                            attr => return Err(attr.concat_not_supported("dict")),
                        }
                    }
                    let mut entries: Vec<_> = res.into_iter().collect();
                    // A literal sorted dict is sorted at coercion; a dict assembled from a
                    // concatenation must come out the same way.
                    if let AttrTypeInner::Dict(dict) = &attr_type.0.inner
                        && dict.sorted
                    {
                        entries.sort_by_cached_key(|(k, _)| k.as_display_no_ctx().to_string());
                    }
                    Ok(ConfiguredAttr::Dict(entries.into_iter().collect()))
                }
                ConfiguredAttr::String(res) => {
                    let mut items = items.peekable();
                    if items.peek().is_none() {
                        Ok(ConfiguredAttr::String(res))
                    } else {
                        let mut res = str::to_owned(&res.0);
                        for x in items {
                            match x? {
                                ConfiguredAttr::String(right) => res.push_str(&right.0),
                                attr => return Err(attr.concat_not_supported("string")),
                            }
                        }
                        Ok(ConfiguredAttr::String(StringLiteral(ArcStr::from(res))))
                    }
                }
                ConfiguredAttr::Arg(left) => {
                    let anon_target_compatible = left.anon_target_compatible();
                    let mut configured_by = left.anon_target_tool_resolution().cloned();
                    let res = left.string_with_macros.concat(items.map(|x| match x? {
                        ConfiguredAttr::Arg(x) => {
                            if configured_by.is_none() {
                                configured_by = x.anon_target_tool_resolution().cloned();
                            }
                            Ok(x.string_with_macros)
                        }
                        attr => Err(attr.concat_not_supported("arg")),
                    }))?;
                    Ok(ConfiguredAttr::Arg(ConfiguredStringWithMacros::new(
                        res,
                        anon_target_compatible,
                        configured_by.as_ref(),
                    )))
                }
                val => Err(buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "addition not supported for this attribute type `{}`",
                    val.as_display_no_ctx()
                )),
            },
        }
    }

    pub(crate) fn try_into_configuration_dep(self) -> buck2_error::Result<ProvidersLabel> {
        match self {
            ConfiguredAttr::ConfigurationDep(d) => Ok(d),
            s => Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "expecting configuration dep, got `{0}`",
                s.as_display_no_ctx()
            )),
        }
    }

    pub fn unpack_list(&self) -> Option<&[ConfiguredAttr]> {
        match self {
            ConfiguredAttr::List(list) => Some(list),
            _ => None,
        }
    }

    pub(crate) fn try_into_list(self) -> buck2_error::Result<Vec<ConfiguredAttr>> {
        match self {
            ConfiguredAttr::List(list) => Ok(list.to_vec()),
            a => Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "expecting a list, got `{0}`",
                a.as_display_no_ctx()
            )),
        }
    }
}
