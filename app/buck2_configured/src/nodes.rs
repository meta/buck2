/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Calculations relating to 'TargetNode's that runs on Dice

use std::hash::Hash;
use std::iter;
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::transition::TRANSITION_ATTRS_PROVIDER;
use buck2_build_api::transition::TRANSITION_CALCULATION;
use buck2_build_signals::node_key::BuildSignalsNodeKey;
use buck2_build_signals::node_key::BuildSignalsNodeKeyImpl;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_common::legacy_configs::view::LegacyBuckConfigView;
use buck2_configuration_providers::dep_only_incompatible_info::DepOnlyIncompatibleCustomSoftErrors;
use buck2_configuration_providers::dep_only_incompatible_info::DepOnlyIncompatibleInfo;
use buck2_core::configuration::compatibility::IncompatiblePlatformReason;
use buck2_core::configuration::compatibility::IncompatiblePlatformReasonCause;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::configuration::compatibility::ResultMaybeCompatibleValueSerialize;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::configuration::pair::ConfigurationNoExec;
use buck2_core::configuration::pair::ConfigurationWithExec;
use buck2_core::configuration::transition::applied::TransitionApplied;
use buck2_core::configuration::transition::id::TransitionId;
use buck2_core::deprecation;
use buck2_core::execution_types::execution::ExecutionGroupResolutionPartial;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::execution_types::execution::ExecutionPlatformResolutionPartial;
use buck2_core::execution_types::execution_group::NamedExecutionGroup;
use buck2_core::package::PackageLabel;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::plugins::PluginKind;
use buck2_core::plugins::PluginKindSet;
use buck2_core::plugins::PluginListElemKind;
use buck2_core::plugins::PluginLists;
use buck2_core::plugins::PluginListsBuilder;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::configured_or_unconfigured::ConfiguredOrUnconfiguredTargetLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_core::target::target_configured_target_label::TargetConfiguredTargetLabel;
use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_node::attrs::configuration_context::AttrConfigurationContext;
use buck2_node::attrs::configuration_context::AttrConfigurationContextImpl;
use buck2_node::attrs::configuration_context::PlatformConfigurationError;
use buck2_node::attrs::configured_attr::ConfiguredAttr;
use buck2_node::attrs::configured_traversal::ConfiguredAttrTraversal;
use buck2_node::attrs::display::AttrDisplayWithContextExt;
use buck2_node::attrs::inspect_options::AttrInspectOptions;
use buck2_node::attrs::spec::AttributeId;
use buck2_node::attrs::spec::internal::INCOMING_TRANSITION_ATTRIBUTE;
use buck2_node::attrs::spec::internal::LEGACY_TARGET_COMPATIBLE_WITH_ATTRIBUTE;
use buck2_node::attrs::spec::internal::TARGET_COMPATIBLE_WITH_ATTRIBUTE;
use buck2_node::configuration::calculation::CellNameForConfigurationResolution;
use buck2_node::configuration::resolved::MatchedConfigurationSettingKeys;
use buck2_node::configuration::resolved::MatchedConfigurationSettingKeysWithCfg;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured_frontend::CONFIGURED_TARGET_NODE_CALCULATION;
use buck2_node::nodes::configured_frontend::ConfiguredTargetNodeCalculation;
use buck2_node::nodes::configured_frontend::ConfiguredTargetNodeCalculationImpl;
use buck2_node::nodes::frontend::TargetGraphCalculation;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_node::nodes::unconfigured::TargetNodeRef;
use buck2_node::package_visibility::HasPackageVisibilityDefaultIntersection;
use buck2_node::rule::RuleExecutionGroup;
use buck2_node::rule::RuleIncomingTransition;
use buck2_settings::PackageVisibilityDefaultIntersection;
use buck2_util::arc_str::ArcStr;
use derive_more::Display;
use dice::Demand;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use dupe::ResultDupedErrExt;
use futures::FutureExt;
use futures::future::BoxFuture;
use itertools::Itertools;
use pagable::Pagable;
use pagable::StaticStr;
use pagable::pagable_typetag;

use crate::configuration::compute_platform_cfgs;
use crate::configuration::get_matched_cfg_keys_for_node;
use crate::execution::configure_exec_dep_with_modifiers;
use crate::execution::find_execution_platform_by_configuration;
use crate::execution::resolve_execution_group_platform;
use crate::execution::resolve_execution_platform;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum NodeCalculationError {
    #[error("expected `{0}` attribute to be a list but got `{1}`")]
    TargetCompatibleNotList(String, String),
    #[error(
        "`{0}` had both `{}` and `{}` attributes. It should only have one.",
        TARGET_COMPATIBLE_WITH_ATTRIBUTE.name,
        LEGACY_TARGET_COMPATIBLE_WITH_ATTRIBUTE.name
    )]
    BothTargetCompatibleWith(String),
    #[error(
        "Target {0} configuration transitioned\n\
        old: {1}\n\
        new: {2}\n\
        but attribute: {3}\n\
        resolved with old configuration to: {4}\n\
        resolved with new configuration to: {5}"
    )]
    TransitionAttrIncompatibleChange(
        TargetLabel,
        ConfigurationData,
        ConfigurationData,
        String,
        String,
        String,
    ),

    #[error(
        "Target {0} configuration transition is not idempotent
         in initial configuration  `{1}`
         first transitioned to cfg `{2}`
         then transitions to cfg   `{3}`
         Use `buck2 audit configurations {1} {2} {3}` to see the configurations."
    )]
    TransitionNotIdempotent(
        TargetLabel,
        ConfigurationData,
        ConfigurationData,
        ConfigurationData,
    ),
}

enum CompatibilityConstraints {
    Any(ConfiguredAttr),
    All(ConfiguredAttr),
}

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum ToolchainDepError {
    #[error("Target `{0}` was used as a toolchain_dep, but is not a toolchain rule")]
    NonToolchainRuleUsedAsToolchainDep(TargetLabel),
    #[error("Target `{0}` was used not as a toolchain_dep, but is a toolchain rule")]
    ToolchainRuleUsedAsNormalDep(TargetLabel),
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum PluginDepError {
    #[error("Plugin dep `{0}` is a toolchain rule")]
    PluginDepIsToolchainRule(TargetLabel),
}

fn unpack_target_compatible_with_attr(
    target_label: &ConfiguredTargetLabel,
    target_node: TargetNodeRef,
    resolved_cfg: &MatchedConfigurationSettingKeysWithCfg,
    attr_id: AttributeId,
) -> buck2_error::Result<MaybeCompatible<Option<ConfiguredAttr>>> {
    let attr = target_node.known_attr_or_none(attr_id, AttrInspectOptions::All);
    let attr = match attr {
        Some(attr) => attr,
        None => return Ok(MaybeCompatible::Compatible(None)),
    };

    struct AttrConfigurationContextToResolveCompatibleWith<'c> {
        target_label: &'c ConfiguredTargetLabel,
        resolved_cfg: &'c MatchedConfigurationSettingKeysWithCfg,
    }

    impl AttrConfigurationContext for AttrConfigurationContextToResolveCompatibleWith<'_> {
        fn matched_cfg_keys(&self) -> &MatchedConfigurationSettingKeys {
            self.resolved_cfg.settings()
        }

        fn cfg(&self) -> ConfigurationNoExec {
            self.resolved_cfg.cfg().dupe()
        }

        fn base_exec_cfg(&self) -> buck2_error::Result<ConfigurationNoExec> {
            Err(internal_error!(
                "exec_cfg() is not needed to resolve `{}` or `{}`",
                TARGET_COMPATIBLE_WITH_ATTRIBUTE.name,
                LEGACY_TARGET_COMPATIBLE_WITH_ATTRIBUTE.name
            ))
        }

        fn toolchain_cfg(&self) -> ConfigurationWithExec {
            unreachable!()
        }

        fn platform_cfg(&self, _label: &TargetLabel) -> buck2_error::Result<ConfigurationData> {
            unreachable!(
                "platform_cfg() is not needed to resolve `{}` or `{}`",
                TARGET_COMPATIBLE_WITH_ATTRIBUTE.name, LEGACY_TARGET_COMPATIBLE_WITH_ATTRIBUTE.name
            )
        }

        fn resolved_transitions(
            &self,
        ) -> buck2_error::Result<&BuckFixedSortedMap<Arc<TransitionId>, Arc<TransitionApplied>>>
        {
            Err(internal_error!(
                "resolved_transitions() is not needed to resolve `{}` or `{}`",
                TARGET_COMPATIBLE_WITH_ATTRIBUTE.name,
                LEGACY_TARGET_COMPATIBLE_WITH_ATTRIBUTE.name
            ))
        }

        fn incompatible_platform_reason(
            &self,
            cause: IncompatiblePlatformReasonCause,
        ) -> Arc<IncompatiblePlatformReason> {
            Arc::new(IncompatiblePlatformReason {
                target: self.target_label.dupe(),
                cause,
            })
        }
    }

    let attr = match attr
        .configure(&AttrConfigurationContextToResolveCompatibleWith {
            resolved_cfg,
            target_label,
        })
        .with_buck_error_context(|| format!("Error configuring attribute `{}`", attr.name))
    {
        ResultMaybeCompatible::Compatible(attr) => attr,
        // A `select_incompatible()` branch makes the target incompatible here as in any
        // other attribute, rather than failing the build.
        ResultMaybeCompatible::Incompatible(reason) => {
            return Ok(MaybeCompatible::Incompatible(reason));
        }
        ResultMaybeCompatible::Err(e) => return Err(e),
    };

    match attr.value.unpack_list() {
        Some(values) => {
            if !values.is_empty() {
                Ok(MaybeCompatible::Compatible(Some(attr.value)))
            } else {
                Ok(MaybeCompatible::Compatible(None))
            }
        }
        None => Err(NodeCalculationError::TargetCompatibleNotList(
            attr.name.to_owned(),
            attr.value.as_display_no_ctx().to_string(),
        )
        .into()),
    }
}

