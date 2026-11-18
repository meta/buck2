/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;
use std::mem;
use std::ops::ControlFlow;
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_build_api::actions::execute::dice_data::HasFallbackExecutorConfig;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_configuration_providers::constraint_value_info::ConstraintValueInfo;
use buck2_configuration_providers::execution_platform_registration_info::ExecutionPlatformRegistrationInfo;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::configuration::data::ExecPlatformMarker;
use buck2_core::configuration::pair::Configuration;
use buck2_core::configuration::pair::ConfigurationNoExec;
use buck2_core::execution_types::execution::ExecutionPlatform;
use buck2_core::execution_types::execution::ExecutionPlatformError;
use buck2_core::execution_types::execution::ExecutionPlatformIncompatibleReason;
use buck2_core::execution_types::execution::ExecutionPlatformResolutionPartial;
use buck2_core::execution_types::execution_platforms::ExecutionPlatformFallback;
use buck2_core::execution_types::execution_platforms::ExecutionPlatforms;
use buck2_core::execution_types::execution_platforms::ExecutionPlatformsData;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_core::target::target_configured_target_label::TargetConfiguredTargetLabel;
use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_node::attrs::configuration_context::AttrConfigurationContext;
use buck2_node::attrs::inspect_options::AttrInspectOptions;
use buck2_node::attrs::spec::AttributeId;
use buck2_node::attrs::spec::internal::EXEC_COMPATIBLE_WITH_ATTRIBUTE;
use buck2_node::cfg_constructor::CFG_CONSTRUCTOR_CALCULATION_IMPL;
use buck2_node::cfg_constructor::CfgConstructorModifiers;
use buck2_node::configuration::calculation::CellNameForConfigurationResolution;
use buck2_node::configuration::resolved::ConfigurationSettingKey;
use buck2_node::configuration::resolved::MatchedConfigurationSettingKeysWithCfg;
use buck2_node::execution::ExecutionPlatformCandidateOutcomes;
use buck2_node::execution::ExecutionPlatformDiagnosis;
use buck2_node::execution::GET_EXECUTION_PLATFORMS;
use buck2_node::execution::GetExecutionPlatforms;
use buck2_node::execution::GetExecutionPlatformsImpl;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured_frontend::ConfiguredTargetNodeCalculation;
use buck2_node::nodes::frontend::TargetGraphCalculation;
use buck2_node::nodes::unconfigured::TargetNodeRef;
use buck2_node::rule::RuleExecutionGroup;
use buck2_settings::ExecutionPlatforms as ExecutionPlatformsSettings;
use buck2_settings::dice::HasDiceBuckSettings;
use derive_more::Display;
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
use pagable::pagable_typetag;

use crate::configuration::compute_platform_cfgs;
use crate::configuration::get_matched_cfg_keys;
use crate::configuration::get_matched_cfg_keys_for_node;
use crate::nodes::CheckVisibility;
use crate::nodes::GatheredDeps;
use crate::nodes::LookingUpConfiguredNodeContext;
use crate::nodes::compute_configured_node_preamble;
use crate::nodes::gather_deps;
use crate::nodes::unspecified_attr_cfg_ctx;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum ExecutionPlatformComputationError {
    #[error("Can't find toolchain_dep execution platform using configuration `{0}`")]
    ToolchainDepMissingPlatform(ConfigurationData),
    #[error("Target `{0}` has a transition_dep, which is not permitted on a toolchain rule")]
    ToolchainTransitionDep(TargetLabel),
    #[error(
        "Expected `{0}` to provide a `ExecutionPlatformRegistrationInfo` as it's configured as the `build.execution_platforms` value."
    )]
    MissingExecutionPlatformRegistrationInfo(TargetLabel),
    #[error("Expected `{0}` (exec_marker_constraint) to provide a `ConstraintValueInfo` provider.")]
    MissingConstraintValueInfo(ProvidersLabel),
}

async fn legacy_execution_platform(
    ctx: &DiceComputations<'_>,
    cfg: &ConfigurationNoExec,
) -> ExecutionPlatform {
    ExecutionPlatform::legacy_execution_platform(
        ctx.get_fallback_executor_config().clone(),
        cfg.dupe(),
    )
}

pub async fn find_execution_platform_by_configuration(
    ctx: &mut DiceComputations<'_>,
    exec_cfg: &ConfigurationData,
    cfg: &ConfigurationData,
) -> buck2_error::Result<ExecutionPlatform> {
    match ctx.get_execution_platforms().await? {
        Some(platforms) if exec_cfg != &ConfigurationData::unbound_exec() => {
            for c in platforms.candidates() {
                if c.cfg() == exec_cfg {
                    return Ok(c.dupe());
                }
            }
            // The parent may have resolved to the fallback platform when every candidate was
            // skipped.
            if let ExecutionPlatformFallback::Platform(p) = platforms.fallback()
                && p.cfg() == exec_cfg
            {
                return Ok(p.dupe());
            }
            Err(buck2_error::Error::from(
                ExecutionPlatformComputationError::ToolchainDepMissingPlatform(exec_cfg.dupe()),
            ))
        }
        _ => Ok(legacy_execution_platform(ctx, &ConfigurationNoExec::new(cfg.dupe())).await),
    }
}

struct ExecutionPlatformConstraints {
    exec_deps: Arc<[TargetLabel]>,
    toolchain_deps: Arc<[TargetConfiguredTargetLabel]>,
    exec_compatible_with: Arc<[ConfigurationSettingKey]>,
}