fn check_compatible(
    target_label: &ConfiguredTargetLabel,
    target_node: TargetNodeRef,
    resolved_cfg: &MatchedConfigurationSettingKeysWithCfg,
) -> buck2_error::Result<MaybeCompatible<()>> {
    let target_compatible_with = match unpack_target_compatible_with_attr(
        target_label,
        target_node,
        resolved_cfg,
        TARGET_COMPATIBLE_WITH_ATTRIBUTE.id,
    )? {
        MaybeCompatible::Compatible(attr) => attr,
        MaybeCompatible::Incompatible(reason) => return Ok(MaybeCompatible::Incompatible(reason)),
    };
    let legacy_compatible_with = match unpack_target_compatible_with_attr(
        target_label,
        target_node,
        resolved_cfg,
        LEGACY_TARGET_COMPATIBLE_WITH_ATTRIBUTE.id,
    )? {
        MaybeCompatible::Compatible(attr) => attr,
        MaybeCompatible::Incompatible(reason) => return Ok(MaybeCompatible::Incompatible(reason)),
    };

    let compatibility_constraints = match (target_compatible_with, legacy_compatible_with) {
        (None, None) => return Ok(MaybeCompatible::Compatible(())),
        (Some(..), Some(..)) => {
            return Err(
                NodeCalculationError::BothTargetCompatibleWith(target_label.to_string()).into(),
            );
        }
        (Some(target_compatible_with), None) => {
            CompatibilityConstraints::All(target_compatible_with)
        }
        (None, Some(legacy_compatible_with)) => {
            CompatibilityConstraints::Any(legacy_compatible_with)
        }
    };

    // We are compatible if the list of target expressions is empty,
    // OR if we match ANY expression in the list of attributes.
    let check_compatibility = |attr| -> buck2_error::Result<(Vec<_>, Vec<_>)> {
        let mut left = Vec::new();
        let mut right = Vec::new();
        for label in ConfiguredTargetNode::attr_as_target_compatible_with(attr) {
            let label = label?;
            match resolved_cfg.settings().setting_matches(&label) {
                Some(_) => left.push(label),
                None => right.push(label),
            }
        }

        Ok((left, right))
    };

    // We only record the first incompatibility, for either ANY or ALL.
    // TODO(cjhopman): Should we report _all_ the things that are incompatible?
    let incompatible_target = match compatibility_constraints {
        CompatibilityConstraints::Any(attr) => {
            let (compatible, incompatible) =
                check_compatibility(attr).with_buck_error_context(|| {
                    format!(
                        "attribute `{}`",
                        LEGACY_TARGET_COMPATIBLE_WITH_ATTRIBUTE.name
                    )
                })?;
            let incompatible = incompatible.into_iter().next();
            match (compatible.is_empty(), incompatible.into_iter().next()) {
                (false, _) | (true, None) => {
                    return Ok(MaybeCompatible::Compatible(()));
                }
                (true, Some(v)) => v,
            }
        }
        CompatibilityConstraints::All(attr) => {
            let (_compatible, incompatible) =
                check_compatibility(attr).with_buck_error_context(|| {
                    format!("attribute `{}`", TARGET_COMPATIBLE_WITH_ATTRIBUTE.name)
                })?;
            match incompatible.into_iter().next() {
                Some(label) => label,
                None => {
                    return Ok(MaybeCompatible::Compatible(()));
                }
            }
        }
    };
    Ok(MaybeCompatible::Incompatible(Arc::new(
        IncompatiblePlatformReason {
            target: target_label.dupe(),
            cause: IncompatiblePlatformReasonCause::UnsatisfiedConfig(incompatible_target.0.dupe()),
        },
    )))
}

/// Ideally, we would check this much earlier. However, that turns out to be a bit tricky to
/// implement. Naively implementing this check on unconfigured nodes doesn't work because it results
/// in dice cycles when there are cycles in the unconfigured graph.
async fn check_plugin_deps(
    ctx: &mut DiceComputations<'_>,
    target_label: &ConfiguredTargetLabel,
    plugin_deps: &PluginLists,
) -> buck2_error::Result<()> {
    let package_visibility = ctx
        .per_transaction_data()
        .get_package_visibility_default_intersection();
    for (_, dep_label, elem_kind) in plugin_deps.iter() {
        if *elem_kind == PluginListElemKind::Direct {
            let dep_node = ctx
                .get_target_node(dep_label)
                .await
                .with_buck_error_context(|| {
                    format!("looking up unconfigured target node `{dep_label}`")
                })?;
            if dep_node.is_toolchain_rule() {
                return Err(PluginDepError::PluginDepIsToolchainRule(dep_label.dupe()).into());
            }
            check_dep_visibility(&dep_node, *target_label.unconfigured(), package_visibility)?;
        }
    }
    Ok(())
}

/// Under `audit`, a dep blocked only by `Default` intersection layers is reported
/// as a soft error instead of failing. A dep whose own `visibility` or a
/// `Marker` layer rejects the consumer fails in every mode, so `audit` is never
/// weaker than `off`.
fn check_dep_visibility(
    dep: &TargetNode,
    consumer: TargetLabel,
    package_visibility: PackageVisibilityDefaultIntersection,
) -> buck2_error::Result<()> {
    if dep.is_visible_to(&consumer)? {
        return Ok(());
    }
    let err = dep.not_visible_to_error(consumer);
    if package_visibility == PackageVisibilityDefaultIntersection::Audit
        && dep.is_visible_to_ignoring_default_layers(&consumer)?
    {
        deprecation!(
            "package_visibility_audit_would_block",
            err.into(),
            owner: "dust", started: "2026-10-02",
            low_cardinality_key_for_additional_logview_samples: Some(Box::new(dep.label().pkg())),
        );
        return Ok(());
    }
    Err(err.into())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CheckVisibility {
    Yes,
    No,
}

/// Adds `dep` to `deps`. A dep already present keeps `Yes` if either entry checks visibility.
fn insert_dep_keeping_stricter_visibility<K: Hash + Eq>(
    deps: &mut BuckMutInsertionMap<K, CheckVisibility>,
    dep: K,
    check_visibility: CheckVisibility,
) {
    let entry = deps.entry(dep).or_insert(check_visibility);
    if check_visibility == CheckVisibility::Yes {
        *entry = CheckVisibility::Yes;
    }
}

#[derive(Default)]
pub(crate) struct ErrorsAndIncompatibilities {
    errs: Vec<buck2_error::Error>,
    incompats: Vec<Arc<IncompatiblePlatformReason>>,
}

impl ErrorsAndIncompatibilities {
    fn unpack_dep_into(
        &mut self,
        target_label: &TargetConfiguredTargetLabel,
        result: ResultMaybeCompatible<&ConfiguredTargetNode>,
        check_visibility: CheckVisibility,
        package_visibility: PackageVisibilityDefaultIntersection,
        list: &mut Vec<ConfiguredTargetNode>,
    ) {
        list.extend(
            self.unpack_dep(target_label, result, check_visibility, package_visibility)
                .map(Dupe::dupe),
        );
    }

    fn unpack_dep<'x>(
        &mut self,
        target_label: &TargetConfiguredTargetLabel,
        result: ResultMaybeCompatible<&'x ConfiguredTargetNode>,
        check_visibility: CheckVisibility,
        package_visibility: PackageVisibilityDefaultIntersection,
    ) -> Option<&'x ConfiguredTargetNode> {
        match result {
            ResultMaybeCompatible::Err(e) => {
                self.errs.push(e);
            }
            ResultMaybeCompatible::Incompatible(reason) => {
                self.incompats.push(Arc::new(IncompatiblePlatformReason {
                    target: target_label.inner().dupe(),
                    cause: IncompatiblePlatformReasonCause::Dependency(reason.dupe()),
                }));
            }
            ResultMaybeCompatible::Compatible(dep) => {
                if CheckVisibility::No == check_visibility {
                    return Some(dep);
                }
                match check_dep_visibility(
                    dep.target_node(),
                    *target_label.unconfigured(),
                    package_visibility,
                ) {
                    Ok(()) => {
                        return Some(dep);
                    }
                    Err(e) => {
                        self.errs.push(e);
                    }
                }
            }
        }
        None
    }

    /// Returns an error/incompatibility to return, if any, and `None` otherwise
    pub(crate) fn finalize(mut self) -> ResultMaybeCompatible<()> {
        // FIXME(JakobDegen): Report all incompatibilities
        if let Some(incompat) = self.incompats.pop() {
            return ResultMaybeCompatible::Incompatible(incompat);
        }
        if let Some(err) = self.errs.pop() {
            return ResultMaybeCompatible::Err(err);
        }
        ResultMaybeCompatible::Compatible(())
    }
}