/// Checks the most recently rejecting exec dep before all other exec deps.
///
/// Before a rejection is observed, every dep is checked in parallel. Afterwards, the most recent
/// rejector is checked as a serial preflight and the rest are checked in their original order, in
/// parallel, only if it succeeds. Since execution platforms are visited in configured order, each
/// rejection becomes the preferred dependency for the next candidate.
struct ExecDepChecker<'a> {
    exec_deps: &'a [TargetLabel],
    preferred_exec_dep_index: Option<usize>,
}

struct RemainingExecDeps<'a> {
    exec_deps: &'a [TargetLabel],
    preferred_exec_dep_index: Option<usize>,
    next_index: usize,
}

impl<'a> ExecDepChecker<'a> {
    fn new(exec_deps: &'a [TargetLabel]) -> Self {
        Self {
            exec_deps,
            preferred_exec_dep_index: None,
        }
    }

    fn preferred(&self) -> Option<(usize, &TargetLabel)> {
        let index = self.preferred_exec_dep_index?;
        Some((index, &self.exec_deps[index]))
    }

    fn remaining(&self) -> RemainingExecDeps<'_> {
        RemainingExecDeps {
            exec_deps: self.exec_deps,
            preferred_exec_dep_index: self.preferred_exec_dep_index,
            next_index: 0,
        }
    }

    fn record_rejection(&mut self, rejecting_exec_dep_index: usize) {
        debug_assert!(
            rejecting_exec_dep_index < self.exec_deps.len(),
            "rejecting exec dep index must be valid"
        );
        self.preferred_exec_dep_index = Some(rejecting_exec_dep_index);
    }
}

impl<'a> Iterator for RemainingExecDeps<'a> {
    type Item = (usize, &'a TargetLabel);

    fn next(&mut self) -> Option<Self::Item> {
        while self.next_index < self.exec_deps.len() {
            let index = self.next_index;
            self.next_index += 1;
            if Some(index) != self.preferred_exec_dep_index {
                return Some((index, &self.exec_deps[index]));
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let mut remaining = self.exec_deps.len() - self.next_index;
        if self
            .preferred_exec_dep_index
            .is_some_and(|index| index >= self.next_index)
        {
            remaining -= 1;
        }
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for RemainingExecDeps<'_> {}

impl ExecutionPlatformConstraints {
    fn new_constraints(
        exec_deps: Arc<[TargetLabel]>,
        toolchain_deps: Arc<[TargetConfiguredTargetLabel]>,
        exec_compatible_with: Arc<[ConfigurationSettingKey]>,
    ) -> Self {
        Self {
            exec_deps,
            toolchain_deps,
            exec_compatible_with,
        }
    }

    fn new(
        node: TargetNodeRef,
        gathered_deps: &GatheredDeps,
        cfg_ctx: &(dyn AttrConfigurationContext + Sync),
    ) -> buck2_error::Result<Self> {
        Self::new_for_deps(
            node,
            &gathered_deps.exec_deps,
            &gathered_deps.toolchain_deps,
            node.default_exec_compatible_with_attr(),
            cfg_ctx,
        )
    }

    /// The constraints that select the execution platform of the named execution group `group`:
    /// the target's `exec_compatible_with`, the group's `attrs.exec_compatible_with()` attribute,
    /// and the deps in the group.
    fn for_execution_group(
        node: TargetNodeRef,
        gathered_deps: &GatheredDeps,
        group: &RuleExecutionGroup,
        cfg_ctx: &(dyn AttrConfigurationContext + Sync),
    ) -> buck2_error::Result<Self> {
        match gathered_deps.execution_group_deps.get(&group.group) {
            Some(group_deps) => Self::new_for_deps(
                node,
                &group_deps.exec_deps,
                &group_deps.toolchain_deps,
                group.exec_compatible_with_attr,
                cfg_ctx,
            ),
            None => Self::new_for_deps(
                node,
                &BuckMutInsertionMap::default(),
                &BuckMutInsertionSet::default(),
                group.exec_compatible_with_attr,
                cfg_ctx,
            ),
        }
    }

    /// `extra_exec_compatible_with_attr` is a rule attribute with further constraints, as
    /// created by `attrs.exec_compatible_with()`.
    fn new_for_deps(
        node: TargetNodeRef,
        exec_deps: &BuckMutInsertionMap<ConfiguredProvidersLabel, CheckVisibility>,
        toolchain_deps: &BuckMutInsertionSet<TargetConfiguredTargetLabel>,
        extra_exec_compatible_with_attr: Option<AttributeId>,
        cfg_ctx: &(dyn AttrConfigurationContext + Sync),
    ) -> buck2_error::Result<Self> {
        let internal_attr =
            node.known_attr_or_none(EXEC_COMPATIBLE_WITH_ATTRIBUTE.id, AttrInspectOptions::All);
        let extra_attr = extra_exec_compatible_with_attr
            .and_then(|id| node.known_attr_or_none(id, AttrInspectOptions::All));
        let has_extra_attr = extra_attr.is_some();
        let mut exec_compatible_with = Vec::new();
        for a in internal_attr.into_iter().chain(extra_attr) {
            let name = a.name;
            let configured_attr = a
                .configure(cfg_ctx)
                .with_buck_error_context(|| {
                    format!("Error configuring attribute `{name}` to resolve execution platform")
                })
                .require_compatible()?;
            for label in ConfiguredTargetNode::attr_as_target_compatible_with(configured_attr.value)
            {
                exec_compatible_with
                    .push(label.with_buck_error_context(|| format!("attribute `{name}`"))?);
            }
        }
        if has_extra_attr {
            // Equal constraint sets share the resolution key whatever order the two attributes
            // list them in.
            exec_compatible_with.sort();
            exec_compatible_with.dedup();
        }
        let exec_compatible_with: Arc<[_]> = exec_compatible_with.into();

        Ok(Self::new_constraints(
            exec_deps
                .iter()
                .map(|c| c.0.target().unconfigured().dupe())
                .collect(),
            toolchain_deps.iter().map(|c| c.dupe()).collect(),
            exec_compatible_with,
        ))
    }

    /// Gets the compatible execution platforms for a give list of compatible_with constraints and execution deps.
    ///
    /// We do this as a sort of monolithic computation (rather than checking things one-by-one or separating
    /// compatible_with and exec deps) because we expect those values to be common across many nodes (for example,
    /// all c++ targets targeting a specific platform are likely to share compatible_with and exec_deps except
    /// for some rare exceptions). By having a monolithic key like `(Vec<TargetLabel>, Vec<TargetLabel>)` allows all
    /// those nodes to just have a single dice dep. This approach has the downside that it is less incremental, but
    /// we expect these things to change rarely.
    async fn one_for_cell(
        self,
        ctx: &mut DiceComputations<'_>,
        cell: CellNameForConfigurationResolution,
    ) -> buck2_error::Result<ExecutionPlatformResolutionPartial> {
        ctx.compute(&ExecutionPlatformResolutionKey {
            target_node_cell: cell,
            exec_compatible_with: self.exec_compatible_with,
            exec_deps: self.exec_deps,
            toolchain_deps: self.toolchain_deps,
        })
        .await?
        .dupe()
    }
}

#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display(
        "ToolchainExecutionPlatformCompatibilityKey({}, {})",
        target,
        exec_platform.id()
    )]
#[pagable_typetag(dice::DiceKeyDyn)]
pub(crate) struct ToolchainExecutionPlatformCompatibilityKey {
    target: TargetConfiguredTargetLabel,
    exec_platform: ExecutionPlatform,
}

impl ToolchainExecutionPlatformCompatibilityKey {
    async fn compute_impl(
        &self,
        ctx: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<Result<(), ExecutionPlatformIncompatibleReason>> {
        let node = ctx.get_target_node(self.target.unconfigured()).await?;
        if node.transition_deps().next().is_some() {
            // We could actually check this when defining the rule, but a bit of a corner
            // case, and much simpler to do so here.
            return Err(buck2_error::Error::from(
                ExecutionPlatformComputationError::ToolchainTransitionDep(
                    self.target.unconfigured().dupe(),
                ),
            ));
        }
        let cell_name = CellNameForConfigurationResolution(self.target.pkg().cell_name());
        let matched_cfg_keys =
            get_matched_cfg_keys_for_node(ctx, self.target.cfg(), cell_name, node.as_ref()).await?;
        let platform_cfgs = compute_platform_cfgs(ctx, node.as_ref()).await?;
        // We don't really need `resolved_transitions` here:
        // `Traversal` declared above ignores transitioned dependencies.
        // But we pass `resolved_transitions` here to prevent breakages in the future
        // if something here changes.
        let resolved_transitions = BuckFixedSortedMap::new();
        let cfg_ctx = unspecified_attr_cfg_ctx(
            self.target.inner(),
            matched_cfg_keys,
            &resolved_transitions,
            &platform_cfgs,
        );
        let (gathered_deps, errors_and_incompats) =
            gather_deps(&self.target, node.as_ref(), &cfg_ctx, ctx)
                .await
                .require_compatible()?;

        errors_and_incompats.finalize().require_compatible()?;

        let constraints =
            ExecutionPlatformConstraints::new(node.as_ref(), &gathered_deps, &cfg_ctx)?;

        ExecDepChecker::new(&constraints.exec_deps)
            .check_execution_platform(
                ctx,
                cell_name,
                &constraints.exec_compatible_with,
                &self.exec_platform,
                &constraints.toolchain_deps,
            )
            .await
    }
}

#[async_trait]
impl Key for ToolchainExecutionPlatformCompatibilityKey {
    type Value = buck2_error::Result<Result<(), ExecutionPlatformIncompatibleReason>>;
    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellation: &CancellationContext,
    ) -> Self::Value {
        Ok(LookingUpConfiguredNodeContext::add_context(
            self.compute_impl(ctx).await,
            self.target.inner().dupe(),
        )?)
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

async fn check_toolchain_execution_platform_compatibility(
    ctx: &mut DiceComputations<'_>,
    target: TargetConfiguredTargetLabel,
    exec_platform: ExecutionPlatform,
) -> buck2_error::Result<Result<(), ExecutionPlatformIncompatibleReason>> {
    ctx.compute(&ToolchainExecutionPlatformCompatibilityKey {
        target,
        exec_platform,
    })
    .await?
    .dupe()
}

pub(crate) async fn get_execution_platform_toolchain_dep(
    ctx: &mut DiceComputations<'_>,
    target_label: &TargetConfiguredTargetLabel,
    target_node: TargetNodeRef<'_>,
) -> buck2_error::Result<MaybeCompatible<ExecutionPlatformResolutionPartial>> {
    assert!(target_node.is_toolchain_rule());
    let target_cfg = target_label.cfg();
    let target_cell = target_node.label().pkg().cell_name();
    let matched_cfg_keys = get_matched_cfg_keys_for_node(
        ctx,
        target_cfg,
        CellNameForConfigurationResolution(target_cell),
        target_node,
    )
    .await?;
    if target_node.transition_deps().next().is_some() {
        Err(buck2_error::Error::from(
            ExecutionPlatformComputationError::ToolchainTransitionDep(
                target_label.unconfigured().dupe(),
            ),
        ))
    } else {
        let platform_cfgs = compute_platform_cfgs(ctx, target_node).await?;
        let resolved_transitions = BuckFixedSortedMap::new();
        let cfg_ctx = unspecified_attr_cfg_ctx(
            target_label.inner(),
            matched_cfg_keys,
            &resolved_transitions,
            &platform_cfgs,
        );
        let (gathered_deps, errors_and_incompats) =
            gather_deps(target_label, target_node, &cfg_ctx, ctx)
                .await
                .require_compatible()?;
        match errors_and_incompats.finalize() {
            ResultMaybeCompatible::Compatible(_) => {}
            ResultMaybeCompatible::Incompatible(reason) => {
                return Ok(MaybeCompatible::Incompatible(reason));
            }
            ResultMaybeCompatible::Err(e) => return Err(e),
        };
        Ok(MaybeCompatible::Compatible(
            resolve_execution_platform(
                ctx,
                target_node,
                matched_cfg_keys,
                &gathered_deps,
                &cfg_ctx,
            )
            .await?,
        ))
    }
}

pub(crate) async fn resolve_execution_platform(
    ctx: &mut DiceComputations<'_>,
    node: TargetNodeRef<'_>,
    matched_cfg_keys: &MatchedConfigurationSettingKeysWithCfg,
    gathered_deps: &GatheredDeps,
    cfg_ctx: &(dyn AttrConfigurationContext + Sync),
) -> buck2_error::Result<ExecutionPlatformResolutionPartial> {
    resolve_execution_platform_with_constraints(ctx, node, matched_cfg_keys, || {
        ExecutionPlatformConstraints::new(node, gathered_deps, cfg_ctx)
    })
    .await
}

/// Resolves the execution platform of a named execution group from the deps in that group.
pub(crate) async fn resolve_execution_group_platform(
    ctx: &mut DiceComputations<'_>,
    node: TargetNodeRef<'_>,
    matched_cfg_keys: &MatchedConfigurationSettingKeysWithCfg,
    gathered_deps: &GatheredDeps,
    group: &RuleExecutionGroup,
    cfg_ctx: &(dyn AttrConfigurationContext + Sync),
) -> buck2_error::Result<ExecutionPlatformResolutionPartial> {
    resolve_execution_platform_with_constraints(ctx, node, matched_cfg_keys, || {
        ExecutionPlatformConstraints::for_execution_group(node, gathered_deps, group, cfg_ctx)
    })
    .await
}

async fn resolve_execution_platform_with_constraints(
    ctx: &mut DiceComputations<'_>,
    node: TargetNodeRef<'_>,
    matched_cfg_keys: &MatchedConfigurationSettingKeysWithCfg,
    constraints: impl FnOnce() -> buck2_error::Result<ExecutionPlatformConstraints>,
) -> buck2_error::Result<ExecutionPlatformResolutionPartial> {
    // If no execution platforms are configured, we fall back to the legacy execution
    // platform behavior. We currently only support legacy execution platforms. That behavior is that there is a
    // single executor config (the fallback config) and the execution platform is in the same
    // configuration as the target.
    // The non-none case will be handled when we invoke the resolve_execution_platform() on ctx below, the none
    // case can't be handled there because we don't pass the full configuration into it.
    if ctx.get_execution_platforms().await?.is_none() {
        return Ok(ExecutionPlatformResolutionPartial::new(
            Some(legacy_execution_platform(ctx, matched_cfg_keys.cfg()).await),
            Vec::new(),
        ));
    };

    constraints()?
        .one_for_cell(
            ctx,
            CellNameForConfigurationResolution(node.label().pkg().cell_name()),
        )
        .await
}

/// Returns the configured [ExecutionPlatforms] or None if `build.execution_platforms` is not configured.
async fn compute_execution_platforms(
    ctx: &mut DiceComputations<'_>,
) -> buck2_error::Result<Option<ExecutionPlatforms>> {
    let cells = ctx.get_cell_resolver().await?;
    let cell_alias_resolver = ctx.get_cell_alias_resolver(cells.root_cell()).await?;

    let root_config = ctx.get_legacy_root_config_on_dice().await?;
    let execution_platforms_target = ctx
        .dice_settings::<ExecutionPlatformsSettings>()
        .await?
        .target(&mut root_config.view(ctx))?;

    let execution_platforms_target = match execution_platforms_target {
        Some(v) => TargetLabel::parse(&v, cells.root_cell(), &cells, &cell_alias_resolver)?,
        None => {
            return Ok(None);
        }
    };

    let providers = ctx
        // Execution platform won't be supplied as a subtarget
        .get_configuration_analysis_result(&ProvidersLabel::default_for(
            execution_platforms_target.dupe(),
        ))
        .await?;

    let registration_info = providers
        .builtin_provider_value::<ExecutionPlatformRegistrationInfo>()?
        .ok_or_else(|| {
            ExecutionPlatformComputationError::MissingExecutionPlatformRegistrationInfo(
                execution_platforms_target.dupe(),
            )
        })?;

    let marker_str = registration_info
        .as_ref()
        .value()
        .as_ref()
        .exec_marker_constraint()
        .map(str::to_owned);

    // Resolve the exec_marker_constraint if set
    let marker_constraint = if let Some(marker_str) = marker_str {
        let marker_label =
            ProvidersLabel::parse(&marker_str, cells.root_cell(), &cells, &cell_alias_resolver)?;
        let marker_providers = ctx.get_configuration_analysis_result(&marker_label).await?;
        let constraint_value_info = marker_providers
            .provider_collection()
            .builtin_provider::<ConstraintValueInfo>()?
            .ok_or_else(|| {
                buck2_error::Error::from(
                    ExecutionPlatformComputationError::MissingConstraintValueInfo(
                        marker_label.dupe(),
                    ),
                )
            })?;

        let (key, value) = constraint_value_info.to_constraint_key_value();
        Some(Arc::new(ExecPlatformMarker::new(key, value)))
    } else {
        None
    };

    let root_config = ctx.get_legacy_root_config_on_dice().await?;
    let preserve_exec_platform_marker = root_config
        .view(ctx)
        .parse::<bool>(BuckconfigKeyRef {
            section: "buck2",
            property: "preserve_exec_platform_marker",
        })?
        .unwrap_or(false);

    let registration_info = registration_info.as_ref().value().as_ref();
    let mut platforms = Vec::new();
    for platform in registration_info.platforms()? {
        platforms.push(platform.to_execution_platform_with_marker(
            marker_constraint.dupe(),
            preserve_exec_platform_marker,
        )?);
    }
    Ok(Some(Arc::new(ExecutionPlatformsData::new(
        execution_platforms_target,
        platforms,
        registration_info.fallback()?,
    ))))
}

/// Computes the configuration an exec dep is built in: the execution platform's configuration
/// with the exec dep's modifiers (package-level + target-level + exec platform) applied.
///
/// This is a DICE key because execution platform resolution asks the same question for the same
/// (exec dep, candidate platform) pair once per `ExecutionPlatformResolutionKey` that shares the
/// exec dep — the target node fetch and cfg constructor invocation only need to happen once.
/// Both fields are interned, so key hashing and comparison are cheap.
#[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("ExecDepCfg({}, {})", exec_dep, execution_platform_cfg)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct ExecDepCfgKey {
    exec_dep: TargetLabel,
    execution_platform_cfg: ConfigurationData,
}

#[async_trait]
impl Key for ExecDepCfgKey {
    type Value = buck2_error::Result<ConfigurationData>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellation: &CancellationContext,
    ) -> Self::Value {
        let (node, super_package) = ctx
            .get_target_node_with_super_package(&self.exec_dep)
            .await?;
        CFG_CONSTRUCTOR_CALCULATION_IMPL
            .get()?
            .eval_cfg_constructor(
                ctx,
                node.as_ref(),
                &super_package,
                self.execution_platform_cfg.dupe(),
                CfgConstructorModifiers::ExecPlatform,
                node.rule_type(),
            )
            .await
            .with_buck_error_context(|| {
                format!(
                    "Resolving modifiers for exec dep target `{}`",
                    self.exec_dep
                )
            })
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

/// Configure an exec_dep with modifiers applied from the execution platform.
/// This function is used in two places:
/// 1. During execution platform selection (check_execution_platform) - to check target_compatible_with
/// 2. During dependency graph construction (nodes.rs) - to create the final configured nodes
pub(crate) async fn configure_exec_dep_with_modifiers<'d>(
    ctx: &mut DiceComputations<'d>,
    exec_dep: &TargetLabel,
    execution_platform_cfg: &ConfigurationData,
) -> ResultMaybeCompatible<&'d ConfiguredTargetNode> {
    if !execution_platform_cfg.is_bound() {
        // No valid execution platform was resolved (e.g., all candidates were incompatible
        // and fallback is UseUnspecifiedExec). We cannot apply modifiers without a bound
        // configuration, so configure the exec_dep directly. This will likely fail later
        // when resolving configuration settings.
        let cfg_pair = Configuration::new(execution_platform_cfg.dupe(), None);
        return ctx
            .get_internal_configured_target_node(&exec_dep.configure_pair(cfg_pair))
            .await;
    }

    let cfg_config = ctx
        .compute(&ExecDepCfgKey {
            exec_dep: exec_dep.dupe(),
            execution_platform_cfg: execution_platform_cfg.dupe(),
        })
        .await?
        .dupe()?;

    // Create configuration pair with modifiers applied
    let cfg_pair = Configuration::new(cfg_config, None);

    // Configure exec_dep with modifiers applied
    ctx.get_internal_configured_target_node(&exec_dep.configure_pair(cfg_pair))
        .await
}

async fn check_exec_dep_for_platform(
    ctx: &mut DiceComputations<'_>,
    dep: &TargetLabel,
    exec_platform: &ExecutionPlatform,
) -> ResultMaybeCompatible<()> {
    let cfg = exec_platform.cfg();
    configure_exec_dep_with_modifiers(ctx, dep, cfg)
        .await
        .map(|_| ())
        .map_err(|e| {
            e.context(format!(
                "Error checking compatibility of `{}` with `{}`",
                dep, cfg
            ))
        })
}

impl ExecDepChecker<'_> {
    /// Check if a particular execution platform is compatible with the constraints or not.
    /// Return either Ok/Ok if it is, or a reason if not.
    async fn check_execution_platform(
        &mut self,
        ctx: &mut DiceComputations<'_>,
        target_node_cell: CellNameForConfigurationResolution,
        exec_compatible_with: &[ConfigurationSettingKey],
        exec_platform: &ExecutionPlatform,
        toolchain_deps: &[TargetConfiguredTargetLabel],
    ) -> buck2_error::Result<Result<(), ExecutionPlatformIncompatibleReason>> {
        let matched_cfg_keys = get_matched_cfg_keys(
            ctx,
            exec_platform.cfg(),
            target_node_cell,
            exec_compatible_with,
        )
        .await?;

        // Then check if the platform satisfies compatible_with
        for constraint in exec_compatible_with {
            if matched_cfg_keys
                .settings()
                .setting_matches(constraint)
                .is_none()
            {
                return Ok(Err(
                    ExecutionPlatformIncompatibleReason::ConstraintNotSatisfied(
                        constraint.dupe().0,
                    ),
                ));
            }
        }

        // Then check that all exec_deps are compatible with the platform. Retain an error only in
        // case no dependency proves the platform incompatible.
        let mut error = None;
        if let Some((_, dep)) = self.preferred() {
            match check_exec_dep_for_platform(ctx, dep, exec_platform).await {
                ResultMaybeCompatible::Compatible(()) => {}
                ResultMaybeCompatible::Incompatible(reason) => {
                    return Ok(Err(
                        ExecutionPlatformIncompatibleReason::ExecutionDependencyIncompatible(
                            reason,
                        ),
                    ));
                }
                ResultMaybeCompatible::Err(e) => error = Some(e),
            }
        }

        let dep_results = ctx
            .compute_join(self.remaining(), async |ctx, (index, dep)| {
                (
                    index,
                    check_exec_dep_for_platform(ctx, dep, exec_platform).await,
                )
            })
            .await;

        for (index, result) in dep_results {
            match result {
                ResultMaybeCompatible::Compatible(..) => (),
                ResultMaybeCompatible::Incompatible(reason) => {
                    self.record_rejection(index);
                    return Ok(Err(
                        ExecutionPlatformIncompatibleReason::ExecutionDependencyIncompatible(
                            reason,
                        ),
                    ));
                }
                ResultMaybeCompatible::Err(e) => error = Some(e),
            };
        }

        for result in ctx
            .compute_join(toolchain_deps.iter(), async |ctx, dep| {
                check_toolchain_execution_platform_compatibility(
                    ctx,
                    dep.dupe(),
                    exec_platform.dupe(),
                )
                .await
            })
            .await
        {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(reason)) => {
                    return Ok(Err(reason));
                }
                Err(e) => error = Some(e),
            }
        }

        if let Some(e) = error {
            return Err(e);
        }

        Ok(Ok(()))
    }
}