#[derive(Default)]
pub(crate) struct GatheredDeps {
    pub(crate) deps: Vec<ConfiguredTargetNode>,
    /// Exec deps of the default execution group.
    pub(crate) exec_deps: BuckMutInsertionMap<ConfiguredProvidersLabel, CheckVisibility>,
    /// Toolchain deps of the default execution group.
    pub(crate) toolchain_deps: BuckMutInsertionSet<TargetConfiguredTargetLabel>,
    pub(crate) plugin_lists: PluginLists,
    /// Deps of named execution groups. Groups the rule declares but no attribute value uses are
    /// absent.
    pub(crate) execution_group_deps:
        BuckMutInsertionMap<NamedExecutionGroup, GatheredExecutionGroupDeps>,
}

/// The deps that determine the execution platform of a named execution group.
#[derive(Default)]
pub(crate) struct GatheredExecutionGroupDeps {
    pub(crate) exec_deps: BuckMutInsertionMap<ConfiguredProvidersLabel, CheckVisibility>,
    pub(crate) toolchain_deps: BuckMutInsertionSet<TargetConfiguredTargetLabel>,
}

pub(crate) async fn gather_deps(
    target_label: &TargetConfiguredTargetLabel,
    target_node: TargetNodeRef<'_>,
    attr_cfg_ctx: &(dyn AttrConfigurationContext + Sync),
    ctx: &mut DiceComputations<'_>,
) -> ResultMaybeCompatible<(GatheredDeps, ErrorsAndIncompatibilities)> {
    #[derive(Default)]
    struct Traversal {
        deps: BuckMutInsertionMap<ConfiguredProvidersLabel, BuckMutInsertionSet<PluginKindSet>>,
        exec_deps: BuckMutInsertionMap<ConfiguredProvidersLabel, CheckVisibility>,
        toolchain_deps: BuckMutInsertionSet<TargetConfiguredTargetLabel>,
        plugin_lists: PluginListsBuilder,
        execution_group_deps: BuckMutInsertionMap<NamedExecutionGroup, GatheredExecutionGroupDeps>,
    }

    impl ConfiguredAttrTraversal for Traversal {
        fn dep(&mut self, dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
            self.deps.entry(dep.dupe()).or_default();
            Ok(())
        }

        fn dep_with_plugins(
            &mut self,
            dep: &ConfiguredProvidersLabel,
            plugin_kinds: &PluginKindSet,
        ) -> buck2_error::Result<()> {
            self.deps
                .entry(dep.dupe())
                .or_default()
                .insert(plugin_kinds.dupe());
            Ok(())
        }

        fn exec_dep(&mut self, dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
            self.exec_deps.insert(dep.dupe(), CheckVisibility::Yes);
            Ok(())
        }

        fn toolchain_dep(&mut self, dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
            self.toolchain_deps
                .insert(TargetConfiguredTargetLabel::new_without_exec_cfg(
                    dep.target().dupe(),
                ));
            Ok(())
        }

        fn plugin_dep(&mut self, dep: &TargetLabel, kind: &PluginKind) -> buck2_error::Result<()> {
            self.plugin_lists
                .insert(kind.dupe(), dep.dupe(), PluginListElemKind::Direct);
            Ok(())
        }

        fn exec_dep_in_group(
            &mut self,
            dep: &ConfiguredProvidersLabel,
            group: &NamedExecutionGroup,
        ) -> buck2_error::Result<()> {
            self.execution_group_deps
                .entry(group.dupe())
                .or_default()
                .exec_deps
                .insert(dep.dupe(), CheckVisibility::Yes);
            Ok(())
        }

        fn toolchain_dep_in_group(
            &mut self,
            dep: &ConfiguredProvidersLabel,
            group: &NamedExecutionGroup,
        ) -> buck2_error::Result<()> {
            self.execution_group_deps
                .entry(group.dupe())
                .or_default()
                .toolchain_deps
                .insert(TargetConfiguredTargetLabel::new_without_exec_cfg(
                    dep.target().dupe(),
                ));
            Ok(())
        }
    }

    let mut traversal = Traversal::default();
    for a in target_node.attrs(AttrInspectOptions::All) {
        a.traverse_configured(target_node.label().pkg(), attr_cfg_ctx, &mut traversal)?;
    }

    let dep_results = ctx
        .compute_join(traversal.deps.iter(), async |ctx, v| {
            ctx.get_internal_configured_target_node(v.0.target()).await
        })
        .await;

    let mut plugin_lists = traversal.plugin_lists;
    let mut deps = Vec::new();
    let mut errors_and_incompats = ErrorsAndIncompatibilities::default();
    let package_visibility = ctx
        .per_transaction_data()
        .get_package_visibility_default_intersection();
    for (res, (_, plugin_kind_sets)) in dep_results.into_iter().zip(traversal.deps) {
        let Some(dep) = errors_and_incompats.unpack_dep(
            target_label,
            res,
            CheckVisibility::Yes,
            package_visibility,
        ) else {
            continue;
        };

        if !plugin_kind_sets.is_empty() {
            for (kind, plugins) in dep.plugin_lists().iter_by_kind() {
                let Some(should_propagate) = plugin_kind_sets
                    .iter()
                    .filter_map(|set| set.get(kind))
                    .reduce(std::ops::BitOr::bitor)
                else {
                    continue;
                };
                let should_propagate = if should_propagate {
                    PluginListElemKind::Propagate
                } else {
                    PluginListElemKind::NoPropagate
                };
                for (target, elem_kind) in plugins {
                    if *elem_kind != PluginListElemKind::NoPropagate {
                        plugin_lists.insert(kind.dupe(), target.dupe(), should_propagate);
                    }
                }
            }
        }

        deps.push(dep.dupe());
    }
    let plugin_lists = plugin_lists.freeze();

    let mut exec_deps = traversal.exec_deps;
    for kind in target_node.uses_plugins() {
        for plugin_label in plugin_lists.iter_for_kind(kind).map(|(target, _)| {
            attr_cfg_ctx.configure_exec_target(&ProvidersLabel::default_for(target.dupe()))
        }) {
            exec_deps
                .entry(plugin_label?)
                .or_insert(CheckVisibility::No);
        }
    }

    ResultMaybeCompatible::Compatible((
        GatheredDeps {
            deps,
            exec_deps,
            toolchain_deps: traversal.toolchain_deps,
            plugin_lists,
            execution_group_deps: traversal.execution_group_deps,
        },
        errors_and_incompats,
    ))
}

/// A named execution group's execution platform, and its exec deps and toolchain deps configured
/// for that platform.
struct ConfiguredExecutionGroup<'d> {
    group: NamedExecutionGroup,
    partial: ExecutionPlatformResolutionPartial,
    toolchain_deps: Vec<ResultMaybeCompatible<&'d ConfiguredTargetNode>>,
    exec_deps: Vec<(
        ResultMaybeCompatible<&'d ConfiguredTargetNode>,
        CheckVisibility,
    )>,
}