async fn get_execution_platforms_enabled(
    ctx: &mut DiceComputations<'_>,
) -> buck2_error::Result<ExecutionPlatforms> {
    match ctx.get_execution_platforms().await? {
        Some(platforms) => Ok(platforms.dupe()),
        None => Err(internal_error!("Execution platforms are not enabled")),
    }
}

/// Compile-time mode for the execution platform candidate traversal: modes share the
/// traversal and differ in accumulation and stop semantics.
pub(crate) trait ResolutionMode {
    type Accumulator: Default;
    type Output;

    /// One candidate's outcome. `Break` ends the traversal with the final output; `Continue`
    /// hands the accumulator back for the next candidate.
    fn visit(
        accumulator: Self::Accumulator,
        platform: &ExecutionPlatform,
        outcome: Result<(), ExecutionPlatformIncompatibleReason>,
    ) -> ControlFlow<Self::Output, Self::Accumulator>;

    /// Every candidate was visited without a `Break`.
    fn finish(
        accumulator: Self::Accumulator,
        fallback: &ExecutionPlatformFallback,
    ) -> buck2_error::Result<Self::Output>;
}

/// Real resolution: stop at the first compatible platform, accumulating the skipped ones.
pub(crate) struct Resolve;

impl ResolutionMode for Resolve {
    type Accumulator = Vec<(String, ExecutionPlatformIncompatibleReason)>;
    type Output = ExecutionPlatformResolutionPartial;