/// Resolves the execution platform of every named execution group the rule declares, and
/// configures the exec deps and toolchain deps of each group for its platform. The groups are resolved
/// concurrently; the result is in declaration order, and its error is that of the first failing
/// group in declaration order.
async fn configure_execution_groups<'d>(
    ctx: &mut DiceComputations<'d>,
    target_node: TargetNodeRef<'_>,
    resolved_configuration: &MatchedConfigurationSettingKeysWithCfg,
    gathered_deps: &GatheredDeps,
    attr_cfg_ctx: &(dyn AttrConfigurationContext + Sync),
) -> buck2_error::Result<Vec<ConfiguredExecutionGroup<'d>>> {
    ctx.compute_join(target_node.execution_groups(), async |ctx, group| {
        configure_execution_group(
            ctx,
            target_node,
            resolved_configuration,
            gathered_deps,
            group,
            attr_cfg_ctx,
        )
        .boxed()
        .await
    })
    .await
    .into_iter()
    .collect()
}

async fn configure_execution_group<'d>(
    ctx: &mut DiceComputations<'d>,
    target_node: TargetNodeRef<'_>,
    resolved_configuration: &MatchedConfigurationSettingKeysWithCfg,
    gathered_deps: &GatheredDeps,
    rule_group: &RuleExecutionGroup,
    attr_cfg_ctx: &(dyn AttrConfigurationContext + Sync),
) -> buck2_error::Result<ConfiguredExecutionGroup<'d>> {
    let group = &rule_group.group;
    let partial = resolve_execution_group_platform(
        ctx,
        target_node,
        resolved_configuration,
        gathered_deps,
        rule_group,
        attr_cfg_ctx,
    )
    .await
    .with_buck_error_context(|| {
        format!("Error resolving the execution platform of execution group `{group}`")
    })?;
    let group_cfg = &partial.cfg();
    let no_deps = GatheredExecutionGroupDeps::default();
    let group_deps = gathered_deps
        .execution_group_deps
        .get(group)
        .unwrap_or(&no_deps);

    let get_toolchain_deps = DiceComputations::declare_closure(async |ctx| {
        ctx.compute_join(
            &group_deps.toolchain_deps,
            async |ctx, target: &TargetConfiguredTargetLabel| {
                ctx.get_internal_configured_target_node(
                    &target.with_exec_cfg(group_cfg.cfg().dupe()),
                )
                .await
            },
        )
        .await
    });
    let get_exec_deps = DiceComputations::declare_closure(async |ctx| {
        ctx.compute_join(
            &group_deps.exec_deps,
            async |ctx, (target, check_visibility)| {
                let result = configure_exec_dep_with_modifiers(
                    ctx,
                    target.target().unconfigured(),
                    group_cfg.cfg(),
                )
                .await;
                (result, *check_visibility)
            },
        )
        .await
    });
    let (toolchain_deps, exec_deps) = ctx.compute2(get_toolchain_deps, get_exec_deps).await;
    Ok(ConfiguredExecutionGroup {
        group: group.dupe(),
        partial,
        toolchain_deps,
        exec_deps,
    })
}

/// Adds the toolchain deps of each configured execution group to the node's `deps` and its exec
/// deps to the node's `exec_deps`, and returns each group's resolution. A dep configured
/// identically in several groups is a dep of the node once.
fn merge_execution_groups(
    groups: Vec<ConfiguredExecutionGroup<'_>>,
    partial_target_label: &TargetConfiguredTargetLabel,
    package_visibility: PackageVisibilityDefaultIntersection,
    errors_and_incompats: &mut ErrorsAndIncompatibilities,
    deps: &mut Vec<ConfiguredTargetNode>,
    exec_deps: &mut Vec<ConfiguredTargetNode>,
) -> Vec<ExecutionGroupResolutionPartial> {
    groups
        .into_iter()
        .map(|group| {
            let mut group_toolchain_deps = Vec::with_capacity(group.toolchain_deps.len());
            for dep in group.toolchain_deps {
                errors_and_incompats.unpack_dep_into(
                    partial_target_label,
                    dep,
                    CheckVisibility::Yes,
                    package_visibility,
                    &mut group_toolchain_deps,
                );
            }
            push_new_deps(deps, group_toolchain_deps);

            let mut group_exec_deps = Vec::with_capacity(group.exec_deps.len());
            for (dep, check_visibility) in group.exec_deps {
                errors_and_incompats.unpack_dep_into(
                    partial_target_label,
                    dep,
                    check_visibility,
                    package_visibility,
                    &mut group_exec_deps,
                );
            }
            let exec_dep_cfgs = group_exec_deps
                .iter()
                .map(|dep| (dep.label().unconfigured().dupe(), dep.label().cfg().dupe()))
                .collect();
            push_new_deps(exec_deps, group_exec_deps);
            ExecutionGroupResolutionPartial {
                group: group.group,
                partial: group.partial,
                exec_dep_cfgs,
            }
        })
        .collect()
}

/// Appends the deps of `new` that `deps` does not already contain. A linear scan: a group has
/// few deps, and this allocates nothing.
fn push_new_deps(deps: &mut Vec<ConfiguredTargetNode>, new: Vec<ConfiguredTargetNode>) {
    for dep in new {
        if !deps.iter().any(|d| d.label() == dep.label()) {
            deps.push(dep);
        }
    }
}

/// Resolves configured attributes of target node needed to compute transitions
async fn resolve_transition_input_attrs<'a>(
    target_label: &ConfiguredTargetLabel,
    transitions: impl Iterator<Item = &TransitionId>,
    target_node: &'a TargetNode,
    matched_cfg_keys: &MatchedConfigurationSettingKeysWithCfg,
    platform_cfgs: &BuckFixedSortedMap<TargetLabel, ConfigurationData>,
    ctx: &mut DiceComputations<'_>,
) -> ResultMaybeCompatible<BuckMutInsertionMap<&'a str, Arc<ConfiguredAttr>>> {
    struct AttrConfigurationContextToResolveTransitionAttrs<'c> {
        target_label: &'c ConfiguredTargetLabel,
        matched_cfg_keys: &'c MatchedConfigurationSettingKeysWithCfg,
        toolchain_cfg: ConfigurationWithExec,
        platform_cfgs: &'c BuckFixedSortedMap<TargetLabel, ConfigurationData>,
    }

    impl AttrConfigurationContext for AttrConfigurationContextToResolveTransitionAttrs<'_> {
        fn matched_cfg_keys(&self) -> &MatchedConfigurationSettingKeys {
            self.matched_cfg_keys.settings()
        }

        fn cfg(&self) -> ConfigurationNoExec {
            self.matched_cfg_keys.cfg().dupe()
        }

        fn base_exec_cfg(&self) -> buck2_error::Result<ConfigurationNoExec> {
            Err(internal_error!(
                "exec_cfg() is not needed in pre transition attribute resolution."
            ))
        }

        fn toolchain_cfg(&self) -> ConfigurationWithExec {
            self.toolchain_cfg.dupe()
        }

        fn platform_cfg(&self, label: &TargetLabel) -> buck2_error::Result<ConfigurationData> {
            match self.platform_cfgs.get(label) {
                Some(configuration) => Ok(configuration.dupe()),
                None => Err(PlatformConfigurationError::UnknownPlatformTarget(label.dupe()).into()),
            }
        }

        fn resolved_transitions(
            &self,
        ) -> buck2_error::Result<&BuckFixedSortedMap<Arc<TransitionId>, Arc<TransitionApplied>>>
        {
            // TODO(cjhopman): Why is this an internal error? Doesn't it indicate an error in the
            // rule or the target definition? Do we enforce it somewhere else as an input error?
            Err(internal_error!(
                "resolved_transitions() can't be used before transition execution."
            ))
        }

        fn incompatible_platform_reason(
            &self,
            cause: IncompatiblePlatformReasonCause,
        ) -> Arc<IncompatiblePlatformReason> {
            Arc::new(IncompatiblePlatformReason {
                target: self.target_label.dupe(),
                cause,
            })
        }
    }

    let cfg_ctx = AttrConfigurationContextToResolveTransitionAttrs {
        target_label,
        matched_cfg_keys,
        platform_cfgs,
        toolchain_cfg: matched_cfg_keys
            .cfg()
            .make_toolchain(&ConfigurationNoExec::unbound_exec()),
    };
    let mut result = BuckMutInsertionMap::default();
    for tr in transitions {
        let attrs = TRANSITION_ATTRS_PROVIDER
            .get()?
            .transition_attrs(ctx, tr)
            .await?;
        if let Some(attrs) = attrs {
            for attr in attrs {
                // Multiple outgoing transitions may refer the same attribute.
                if result.contains_key(attr.as_str()) {
                    continue;
                }

                if let Some(coerced_attr) = target_node.attr(attr, AttrInspectOptions::All)? {
                    let configured_attr = coerced_attr.configure(&cfg_ctx)?;
                    if let Some(old_val) =
                        result.insert(configured_attr.name, Arc::new(configured_attr.value))
                    {
                        return internal_error!(
                            "Found duplicated value `{}` for attr `{}` on target `{}`",
                            &old_val.as_display_no_ctx(),
                            attr,
                            target_node.label()
                        )
                        .into();
                    }
                }
            }
        }
    }
    ResultMaybeCompatible::Compatible(result)
}

/// Verifies if configured node's attributes are equal to the same attributes configured with pre-transition configuration.
/// Only check attributes used in transition.
fn verify_transitioned_attrs(
    // Attributes resolved with pre-transition configuration
    pre_transition_attrs: &BuckMutInsertionMap<&str, Arc<ConfiguredAttr>>,
    pre_transition_config: &ConfigurationData,
    node: &ConfiguredTargetNode,
) -> buck2_error::Result<()> {
    for (attr, attr_value) in pre_transition_attrs {
        let transition_configured_attr =
            node.get(attr, AttrInspectOptions::All).ok_or_else(|| {
                internal_error!(
                    "Attr {} was not found in transition for target {} ({})",
                    attr,
                    node.label(),
                    node.attrs(AttrInspectOptions::All)
                        .format_with(", ", |v, f| f(&format_args!("{v:?}")))
                )
            })?;
        if &transition_configured_attr.value != attr_value.as_ref() {
            return Err(NodeCalculationError::TransitionAttrIncompatibleChange(
                node.label().unconfigured().dupe(),
                pre_transition_config.dupe(),
                node.label().cfg().dupe(),
                attr.to_string(),
                attr_value.as_display_no_ctx().to_string(),
                transition_configured_attr
                    .value
                    .as_display_no_ctx()
                    .to_string(),
            )
            .into());
        }
    }
    Ok(())
}

/// The prefix of configuring a target that must run before execution platform resolution:
/// configuration-dep matching, the compatibility check, platform cfgs, transition resolution,
/// and dependency gathering. Shared between the real configuration path and the audit
/// diagnostic re-traversal.
pub(crate) struct ConfiguredNodePreamble<'d> {
    pub(crate) resolved_configuration: &'d MatchedConfigurationSettingKeysWithCfg,
    pub(crate) platform_cfgs: BuckFixedSortedMap<TargetLabel, ConfigurationData>,
    pub(crate) resolved_transitions: BuckFixedSortedMap<Arc<TransitionId>, Arc<TransitionApplied>>,
    pub(crate) gathered_deps: GatheredDeps,
    pub(crate) errors_and_incompats: ErrorsAndIncompatibilities,
}

impl ConfiguredNodePreamble<'_> {
    /// The unspecified-exec attr configuration context, as used for dependency gathering.
    /// Exec-dep and toolchain-dep cfgs from it are placeholders that callers replace once
    /// execution platform resolution has run. The real configuration path builds this context
    /// inline from the destructured pieces instead (see the size-assert comment there).
    pub(crate) fn attr_cfg_ctx(
        &self,
        target_label: &ConfiguredTargetLabel,
    ) -> AttrConfigurationContextImpl<'_> {
        unspecified_attr_cfg_ctx(
            target_label,
            self.resolved_configuration,
            &self.resolved_transitions,
            &self.platform_cfgs,
        )
    }
}

/// The attr configuration context as it exists before execution platform resolution has run:
/// exec-dep and toolchain-dep cfgs from it are placeholders that callers replace with the
/// resolved exec cfg. Sole constructor for this context; the placeholder resolution and owner
/// label are supplied in here, so construction sites cannot drift apart in them.
pub(crate) fn unspecified_attr_cfg_ctx<'v>(
    target_label: &ConfiguredTargetLabel,
    resolved_configuration: &'v MatchedConfigurationSettingKeysWithCfg,
    resolved_transitions: &'v BuckFixedSortedMap<Arc<TransitionId>, Arc<TransitionApplied>>,
    platform_cfgs: &'v BuckFixedSortedMap<TargetLabel, ConfigurationData>,
) -> AttrConfigurationContextImpl<'v> {
    // A `static` rather than a promoted `&Unspecified` rvalue: the enum's other variant has
    // drop glue, which rules out static promotion.
    static UNSPECIFIED_RESOLUTION: ExecutionPlatformResolution =
        ExecutionPlatformResolution::Unspecified;
    AttrConfigurationContextImpl::new(
        target_label.dupe(),
        resolved_configuration,
        &UNSPECIFIED_RESOLUTION,
        resolved_transitions,
        platform_cfgs,
    )
}

pub(crate) async fn compute_configured_node_preamble<'d>(
    ctx: &mut DiceComputations<'d>,
    target_label: &ConfiguredTargetLabel,
    target_node: &TargetNode,
) -> ResultMaybeCompatible<ConfiguredNodePreamble<'d>> {
    let partial_target_label =
        &TargetConfiguredTargetLabel::new_without_exec_cfg(target_label.dupe());
    let target_cfg = target_label.cfg();
    let target_cell = target_node.label().pkg().cell_name();
    let resolved_configuration = get_matched_cfg_keys_for_node(
        ctx,
        target_cfg,
        CellNameForConfigurationResolution(target_cell),
        target_node.as_ref(),
    )
    .await
    .with_buck_error_context(|| {
        format!("Error resolving configuration deps of `{target_label}`")
    })?;

    // Must check for compatibility before evaluating non-compatibility attributes.
    if let MaybeCompatible::Incompatible(reason) =
        check_compatible(target_label, target_node.as_ref(), resolved_configuration)?
    {
        return ResultMaybeCompatible::Incompatible(reason);
    }

    let platform_cfgs = compute_platform_cfgs(ctx, target_node.as_ref()).await?;

    let attrs = resolve_transition_input_attrs(
        target_label,
        target_node.transition_deps().map(|(_, tr)| tr.as_ref()),
        target_node,
        resolved_configuration,
        &platform_cfgs,
        ctx,
    )
    .boxed()
    .await?;
    let mut resolved_transitions = Vec::new();
    for (_dep, tr) in target_node.transition_deps() {
        let resolved_cfg = TRANSITION_CALCULATION
            .get()?
            .apply_transition(ctx, &attrs, target_cfg, tr)
            .await?;
        resolved_transitions.push((tr.dupe(), resolved_cfg));
    }
    let resolved_transitions: BuckFixedSortedMap<_, _> = resolved_transitions.into_iter().collect();

    // We need to collect deps and to ensure that all attrs can be successfully
    // configured so that we don't need to support propagate configuration errors on attr access.
    let attr_cfg_ctx = unspecified_attr_cfg_ctx(
        target_label,
        resolved_configuration,
        &resolved_transitions,
        &platform_cfgs,
    );

    let (gathered_deps, errors_and_incompats) = gather_deps(
        partial_target_label,
        target_node.as_ref(),
        &attr_cfg_ctx,
        ctx,
    )
    .boxed()
    .await?;

    ResultMaybeCompatible::Compatible(ConfiguredNodePreamble {
        resolved_configuration,
        platform_cfgs,
        resolved_transitions,
        gathered_deps,
        errors_and_incompats,
    })
}