    fn visit(
        mut skipped: Self::Accumulator,
        platform: &ExecutionPlatform,
        outcome: Result<(), ExecutionPlatformIncompatibleReason>,
    ) -> ControlFlow<Self::Output, Self::Accumulator> {
        match outcome {
            Ok(()) => ControlFlow::Break(ExecutionPlatformResolutionPartial::new(
                Some(platform.dupe()),
                skipped,
            )),
            Err(reason) => {
                skipped.push((platform.id(), reason));
                ControlFlow::Continue(skipped)
            }
        }
    }

    fn finish(
        skipped: Self::Accumulator,
        fallback: &ExecutionPlatformFallback,
    ) -> buck2_error::Result<Self::Output> {
        match fallback {
            ExecutionPlatformFallback::UseUnspecifiedExec => {
                Ok(ExecutionPlatformResolutionPartial::new(None, skipped))
            }
            ExecutionPlatformFallback::Error => {
                Err(ExecutionPlatformError::NoCompatiblePlatform(skipped.len()).into())
            }
            ExecutionPlatformFallback::Platform(platform) => Ok(
                ExecutionPlatformResolutionPartial::new(Some(platform.dupe()), skipped),
            ),
        }
    }
}

pub(crate) async fn resolve_execution_platform_candidates<M: ResolutionMode>(
    ctx: &mut DiceComputations<'_>,
    target_node_cell: CellNameForConfigurationResolution,
    exec_compatible_with: &[ConfigurationSettingKey],
    exec_deps: &[TargetLabel],
    toolchain_deps: &[TargetConfiguredTargetLabel],
) -> buck2_error::Result<M::Output> {
    let mut exec_dep_checker = ExecDepChecker::new(exec_deps);
    let execution_platforms = get_execution_platforms_enabled(ctx).await?;
    let mut accumulator = M::Accumulator::default();
    for exec_platform in execution_platforms.candidates() {
        let outcome = exec_dep_checker
            .check_execution_platform(
                ctx,
                target_node_cell,
                exec_compatible_with,
                exec_platform,
                toolchain_deps,
            )
            .await?;
        accumulator = match M::visit(accumulator, exec_platform, outcome) {
            ControlFlow::Break(output) => return Ok(output),
            ControlFlow::Continue(accumulator) => accumulator,
        };
    }
    M::finish(accumulator, execution_platforms.fallback())
}

#[derive(Clone, Dupe, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[pagable_typetag(dice::DiceKeyDyn)]
pub(crate) struct ExecutionPlatformResolutionKey {
    /// Determining a compatible execution platform requires checking the target and toolchain's
    /// exec_compatible_with. This in turn requires a ResolvedConfiguration, which resolves the
    /// buckconfig-related config_setting values based on the cell of the target the configuration
    /// is being resolved for.
    target_node_cell: CellNameForConfigurationResolution,
    exec_compatible_with: Arc<[ConfigurationSettingKey]>,
    exec_deps: Arc<[TargetLabel]>,
    toolchain_deps: Arc<[TargetConfiguredTargetLabel]>,
}

impl Display for ExecutionPlatformResolutionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Resolving execution platform: cell:{}",
            self.target_node_cell
        )?;

        if !self.exec_compatible_with.is_empty() {
            write!(
                f,
                ", exec_compatible_with=[{}]",
                self.exec_compatible_with.iter().join(", ")
            )?
        }

        if !self.exec_deps.is_empty() {
            write!(f, ", exec_deps=[{}]", self.exec_deps.iter().join(", "))?
        }

        if !self.toolchain_deps.is_empty() {
            write!(
                f,
                ", toolchain_deps=[{}]",
                self.toolchain_deps.iter().join(", ")
            )?;
        }

        Ok(())
    }
}