/// Compute configured target node ignoring transition for this node.
async fn compute_configured_target_node_no_transition(
    target_label: &ConfiguredTargetLabel,
    target_node: TargetNode,
    ctx: &mut DiceComputations<'_>,
) -> ResultMaybeCompatible<ConfiguredTargetNode> {
    let target_cfg = target_label.cfg();
    // Destructured immediately and the attr context rebuilt from the pieces: keeping the whole
    // preamble alive across the awaits below grows this future past its size assert.
    let ConfiguredNodePreamble {
        resolved_configuration,
        platform_cfgs,
        resolved_transitions,
        gathered_deps,
        mut errors_and_incompats,
    } = compute_configured_node_preamble(ctx, target_label, &target_node)
        .boxed()
        .await?;

    check_plugin_deps(ctx, target_label, &gathered_deps.plugin_lists)
        .boxed()
        .await?;

    let attr_cfg_ctx = unspecified_attr_cfg_ctx(
        target_label,
        resolved_configuration,
        &resolved_transitions,
        &platform_cfgs,
    );

    let execution_platform_partial = if target_cfg.is_unbound() {
        // The unbound configuration is used when evaluation configuration nodes.
        // That evaluation is
        // (1) part of execution platform resolution and
        // (2) isn't allowed to do execution
        // And so we use an "unspecified" execution platform to avoid cycles and cause any attempts at execution to fail.
        None
    } else if let Some(exec_cfg) = target_label.exec_cfg()
        && exec_cfg == &ConfigurationData::unspecified_exec()
    {
        // The parent resolved to no execution platform (the `use_unspecified` fallback), and so
        // does its toolchain dep.
        Some(ExecutionPlatformResolutionPartial::new(None, Vec::new()))
    } else if let Some(exec_cfg) = target_label.exec_cfg() {
        // The label was produced by a toolchain_dep, so we use the execution platform of our parent
        // We need to convert that to an execution platform, so just find the one with the same configuration.
        Some(ExecutionPlatformResolutionPartial::new(
            Some(
                find_execution_platform_by_configuration(
                    ctx,
                    exec_cfg,
                    resolved_configuration.cfg().cfg(),
                )
                .await?,
            ),
            Vec::new(),
        ))
    } else {
        Some(
            resolve_execution_platform(
                ctx,
                target_node.as_ref(),
                resolved_configuration,
                &gathered_deps,
                &attr_cfg_ctx,
            )
            .boxed()
            .await?,
        )
    };

    // Get the execution platform configuration - either from partial or use unspecified
    let execution_platform_cfg = match &execution_platform_partial {
        Some(partial) => partial.cfg(),
        None => ConfigurationNoExec::unspecified_exec().dupe(),
    };

    // We now need to replace the dummy exec config we used above with the real one

    let execution_platform_cfg = &execution_platform_cfg;
    // An unbound node resolves no execution platforms, so the exec deps and toolchain deps of its
    // named execution groups are configured with the unspecified execution configuration, like
    // the default group's. Boxed to keep this future within its size assert.
    let unbound_toolchain_deps: Box<BuckMutInsertionSet<TargetConfiguredTargetLabel>>;
    let toolchain_deps =
        if execution_platform_partial.is_none() && !gathered_deps.execution_group_deps.is_empty() {
            let mut all = gathered_deps.toolchain_deps.clone();
            for group_deps in gathered_deps.execution_group_deps.values() {
                for dep in &group_deps.toolchain_deps {
                    all.insert(dep.dupe());
                }
            }
            unbound_toolchain_deps = Box::new(all);
            &*unbound_toolchain_deps
        } else {
            &gathered_deps.toolchain_deps
        };
    let unbound_exec_deps: Box<BuckMutInsertionMap<ConfiguredProvidersLabel, CheckVisibility>>;
    let exec_deps =
        if execution_platform_partial.is_none() && !gathered_deps.execution_group_deps.is_empty() {
            let mut all = gathered_deps.exec_deps.clone();
            for group_deps in gathered_deps.execution_group_deps.values() {
                for (dep, check_visibility) in &group_deps.exec_deps {
                    insert_dep_keeping_stricter_visibility(&mut all, dep.dupe(), *check_visibility);
                }
            }
            unbound_exec_deps = Box::new(all);
            &*unbound_exec_deps
        } else {
            &gathered_deps.exec_deps
        };

    let get_toolchain_deps = DiceComputations::declare_closure(async move |ctx| {
        ctx.compute_join(
            toolchain_deps,
            async |ctx, target: &TargetConfiguredTargetLabel| {
                ctx.get_internal_configured_target_node(
                    &target.with_exec_cfg(execution_platform_cfg.cfg().dupe()),
                )
                .await
            },
        )
        .await
    });

    let get_exec_deps = DiceComputations::declare_closure(async |ctx| {
        ctx.compute_join(exec_deps, async |ctx, (target, check_visibility)| {
            // Apply modifiers to exec_dep before configuring
            let result = configure_exec_dep_with_modifiers(
                ctx,
                target.target().unconfigured(),
                execution_platform_cfg.cfg(),
            )
            .await;

            (result, *check_visibility)
        })
        .await
    });

    let get_default_deps = DiceComputations::declare_closure(async |ctx| {
        ctx.compute2(get_toolchain_deps, get_exec_deps).await
    });
    let resolve_execution_groups =
        execution_platform_partial.is_some() && !target_node.execution_groups().is_empty();
    let get_execution_groups = DiceComputations::declare_closure(async |ctx| {
        if !resolve_execution_groups {
            return Ok(Vec::new());
        }
        configure_execution_groups(
            ctx,
            target_node.as_ref(),
            resolved_configuration,
            &gathered_deps,
            &attr_cfg_ctx,
        )
        .boxed()
        .await
    });

    let ((toolchain_dep_results, exec_dep_results), execution_groups): ((Vec<_>, Vec<_>), _) =
        ctx.compute2(get_default_deps, get_execution_groups).await;

    let mut deps = gathered_deps.deps;
    let mut exec_deps = Vec::with_capacity(gathered_deps.exec_deps.len());

    // Derived here rather than at the top of the function: it is only needed after the last
    // await, and deriving it early would keep the label value saved across every suspension
    // point above.
    let partial_target_label =
        &TargetConfiguredTargetLabel::new_without_exec_cfg(target_label.dupe());
    let package_visibility = ctx
        .per_transaction_data()
        .get_package_visibility_default_intersection();
    for dep in toolchain_dep_results {
        errors_and_incompats.unpack_dep_into(
            partial_target_label,
            dep,
            CheckVisibility::Yes,
            package_visibility,
            &mut deps,
        );
    }
    for (dep, check_visibility) in exec_dep_results {
        errors_and_incompats.unpack_dep_into(
            partial_target_label,
            dep,
            check_visibility,
            package_visibility,
            &mut exec_deps,
        );
    }

    // Build the exec_dep_cfgs mapping from exec_dep target labels to their actual cfgs.
    // This is needed because modifiers may change the cfg of exec_deps, and we need to
    // use the actual cfg when configuring exec_dep attributes during analysis.
    let exec_dep_cfgs = exec_deps
        .iter()
        .map(|exec_dep| {
            (
                exec_dep.label().unconfigured().dupe(),
                exec_dep.label().cfg().dupe(),
            )
        })
        .collect();

    let execution_group_resolutions = merge_execution_groups(
        execution_groups?,
        partial_target_label,
        package_visibility,
        &mut errors_and_incompats,
        &mut deps,
        &mut exec_deps,
    );

    // Finalize the execution platform resolution with exec_dep_cfgs
    let execution_platform_resolution = match execution_platform_partial {
        Some(partial) => partial.finalize_with_execution_groups(
            exec_dep_cfgs,
            execution_group_resolutions,
            target_node.default_execution_group(),
        ),
        None => ExecutionPlatformResolution::unspecified(),
    };

    errors_and_incompats.finalize()?;

    ResultMaybeCompatible::Compatible(ConfiguredTargetNode::new(
        target_label.dupe(),
        target_node.dupe(),
        resolved_configuration.dupe(),
        resolved_transitions,
        execution_platform_resolution,
        deps,
        exec_deps,
        platform_cfgs,
        gathered_deps.plugin_lists,
    ))
}

async fn compute_configured_target_node(
    key: &ConfiguredTargetNodeKey,
    ctx: &mut DiceComputations<'_>,
) -> ResultMaybeCompatible<ConfiguredTargetNode> {
    let target_node = ctx
        .get_target_node(key.0.unconfigured())
        .await
        .with_buck_error_context(|| {
            format!(
                "looking up unconfigured target node `{}`",
                key.0.unconfigured()
            )
        })?;

    match key.0.exec_cfg() {
        None if target_node.is_toolchain_rule() => {
            return ResultMaybeCompatible::Err(
                ToolchainDepError::ToolchainRuleUsedAsNormalDep(key.0.unconfigured().dupe()).into(),
            );
        }
        Some(_) if !target_node.is_toolchain_rule() => {
            return ResultMaybeCompatible::Err(
                ToolchainDepError::NonToolchainRuleUsedAsToolchainDep(key.0.unconfigured().dupe())
                    .into(),
            );
        }
        _ => {}
    }

    let transition_id = match &target_node.rule.cfg {
        RuleIncomingTransition::None => None,
        RuleIncomingTransition::Fixed(transition_id) => Some(transition_id.dupe()),
        RuleIncomingTransition::FromAttribute => target_node
            .attr_or_none(INCOMING_TRANSITION_ATTRIBUTE.name, AttrInspectOptions::All)
            .map(|v| v.value.unpack_configuration_dep())
            .transpose()?
            .flatten()
            .map(|l| Arc::new(TransitionId::Target(l.dupe()))),
    };

    if let Some(transition_id) = transition_id {
        compute_configured_forward_target_node(key, &target_node, &transition_id, ctx).await
    } else {
        // We are not caching `ConfiguredTransitionedNodeKey` because this is cheap,
        // and no need to fetch `target_node` again.
        compute_configured_target_node_no_transition(&key.0.dupe(), target_node, ctx).await
    }
}