#[async_trait]
impl Key for ExecutionPlatformResolutionKey {
    type Value = buck2_error::Result<ExecutionPlatformResolutionPartial>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellation: &CancellationContext,
    ) -> Self::Value {
        resolve_execution_platform_candidates::<Resolve>(
            ctx,
            self.target_node_cell,
            &self.exec_compatible_with,
            &self.exec_deps,
            &self.toolchain_deps,
        )
        .await
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

#[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("ExecutionPlatforms")]
#[pagable_typetag(dice::DiceKeyDyn)]
pub struct ExecutionPlatformsKey;

#[async_trait]
impl Key for ExecutionPlatformsKey {
    type Value = buck2_error::Result<Option<ExecutionPlatforms>>;
    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellation: &CancellationContext,
    ) -> Self::Value {
        compute_execution_platforms(ctx).await
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_, _| {
            // TODO(cjhopman) should these be comparable for caching
            false
        })
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}

/// Audit diagnosis: visit every candidate and report each outcome.
struct Diagnose;

impl ResolutionMode for Diagnose {
    type Accumulator = ExecutionPlatformCandidateOutcomes;
    type Output = ExecutionPlatformCandidateOutcomes;

    fn visit(
        mut accumulator: Self::Accumulator,
        platform: &ExecutionPlatform,
        outcome: Result<(), ExecutionPlatformIncompatibleReason>,
    ) -> ControlFlow<Self::Output, Self::Accumulator> {
        accumulator.push((platform.id(), outcome));
        ControlFlow::Continue(accumulator)
    }

    fn finish(
        accumulator: Self::Accumulator,
        _fallback: &ExecutionPlatformFallback,
    ) -> buck2_error::Result<Self::Output> {
        Ok(accumulator)
    }
}

/// See `GetExecutionPlatformsImpl::diagnose_execution_platform_resolution`. Runs the same
/// preamble and candidate traversal as real resolution, in `Diagnose` mode so the report covers
/// every candidate instead of stopping at the first compatible one.
///
/// Mirrors the regular-target path only: toolchain rules resolve via
/// `ToolchainExecutionPlatformCompatibilityKey`, whose pre-resolution steps (e.g. the
/// transition-dep rejection) are not replayed here.
async fn diagnose_execution_platform_resolution(
    ctx: &mut DiceComputations<'_>,
    target: &TargetConfiguredTargetLabel,
) -> buck2_error::Result<ExecutionPlatformDiagnosis> {
    let node = ctx.get_target_node(target.unconfigured()).await?;
    let mut preamble = compute_configured_node_preamble(ctx, target.inner(), &node)
        .await
        .require_compatible()?;
    mem::take(&mut preamble.errors_and_incompats)
        .finalize()
        .require_compatible()?;

    let cfg_ctx = preamble.attr_cfg_ctx(target.inner());
    let cell = CellNameForConfigurationResolution(target.pkg().cell_name());
    let default =
        match ExecutionPlatformConstraints::new(node.as_ref(), &preamble.gathered_deps, &cfg_ctx) {
            Ok(constraints) => diagnose_constraints(ctx, cell, &constraints).await,
            Err(e) => Err(e),
        };

    let gathered_deps = &preamble.gathered_deps;
    let cfg_ctx = &cfg_ctx;
    let execution_groups = ctx
        .compute_join(node.execution_groups(), async |ctx, group| {
            let outcomes = match ExecutionPlatformConstraints::for_execution_group(
                node.as_ref(),
                gathered_deps,
                group,
                cfg_ctx,
            ) {
                Ok(constraints) => diagnose_constraints(ctx, cell, &constraints).await,
                Err(e) => Err(e),
            };
            (group.group.dupe(), outcomes)
        })
        .await;

    Ok(ExecutionPlatformDiagnosis {
        default,
        execution_groups,
    })
}

async fn diagnose_constraints(
    ctx: &mut DiceComputations<'_>,
    cell: CellNameForConfigurationResolution,
    constraints: &ExecutionPlatformConstraints,
) -> buck2_error::Result<ExecutionPlatformCandidateOutcomes> {
    resolve_execution_platform_candidates::<Diagnose>(
        ctx,
        cell,
        &constraints.exec_compatible_with,
        &constraints.exec_deps,
        &constraints.toolchain_deps,
    )
    .await
}

struct GetExecutionPlatformsInstance;

#[async_trait]
impl GetExecutionPlatformsImpl for GetExecutionPlatformsInstance {
    fn get_execution_platforms_impl<'a, 'd>(
        &self,
        ctx: &'a mut DiceComputations<'d>,
    ) -> BoxFuture<'a, buck2_error::Result<&'d Option<ExecutionPlatforms>>>
    where
        'd: 'a,
    {
        async move {
            ctx.compute(&ExecutionPlatformsKey)
                .await?
                .as_ref()
                .duped_err()
        }
        .boxed()
    }

    async fn execution_platform_resolution_one_for_cell(
        &self,
        dice: &mut DiceComputations<'_>,
        exec_deps: Arc<[TargetLabel]>,
        toolchain_deps: Arc<[TargetConfiguredTargetLabel]>,
        exec_compatible_with: Arc<[ConfigurationSettingKey]>,
        cell: CellNameForConfigurationResolution,
    ) -> buck2_error::Result<ExecutionPlatformResolutionPartial> {
        ExecutionPlatformConstraints::new_constraints(
            exec_deps,
            toolchain_deps,
            exec_compatible_with,
        )
        .one_for_cell(dice, cell)
        .await
    }

    async fn diagnose_execution_platform_resolution(
        &self,
        dice: &mut DiceComputations<'_>,
        target: &TargetConfiguredTargetLabel,
    ) -> buck2_error::Result<ExecutionPlatformDiagnosis> {
        diagnose_execution_platform_resolution(dice, target).await
    }
}

pub(crate) fn init_get_execution_platforms() {
    GET_EXECUTION_PLATFORMS.init(&GetExecutionPlatformsInstance);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_dep_checker_only_prefers_the_latest_rejector() {
        let deps = [
            TargetLabel::testing_parse("root//:first"),
            TargetLabel::testing_parse("root//:second"),
            TargetLabel::testing_parse("root//:third"),
        ];
        let mut checker = ExecDepChecker::new(&deps);

        assert_eq!(checker.remaining().len(), 3);
        assert_eq!(
            checker
                .remaining()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2],
            "All dependencies should be checked together before a rejector is known",
        );

        checker.record_rejection(1);

        assert_eq!(checker.remaining().len(), 2);
        assert_eq!(
            checker.preferred().map(|(index, _)| index),
            Some(1),
            "The rejecting dependency should be checked first for the next platform",
        );
        assert_eq!(
            checker
                .remaining()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            vec![0, 2],
            "The preferred dependency should not also be checked in the parallel remainder",
        );

        checker.record_rejection(2);

        assert_eq!(
            checker.preferred().map(|(index, _)| index),
            Some(2),
            "A newer rejecting dependency should replace the previous preference",
        );
        assert_eq!(
            checker
                .remaining()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            vec![0, 1],
            "Dependencies other than the preferred rejector should retain attribute order",
        );
    }

    #[test]
    fn exec_dep_checker_keeps_its_preference_until_another_rejection() {
        let deps = [
            TargetLabel::testing_parse("root//:first"),
            TargetLabel::testing_parse("root//:second"),
        ];
        let mut checker = ExecDepChecker::new(&deps);

        checker.record_rejection(1);

        assert_eq!(checker.preferred().map(|(index, _)| index), Some(1));
        assert_eq!(
            checker
                .remaining()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            vec![0],
            "The preferred dependency should remain until another exec dependency rejects",
        );
    }
}