async fn compute_configured_forward_target_node(
    key: &ConfiguredTargetNodeKey,
    target_node: &TargetNode,
    transition_id: &TransitionId,
    ctx: &mut DiceComputations<'_>,
) -> ResultMaybeCompatible<ConfiguredTargetNode> {
    let target_label_before_transition = &key.0;
    let platform_cfgs = compute_platform_cfgs(ctx, target_node.as_ref())
        .boxed()
        .await?;
    let matched_cfg_keys = get_matched_cfg_keys_for_node(
        ctx,
        target_label_before_transition.cfg(),
        CellNameForConfigurationResolution(target_node.label().pkg().cell_name()),
        target_node.as_ref(),
    )
    .await
    .with_buck_error_context(|| {
        format!("Error resolving configuration deps of `{target_label_before_transition}`")
    })?;

    let attrs = resolve_transition_input_attrs(
        target_label_before_transition,
        iter::once(transition_id),
        target_node,
        matched_cfg_keys,
        &platform_cfgs,
        ctx,
    )
    .boxed()
    .await?;

    let cfg = TRANSITION_CALCULATION
        .get()?
        .apply_transition(
            ctx,
            &attrs,
            target_label_before_transition.cfg(),
            transition_id,
        )
        .await?;
    let target_label_after_transition = target_label_before_transition
        .unconfigured()
        .configure(cfg.single()?.dupe());

    if &target_label_after_transition == target_label_before_transition {
        // Transitioned to identical configured target, no need to create a forward node.
        compute_configured_target_node_no_transition(
            target_label_before_transition,
            target_node.dupe(),
            ctx,
        )
        .boxed()
        .await
    } else {
        // This must call through dice to get the configured target node so that it is the correct
        // instance (because ConfiguredTargetNode uses reference equality on its deps).
        // This also helps further verify idempotence (as we will get the real result with the any transition applied again).
        let transitioned_node = ctx
            .get_internal_configured_target_node(&target_label_after_transition)
            .await?;

        // In apply_transition() above we've checked that the transition is idempotent when applied again with the same attrs (but the
        // transitioned cfg) we don't know if it causes an attr change (and then a subsequent change in the transition
        // result). We verify that here. If we're in a case where it is changing the attr in a way that causes the transition
        // to introduce a cycle, we depend on the dice cycle detection to identify it. Alternatively we could directly recompute
        // the node and check the attrs, but we'd still need to request the real node from dice and it doesn't seem worth
        // that extra cost just for a slightly improved error message.

        // check that the attrs weren't changed first. This should be the only way that we can hit non-idempotence
        // here and gives a better error than if we just give the general idempotence error.
        verify_transitioned_attrs(&attrs, matched_cfg_keys.cfg().cfg(), transitioned_node)?;

        if let Some(forward) = transitioned_node.forward_target() {
            return ResultMaybeCompatible::Err(NodeCalculationError::TransitionNotIdempotent(
                    target_label_before_transition.unconfigured().dupe(),
                    target_label_before_transition.cfg().dupe(),
                    target_label_after_transition.cfg().dupe(),
                    forward.label().cfg().dupe(),
                ).into())
                .internal_error("idempotence should have been enforced by transition idempotence and attr change checks");
        }

        let configured_target_node = ConfiguredTargetNode::new_forward(
            target_label_before_transition.dupe(),
            transitioned_node.dupe(),
        )?;

        ResultMaybeCompatible::Compatible(configured_target_node)
    }
}

#[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[pagable_typetag(dice::DiceKeyDyn)]
pub struct ConfiguredTargetNodeKey(pub ConfiguredTargetLabel);

struct ConfiguredTargetNodeCalculationInstance;

pub(crate) fn init_configured_target_node_calculation() {
    CONFIGURED_TARGET_NODE_CALCULATION.init(&ConfiguredTargetNodeCalculationInstance);
}

#[derive(Debug, Allocative, Eq, PartialEq)]
pub(crate) struct LookingUpConfiguredNodeContext {
    target: ConfiguredTargetLabel,
    len: usize,
    rest: Option<Arc<Self>>,
}

impl buck2_error::TypedContext for LookingUpConfiguredNodeContext {
    fn eq(&self, other: &dyn buck2_error::TypedContext) -> bool {
        match (other as &dyn std::any::Any).downcast_ref::<Self>() {
            Some(v) => self == v,
            None => false,
        }
    }

    fn display(&self) -> Option<String> {
        Some(format!("{}", self))
    }
}

impl LookingUpConfiguredNodeContext {
    pub(crate) fn new(target: ConfiguredTargetLabel, parent: Option<Arc<Self>>) -> Self {
        let (len, rest) = match parent {
            Some(v) => (v.len + 1, Some(v.clone())),
            None => (1, None),
        };
        Self { target, len, rest }
    }

    pub(crate) fn add_context<T>(
        res: buck2_error::Result<T>,
        target: ConfiguredTargetLabel,
    ) -> buck2_error::Result<T> {
        res.compute_context(
            |parent_ctx: Arc<Self>| Self::new(target.dupe(), Some(parent_ctx)),
            || Self::new(target.dupe(), None),
        )
    }

    pub(crate) fn add_context_rmc<T>(
        res: ResultMaybeCompatible<T>,
        target: ConfiguredTargetLabel,
    ) -> ResultMaybeCompatible<T> {
        res.compute_context(
            |parent_ctx: Arc<Self>| Self::new(target.dupe(), Some(parent_ctx)),
            || Self::new(target.dupe(), None),
        )
    }
}

impl std::fmt::Display for LookingUpConfiguredNodeContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.len == 1 {
            write!(f, "Error looking up configured node {}", self.target)?;
        } else {
            writeln!(
                f,
                "Error in configured node dependency, dependency chain follows (-> indicates depends on, ^ indicates same configuration as previous):"
            )?;

            let mut curr = self;
            let mut prev_cfg = None;
            let mut is_first = true;

            loop {
                f.write_str("    ")?;
                if is_first {
                    f.write_str("   ")?;
                } else {
                    f.write_str("-> ")?;
                }

                write!(f, "{}", curr.target.unconfigured())?;
                let cfg = Some(curr.target.cfg());
                f.write_str(" (")?;
                if cfg == prev_cfg {
                    f.write_str("^")?;
                } else {
                    std::fmt::Display::fmt(curr.target.cfg(), f)?;
                }
                f.write_str(")\n")?;
                is_first = false;
                prev_cfg = Some(curr.target.cfg());
                match &curr.rest {
                    Some(v) => curr = &**v,
                    None => break,
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Key for ConfiguredTargetNodeKey {
    type Value = ResultMaybeCompatible<ConfiguredTargetNode>;
    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellation: &CancellationContext,
    ) -> Self::Value {
        let res = compute_configured_target_node(self, ctx).await;
        LookingUpConfiguredNodeContext::add_context_rmc(res, self.0.dupe())
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (ResultMaybeCompatible::Compatible(x), ResultMaybeCompatible::Compatible(y)) => x == y,
            (ResultMaybeCompatible::Incompatible(x), ResultMaybeCompatible::Incompatible(y)) => {
                x == y
            }
            _ => false,
        })
    }

    fn provide<'a>(&'a self, demand: &mut Demand<'a>) {
        demand.provide_value_with(|| BuildSignalsNodeKey::new(self.dupe()))
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        ResultMaybeCompatibleValueSerialize::<ConfiguredTargetNode>::new()
    }
}

impl BuildSignalsNodeKeyImpl for ConfiguredTargetNodeKey {
    fn kind(&self) -> &'static str {
        "configure_target"
    }

    fn package(&self) -> Option<PackageLabel> {
        Some(self.0.unconfigured().pkg())
    }
}

impl ConfiguredTargetNodeCalculationImpl for ConfiguredTargetNodeCalculationInstance {
    fn get_configured_target_node<'a, 'd>(
        &self,
        ctx: &'a mut DiceComputations<'d>,
        target: &'a ConfiguredTargetLabel,
        check_dependency_incompatibility: bool,
    ) -> BoxFuture<'a, ResultMaybeCompatible<&'d ConfiguredTargetNode>>
    where
        'd: 'a,
    {
        async move {
            let maybe_compatible_result =
                ctx.compute(&ConfiguredTargetNodeKey(target.dupe())).await?;
            if check_dependency_incompatibility {
                if let ResultMaybeCompatible::Incompatible(reason) = maybe_compatible_result {
                    if matches!(
                        &reason.cause,
                        &IncompatiblePlatformReasonCause::Dependency(_)
                    ) {
                        if check_error_on_incompatible_dep(ctx, target.unconfigured_label()).await? {
                            return ResultMaybeCompatible::Err(reason.to_err());
                        }
                        deprecation!(
                            "dep_only_incompatible_version_two", reason.to_soft_err(),
                            owner: "scottcao", started: "2024-06-20",
                            // Log at least one sample per unique package.
                            low_cardinality_key_for_additional_logview_samples: Some(Box::new(target.unconfigured().pkg())),
                        );
                        if let Some(custom_soft_errors) = get_dep_only_incompatible_custom_soft_error(
                            ctx,
                            target.unconfigured_label(),
                        )
                        .await?
                        {
                            for custom_soft_error in custom_soft_errors {
                                deprecation!(
                                    &custom_soft_error,
                                    reason.to_soft_err(),
                                    owner: "scottcao", started: "2025-03-17",
                                    task: false,
                                );
                            }
                        }
                    }
                }
            }
            maybe_compatible_result.as_ref()
        }
        .boxed()
    }
}

pagable::static_str!(SECTION_BUCK2 = "buck2");
pagable::static_str!(PROPERTY_ERROR_ON_DEP_ONLY_INCOMPATIBLE = "error_on_dep_only_incompatible");
pagable::static_str!(
    PROPERTY_ERROR_ON_DEP_ONLY_INCOMPATIBLE_EXCLUDED = "error_on_dep_only_incompatible_excluded"
);

async fn check_error_on_incompatible_dep(
    ctx: &mut DiceComputations<'_>,
    target_label: &TargetLabel,
) -> buck2_error::Result<bool> {
    if check_target_enabled_for_config(
        ctx,
        target_label,
        SECTION_BUCK2,
        PROPERTY_ERROR_ON_DEP_ONLY_INCOMPATIBLE_EXCLUDED,
    )
    .await?
    {
        return Ok(false);
    }
    check_target_enabled_for_config(
        ctx,
        target_label,
        SECTION_BUCK2,
        PROPERTY_ERROR_ON_DEP_ONLY_INCOMPATIBLE,
    )
    .await
}

async fn check_target_enabled_for_config(
    ctx: &mut DiceComputations<'_>,
    target_label: &TargetLabel,
    section: StaticStr,
    property: StaticStr,
) -> buck2_error::Result<bool> {
    #[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("ConfigPatternCalculation({section}, {property})")]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct ConfigPatternCalculation {
        section: StaticStr,
        property: StaticStr,
    }

    #[async_trait]
    impl Key for ConfigPatternCalculation {
        type Value = buck2_error::Result<Vec<ParsedPattern<TargetPatternExtra>>>;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellation: &CancellationContext,
        ) -> Self::Value {
            let cell_resolver = ctx.get_cell_resolver().await?;
            let root_cell = cell_resolver.root_cell();
            let alias_resolver = ctx.get_cell_alias_resolver(root_cell).await?;
            let root_conf = ctx.get_legacy_root_config_on_dice().await?;
            let patterns: Vec<String> = root_conf
                .view(ctx)
                .parse_list(BuckconfigKeyRef {
                    section: &self.section,
                    property: &self.property,
                })?
                .unwrap_or_default();

            let mut result = Vec::new();
            for pattern in patterns {
                result.push(ParsedPattern::parse_precise(
                    pattern.trim(),
                    root_cell,
                    &cell_resolver,
                    &alias_resolver,
                )?);
            }
            Ok(result)
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| match (x, y) {
                (Ok(x), Ok(y)) => x == y,
                _ => false,
            })
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            OkPagableValueSerialize::<Self::Value>::new()
        }
    }

    let patterns = ctx
        .compute(&ConfigPatternCalculation {
            section: section.into(),
            property: property.into(),
        })
        .await?
        .as_ref()
        .duped_err()?;
    for pattern in patterns.iter() {
        if pattern.matches(target_label) {
            return Ok(true);
        }
    }

    Ok(false)
}

async fn get_dep_only_incompatible_custom_soft_error(
    ctx: &mut DiceComputations<'_>,
    target_label: &TargetLabel,
) -> buck2_error::Result<Option<Vec<ArcStr>>> {
    #[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct GetDepOnlyIncompatibleInfo;

    #[async_trait]
    impl Key for GetDepOnlyIncompatibleInfo {
        type Value = buck2_error::Result<Option<DepOnlyIncompatibleCustomSoftErrors>>;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellation: &CancellationContext,
        ) -> Self::Value {
            let cell_resolver = ctx.get_cell_resolver().await?;
            let root_cell = cell_resolver.root_cell();
            let alias_resolver = ctx.get_cell_alias_resolver(root_cell).await?;
            let root_conf = ctx.get_legacy_root_config_on_dice().await?;
            let Some(target) = root_conf.view(ctx).parse::<String>(BuckconfigKeyRef {
                section: "buck2",
                property: "dep_only_incompatible_info",
            })?
            else {
                return Ok(None);
            };
            let target =
                ProvidersLabel::parse(&target, root_cell.dupe(), &cell_resolver, &alias_resolver)?;
            let providers = ctx.get_configuration_analysis_result(&target).await?;
            let dep_only_incompatible_info = providers
                .provider_collection()
                .builtin_provider::<DepOnlyIncompatibleInfo>()?
                .unwrap();
            let result = dep_only_incompatible_info.custom_soft_errors(
                root_cell,
                &cell_resolver,
                &alias_resolver,
            )?;
            Ok(Some(result))
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| match (x, y) {
                (Ok(x), Ok(y)) => x == y,
                _ => false,
            })
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            OkPagableValueSerialize::<Self::Value>::new()
        }
    }

    let Some(custom_soft_errors) = ctx
        .compute(&GetDepOnlyIncompatibleInfo)
        .await?
        .as_ref()
        .duped_err()?
    else {
        return Ok(None);
    };
    let soft_error_categories: Vec<_> = custom_soft_errors
        .iter()
        .filter_map(|(soft_error_category, rollout_patterns)| {
            if rollout_patterns.matches(target_label) {
                Some(soft_error_category.dupe())
            } else {
                None
            }
        })
        .collect();
    Ok(Some(soft_error_categories))
}

#[allow(unused)]
fn _assert_compute_configured_target_node_no_transition_size() {
    const fn sz<F, T1, T2, T3, R>(_: &F) -> usize
    where
        F: FnOnce(T1, T2, T3) -> R,
    {
        std::mem::size_of::<R>()
    }

    const _: () = assert!(
        sz(&compute_configured_target_node_no_transition) <= 700,
        "compute_configured_target_node_no_transition size is larger than 700 bytes",
    );
}

#[allow(unused)]
fn _assert_compute_configured_forward_target_node_size() {
    const fn sz<F, T1, T2, T3, T4, R>(_: &F) -> usize
    where
        F: FnOnce(T1, T2, T3, T4) -> R,
    {
        std::mem::size_of::<R>()
    }

    const _: () = assert!(
        sz(&compute_configured_forward_target_node) <= 700,
        "compute_configured_forward_target_node size is larger than 700 bytes",
    );
}

#[cfg(test)]
mod tests {
    use buck2_hash::BuckMutInsertionMap;

    use super::CheckVisibility;
    use super::insert_dep_keeping_stricter_visibility;

    #[test]
    fn test_insert_dep_keeping_stricter_visibility() {
        let mut deps = BuckMutInsertionMap::default();
        deps.insert("plugin", CheckVisibility::No);
        deps.insert("tool", CheckVisibility::Yes);
        insert_dep_keeping_stricter_visibility(&mut deps, "plugin", CheckVisibility::Yes);
        insert_dep_keeping_stricter_visibility(&mut deps, "tool", CheckVisibility::No);
        insert_dep_keeping_stricter_visibility(&mut deps, "new", CheckVisibility::No);
        assert_eq!(deps.get("plugin"), Some(&CheckVisibility::Yes));
        assert_eq!(deps.get("tool"), Some(&CheckVisibility::Yes));
        assert_eq!(deps.get("new"), Some(&CheckVisibility::No));
    }
}
