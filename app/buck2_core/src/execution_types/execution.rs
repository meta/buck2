/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use allocative::Allocative;
use buck2_hash::BuckFixedSortedMap;
use dupe::Dupe;
use pagable::Pagable;

use crate::configuration::compatibility::IncompatiblePlatformReason;
use crate::configuration::compatibility::IncompatiblePlatformReasonCause;
use crate::configuration::data::ConfigurationData;
use crate::configuration::pair::ConfigurationNoExec;
use crate::execution_types::execution_group::ExecutionGroup;
use crate::execution_types::execution_group::NamedExecutionGroup;
use crate::execution_types::executor_config::CommandExecutorConfig;
use crate::provider::label::ProvidersLabel;
use crate::target::configured_target_label::ConfiguredTargetLabel;
use crate::target::label::label::TargetLabel;

/// An execution platform is used for the execution deps of a target, those dependencies that
/// need to be invoked as part of a build action or otherwise need to be configured against the
/// platform that execution happens on (as opposed to the normal case where things are configured
/// against the target platform).
///
/// It consists of that platform `Configuration` and configuration of how to do the execution
/// (e.g. local, remote, etc.).
#[derive(Debug, Eq, PartialEq, Hash, Dupe, Clone, Allocative, Pagable)]
pub struct ExecutionPlatform(Arc<ExecutionPlatformData>);

#[derive(Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
pub enum ExecutionPlatformData {
    /// A user-defined platform.
    Platform {
        target: TargetLabel,
        cfg: ConfigurationNoExec,
        executor_config: Arc<CommandExecutorConfig>,
    },
    /// When users haven't configured execution platforms, we will use old legacy behavior where
    /// execution deps inherit the target platform configuration.
    LegacyExecutionPlatform {
        executor_config: Arc<CommandExecutorConfig>,
        cfg: ConfigurationNoExec,
    },
}

impl ExecutionPlatform {
    pub fn platform(
        target: TargetLabel,
        cfg: ConfigurationData,
        executor_config: Arc<CommandExecutorConfig>,
    ) -> Self {
        Self(Arc::new(ExecutionPlatformData::Platform {
            target,
            cfg: ConfigurationNoExec::new(cfg),
            executor_config,
        }))
    }

    pub fn legacy_execution_platform(
        executor_config: Arc<CommandExecutorConfig>,
        cfg: ConfigurationNoExec,
    ) -> Self {
        Self(Arc::new(ExecutionPlatformData::LegacyExecutionPlatform {
            executor_config,
            cfg,
        }))
    }

    #[inline]
    pub fn cfg_pair_no_exec(&self) -> &ConfigurationNoExec {
        match &*self.0 {
            ExecutionPlatformData::Platform { cfg, .. } => cfg,
            ExecutionPlatformData::LegacyExecutionPlatform { cfg, .. } => cfg,
        }
    }

    #[inline]
    pub fn cfg(&self) -> &ConfigurationData {
        self.cfg_pair_no_exec().cfg()
    }

    pub fn id(&self) -> String {
        match &*self.0 {
            ExecutionPlatformData::Platform { target, .. } => target.to_string(),
            ExecutionPlatformData::LegacyExecutionPlatform { .. } => {
                "<legacy_global_exec_platform>".to_owned()
            }
        }
    }

    pub fn executor_config(&self) -> &Arc<CommandExecutorConfig> {
        match &*self.0 {
            ExecutionPlatformData::Platform {
                executor_config, ..
            } => executor_config,
            ExecutionPlatformData::LegacyExecutionPlatform {
                executor_config, ..
            } => executor_config,
        }
    }
}

#[derive(Clone, Dupe, Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
pub enum ExecutionPlatformIncompatibleReason {
    ConstraintNotSatisfied(ProvidersLabel),
    ExecutionDependencyIncompatible(Arc<IncompatiblePlatformReason>),
}

impl ExecutionPlatformIncompatibleReason {
    pub fn into_incompatible_platform_reason(
        self,
        target: ConfiguredTargetLabel,
    ) -> IncompatiblePlatformReason {
        match self {
            Self::ConstraintNotSatisfied(unsatisfied_config) => IncompatiblePlatformReason {
                target,
                cause: IncompatiblePlatformReasonCause::UnsatisfiedConfig(unsatisfied_config),
            },
            Self::ExecutionDependencyIncompatible(previous) => IncompatiblePlatformReason {
                target,
                cause: IncompatiblePlatformReasonCause::Dependency(previous),
            },
        }
    }
}

impl std::fmt::Display for ExecutionPlatformIncompatibleReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionPlatformIncompatibleReason::ConstraintNotSatisfied(v) => write!(
                f,
                "exec_compatible_with requires `{v}` but it was not satisfied"
            ),
            ExecutionPlatformIncompatibleReason::ExecutionDependencyIncompatible(v) => v.fmt(f),
        }
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
pub enum ExecutionPlatformError {
    // Deliberately carries only the candidate count, not the skipped platforms: there can be
    // thousands of candidates, the message is materialized eagerly on conversion to
    // `buck2_error::Error`, and failed resolutions are routine in large graphs where
    // incompatible subtrees are expected — formatting every entry was a measurable share of
    // cold configuration time. The audit command reconstructs the reasons on demand.
    #[error(
        "No compatible execution platform; {0} candidates were skipped. Run `buck2 audit \
         execution-platform-resolution` on the target, with `--reuse-current-config` and a \
         `--target-universe` matching the failing command, to see why each was skipped"
    )]
    NoCompatiblePlatform(usize),
    #[error(
        "Execution platform resolution was not performed: this target was configured in the \
         unspecified execution platform state, which is used while gathering dependencies \
         (before resolution runs) and for nodes that are never resolved against an execution \
         platform"
    )]
    ResolutionNotPerformed,
    #[error(
        "Execution group `{group}` is not declared by this rule. Declared groups: [{declared}]{message}"
    )]
    ExecutionGroupNotDeclared {
        group: String,
        declared: String,
        message: String,
    },
}

/// Base data shared between Partial and Resolved states.
#[derive(Debug, Eq, PartialEq, Hash, Clone, Allocative, Pagable)]
pub(crate) struct ExecutionPlatformResolutionBase {
    /// The platform if resolution found one. If all platforms are skipped, this will be `None`.
    platform: Option<ExecutionPlatform>,
    /// The human readable names of skipped platforms and the reason they were skipped.
    skipped_platforms: Arc<Vec<(String, ExecutionPlatformIncompatibleReason)>>,
    /// One entry per named execution group the rule declares, in declaration order; `None` when
    /// it declares none. Nodes without named groups share the cached base and store nothing for
    /// groups. A node with groups stores its own list; only each group's inner resolution is
    /// shared through the resolution cache.
    execution_groups: Option<Arc<Vec<ExecutionGroupResolution>>>,
    /// Whether the rule may use the default execution group: false only for rules that declare
    /// named execution groups but not `DefaultExecGroup`.
    default_execution_group_declared: bool,
}

impl ExecutionPlatformResolutionBase {
    fn new(
        platform: Option<ExecutionPlatform>,
        skipped: Vec<(String, ExecutionPlatformIncompatibleReason)>,
    ) -> Arc<Self> {
        Arc::new(Self {
            platform,
            skipped_platforms: Arc::new(skipped),
            execution_groups: None,
            default_execution_group_declared: true,
        })
    }

    #[inline]
    fn cfg(&self) -> ConfigurationNoExec {
        match &self.platform {
            Some(v) => v.cfg_pair_no_exec().dupe(),
            None => ConfigurationNoExec::unspecified_exec().dupe(),
        }
    }

    fn platform(&self) -> buck2_error::Result<&ExecutionPlatform> {
        match &self.platform {
            Some(v) => Ok(v),
            None => Err(
                ExecutionPlatformError::NoCompatiblePlatform(self.skipped_platforms.len()).into(),
            ),
        }
    }

    fn skipped(&self) -> &[(String, ExecutionPlatformIncompatibleReason)] {
        &self.skipped_platforms
    }

    fn executor_config(&self) -> buck2_error::Result<&Arc<CommandExecutorConfig>> {
        Ok(self.platform()?.executor_config())
    }

    fn execution_groups(&self) -> &[ExecutionGroupResolution] {
        self.execution_groups
            .as_deref()
            .map_or(&[], |g| g.as_slice())
    }

    fn execution_group(
        &self,
        named: &NamedExecutionGroup,
    ) -> buck2_error::Result<&ExecutionGroupResolution> {
        let groups = self.execution_groups();
        groups.iter().find(|g| &g.group == named).ok_or_else(|| {
            ExecutionPlatformError::ExecutionGroupNotDeclared {
                group: named.name().to_owned(),
                declared: groups
                    .iter()
                    .map(|g| format!("`{}`", g.group))
                    .collect::<Vec<_>>()
                    .join(", "),
                message: match named.same_name_in(groups.iter().map(|g| &g.group)) {
                    Some(_) => format!(
                        ". The declared `{}` is a different group with the same name: each \
                         `exec_group()` call creates its own group",
                        named.name()
                    ),
                    None => String::new(),
                },
            }
            .into()
        })
    }
}

/// Transient result of execution platform resolution, before modifiers are applied.
///
/// Contains the first compatible execution platform from the candidates list.
/// Call `finalize(exec_dep_cfgs)` to produce the complete `ExecutionPlatformResolution`.
#[derive(Debug, Eq, PartialEq, Hash, Clone, Dupe, Allocative, Pagable)]
pub struct ExecutionPlatformResolutionPartial(Arc<ExecutionPlatformResolutionBase>);

impl ExecutionPlatformResolutionPartial {
    /// Create a new partial resolution with the given platform and skipped platforms.
    pub fn new(
        platform: Option<ExecutionPlatform>,
        skipped: Vec<(String, ExecutionPlatformIncompatibleReason)>,
    ) -> Self {
        Self(ExecutionPlatformResolutionBase::new(platform, skipped))
    }

    /// Get the execution platform configuration.
    #[inline]
    pub fn cfg(&self) -> ConfigurationNoExec {
        self.0.cfg()
    }

    /// Get the execution platform, if resolution found one.
    pub fn platform(&self) -> buck2_error::Result<&ExecutionPlatform> {
        self.0.platform()
    }

    /// Get the list of skipped platforms and reasons.
    pub fn skipped(&self) -> &[(String, ExecutionPlatformIncompatibleReason)] {
        self.0.skipped()
    }

    /// Get the executor config for the resolved platform.
    pub fn executor_config(&self) -> buck2_error::Result<&Arc<CommandExecutorConfig>> {
        self.0.executor_config()
    }

    /// Finalize the partial resolution with per-exec_dep configurations.
    /// This produces a complete `ExecutionPlatformResolution` that can be stored
    /// and used for attribute configuration.
    pub fn finalize(
        self,
        exec_dep_cfgs: BuckFixedSortedMap<TargetLabel, ConfigurationData>,
    ) -> ExecutionPlatformResolution {
        self.finalize_with_execution_groups(exec_dep_cfgs, Vec::new(), true)
    }

    /// Like `finalize`, additionally recording the resolution of each named execution group the
    /// rule declares together with the cfgs of that group's exec deps, and whether the rule
    /// declares the default execution group.
    pub fn finalize_with_execution_groups(
        self,
        exec_dep_cfgs: BuckFixedSortedMap<TargetLabel, ConfigurationData>,
        execution_groups: Vec<ExecutionGroupResolutionPartial>,
        default_execution_group: bool,
    ) -> ExecutionPlatformResolution {
        let base = if execution_groups.is_empty() && default_execution_group {
            self.0
        } else {
            Arc::new(ExecutionPlatformResolutionBase {
                platform: self.0.platform.clone(),
                skipped_platforms: self.0.skipped_platforms.dupe(),
                execution_groups: Some(Arc::new(
                    execution_groups
                        .into_iter()
                        .map(|g| ExecutionGroupResolution {
                            group: g.group,
                            base: g.partial.0,
                            exec_dep_cfgs: Arc::new(g.exec_dep_cfgs),
                        })
                        .collect(),
                )),
                default_execution_group_declared: default_execution_group,
            })
        };
        ExecutionPlatformResolution::Resolved {
            base,
            exec_dep_cfgs: Arc::new(exec_dep_cfgs),
        }
    }
}

/// The resolution of a named execution group, before it is stored in an
/// `ExecutionPlatformResolution`.
pub struct ExecutionGroupResolutionPartial {
    pub group: NamedExecutionGroup,
    pub partial: ExecutionPlatformResolutionPartial,
    /// The cfgs the group's exec deps are configured in.
    pub exec_dep_cfgs: BuckFixedSortedMap<TargetLabel, ConfigurationData>,
}

#[derive(Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
struct ExecutionGroupResolution {
    group: NamedExecutionGroup,
    base: Arc<ExecutionPlatformResolutionBase>,
    exec_dep_cfgs: Arc<BuckFixedSortedMap<TargetLabel, ConfigurationData>>,
}

/// Complete execution platform resolution (stored in ConfiguredTargetNode).
///
/// This enum represents the two possible states:
/// - `Unspecified`: No execution platform configured (legacy/default behavior)
/// - `Resolved`: Fully resolved with per-exec_dep configurations computed
#[derive(Debug, Eq, PartialEq, Hash, Clone, Dupe, Allocative, Pagable)]
#[non_exhaustive]
#[allow(private_interfaces)]
pub enum ExecutionPlatformResolution {
    /// No execution platform specified (legacy/default behavior).
    /// Used during dependency gathering before execution platform is resolved,
    /// or for unbound configurations.
    Unspecified,
    /// Fully resolved with per-exec_dep configurations computed via modifiers.
    Resolved {
        #[doc(hidden)]
        base: Arc<ExecutionPlatformResolutionBase>,
        /// Mapping from exec_dep target labels to their actual configured cfgs.
        /// This is used when modifiers are applied to exec_deps, which may result
        /// in a different cfg than the default execution platform cfg.
        #[doc(hidden)]
        exec_dep_cfgs: Arc<BuckFixedSortedMap<TargetLabel, ConfigurationData>>,
    },
}

impl ExecutionPlatformResolution {
    /// Create an unspecified resolution.
    /// This is used when execution platform resolution hasn't been done yet
    /// (e.g., during gather_deps before resolution, or for unbound configurations).
    pub fn unspecified() -> Self {
        Self::Unspecified
    }

    /// Create a new complete resolution directly for testing purposes.
    /// In production code, use `ExecutionPlatformResolutionPartial::new(...).finalize(...)`.
    pub fn new_for_testing(
        platform: Option<ExecutionPlatform>,
        skipped: Vec<(String, ExecutionPlatformIncompatibleReason)>,
    ) -> Self {
        Self::Resolved {
            base: ExecutionPlatformResolutionBase::new(platform, skipped),
            exec_dep_cfgs: Arc::new(BuckFixedSortedMap::new()),
        }
    }

    /// Get the base resolution data, if available.
    fn base(&self) -> Option<&ExecutionPlatformResolutionBase> {
        match self {
            Self::Unspecified => None,
            Self::Resolved { base, .. } => Some(base),
        }
    }

    // TODO(cjhopman): Should this be an buck2_error::Result and never return an invalid configuration?
    #[inline]
    pub fn base_cfg(&self) -> ConfigurationNoExec {
        match self.base() {
            Some(base) => base.cfg(),
            None => ConfigurationNoExec::unspecified_exec().dupe(),
        }
    }

    pub fn platform(&self) -> buck2_error::Result<&ExecutionPlatform> {
        match self.base() {
            Some(base) => base.platform(),
            None => Err(ExecutionPlatformError::ResolutionNotPerformed.into()),
        }
    }

    pub fn skipped(&self) -> &[(String, ExecutionPlatformIncompatibleReason)] {
        match self.base() {
            Some(base) => base.skipped(),
            None => &[],
        }
    }

    pub fn executor_config(&self) -> buck2_error::Result<&Arc<CommandExecutorConfig>> {
        match self.base() {
            Some(base) => base.executor_config(),
            None => Err(ExecutionPlatformError::ResolutionNotPerformed.into()),
        }
    }

    /// Returns true if this is in the Unspecified state (no execution platform resolution).
    /// This is used during dependency gathering before execution platform is resolved.
    pub fn is_unspecified(&self) -> bool {
        matches!(self, Self::Unspecified)
    }

    /// Get the configuration data for an exec_dep target.
    ///
    /// This is the correct way to configure exec_deps during analysis:
    /// - For `Unspecified`: returns the base execution platform cfg (fallback during gather_deps)
    /// - For `Resolved`: returns the cfg from `exec_dep_cfgs` if found, or error if not found
    ///
    /// If the target is not found in `Resolved` state, this indicates a bug where the
    /// exec_dep was not collected during dependency gathering.
    pub fn cfg_for_exec_dep(&self, target: &TargetLabel) -> buck2_error::Result<ConfigurationData> {
        self.cfg_for_exec_dep_in_group(&ExecutionGroup::Default, target)
    }

    /// Like `cfg_for_exec_dep`, for an exec dep in the given execution group.
    pub fn cfg_for_exec_dep_in_group(
        &self,
        group: &ExecutionGroup,
        target: &TargetLabel,
    ) -> buck2_error::Result<ConfigurationData> {
        let Some((_, exec_dep_cfgs)) = self.group_parts(group)? else {
            // During gather_deps, we use the base cfg as a placeholder.
            // This will be reconfigured later with the proper cfg.
            return Ok(self.base_cfg().cfg().dupe());
        };
        exec_dep_cfgs
            .get(target)
            .map(|cfg| cfg.dupe())
            .ok_or_else(|| {
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "exec_dep `{}` was not found in the exec_dep_cfgs of execution group `{}`. \
                    This indicates a bug: this exec_dep was not collected during \
                    dependency gathering. Available exec_deps: [{}]",
                    target,
                    group,
                    exec_dep_cfgs
                        .keys()
                        .map(|k| k.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }

    /// The base resolution and exec dep cfgs of `group`, or `None` in the `Unspecified` state.
    fn group_parts(
        &self,
        group: &ExecutionGroup,
    ) -> buck2_error::Result<
        Option<(
            &ExecutionPlatformResolutionBase,
            &BuckFixedSortedMap<TargetLabel, ConfigurationData>,
        )>,
    > {
        let Self::Resolved {
            base,
            exec_dep_cfgs,
        } = self
        else {
            return Ok(None);
        };
        let named = match group {
            ExecutionGroup::Default => return Ok(Some((base, exec_dep_cfgs))),
            ExecutionGroup::Named(named) => named,
        };
        let g = base.execution_group(named)?;
        Ok(Some((&g.base, &g.exec_dep_cfgs)))
    }

    /// This resolution as seen from `group`: the group's execution platform and exec dep cfgs,
    /// with no named execution groups of its own. An unspecified resolution resolves no groups,
    /// so every group sees it unchanged.
    pub fn resolution_for_group(&self, group: &ExecutionGroup) -> buck2_error::Result<Self> {
        let (ExecutionGroup::Named(named), Self::Resolved { base, .. }) = (group, self) else {
            return Ok(self.dupe());
        };
        let group_resolution = base.execution_group(named)?;
        Ok(Self::Resolved {
            base: group_resolution.base.dupe(),
            exec_dep_cfgs: group_resolution.exec_dep_cfgs.dupe(),
        })
    }

    /// The execution platform cfg of `group`.
    pub fn base_cfg_for_group(
        &self,
        group: &ExecutionGroup,
    ) -> buck2_error::Result<ConfigurationNoExec> {
        Ok(match self.group_parts(group)? {
            Some((base, _)) => base.cfg(),
            None => ConfigurationNoExec::unspecified_exec().dupe(),
        })
    }

    /// The executor config actions in `group` run with.
    pub fn executor_config_for_group(
        &self,
        group: &ExecutionGroup,
    ) -> buck2_error::Result<&Arc<CommandExecutorConfig>> {
        match self.group_parts(group)? {
            Some((base, _)) => base.executor_config(),
            None => Err(ExecutionPlatformError::ResolutionNotPerformed.into()),
        }
    }

    /// The execution platform of `group`: `None` if resolution found no compatible platform.
    pub fn platform_for_group(
        &self,
        group: &ExecutionGroup,
    ) -> buck2_error::Result<Option<&ExecutionPlatform>> {
        match self.group_parts(group)? {
            Some((base, _)) => Ok(base.platform.as_ref()),
            None => Err(ExecutionPlatformError::ResolutionNotPerformed.into()),
        }
    }

    /// The named execution groups the rule declares, in declaration order, with the platform
    /// resolved for each (`None` if resolution found no compatible platform) and the platforms
    /// skipped for it.
    pub fn execution_group_platforms(
        &self,
    ) -> impl Iterator<
        Item = (
            &NamedExecutionGroup,
            Option<&ExecutionPlatform>,
            &[(String, ExecutionPlatformIncompatibleReason)],
        ),
    > {
        self.base()
            .map_or(&[][..], |base| base.execution_groups())
            .iter()
            .map(|g| (&g.group, g.base.platform.as_ref(), g.base.skipped()))
    }

    /// Whether exec deps, actions, anon targets and tests may use the default execution group
    /// implicitly: false only for rules that declare named execution groups but not
    /// `DefaultExecGroup`.
    pub fn default_execution_group_declared(&self) -> bool {
        self.base()
            .is_none_or(|base| base.default_execution_group_declared)
    }

    /// Whether the rule declares any named execution groups.
    pub fn has_execution_groups(&self) -> bool {
        self.base()
            .is_some_and(|base| base.execution_groups.is_some())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use dupe::Dupe;

    use super::*;
    use crate::cells::cell_path::CellPath;
    use crate::configuration::data::ConfigurationDataData;
    use crate::configuration::data::ExecPlatformMarking;
    use crate::execution_types::executor_config::CommandExecutorConfig;
    use crate::target::label::label::TargetLabel;

    fn cfg(name: &str) -> ConfigurationData {
        ConfigurationData::from_platform(
            name.to_owned(),
            ConfigurationDataData {
                constraints: BTreeMap::new(),
            },
            ExecPlatformMarking::Unmarked,
        )
        .unwrap()
    }

    fn partial(platform: &str) -> ExecutionPlatformResolutionPartial {
        ExecutionPlatformResolutionPartial::new(
            Some(ExecutionPlatform::platform(
                TargetLabel::testing_parse(&format!("root//:{platform}")),
                cfg(platform),
                CommandExecutorConfig::testing_local(),
            )),
            Vec::new(),
        )
    }

    fn link() -> NamedExecutionGroup {
        NamedExecutionGroup::new("Link".to_owned(), CellPath::testing_new("root//defs.bzl"))
    }

    fn tool() -> TargetLabel {
        TargetLabel::testing_parse("root//:tool")
    }

    #[test]
    fn test_without_execution_groups() {
        let resolution = partial("default").finalize(BuckFixedSortedMap::from_iter([(
            tool(),
            cfg("default_tool"),
        )]));
        assert!(!resolution.has_execution_groups());
        assert_eq!(resolution.execution_group_platforms().count(), 0);
        assert_eq!(
            resolution
                .cfg_for_exec_dep_in_group(&ExecutionGroup::Default, &tool())
                .unwrap(),
            cfg("default_tool")
        );
    }

    #[test]
    fn test_execution_group_has_own_platform_and_exec_dep_cfgs() {
        let resolution = partial("default").finalize_with_execution_groups(
            BuckFixedSortedMap::from_iter([(tool(), cfg("default_tool"))]),
            vec![ExecutionGroupResolutionPartial {
                group: link(),
                partial: partial("link"),
                exec_dep_cfgs: BuckFixedSortedMap::from_iter([(tool(), cfg("link_tool"))]),
            }],
            true,
        );
        assert!(resolution.has_execution_groups());
        assert_eq!(
            resolution.cfg_for_exec_dep(&tool()).unwrap(),
            cfg("default_tool")
        );
        let group = ExecutionGroup::Named(link());
        assert_eq!(
            resolution
                .cfg_for_exec_dep_in_group(&group, &tool())
                .unwrap(),
            cfg("link_tool")
        );
        assert_eq!(resolution.base_cfg().cfg(), &cfg("default"));
        assert_eq!(
            resolution.base_cfg_for_group(&group).unwrap().cfg(),
            &cfg("link")
        );
        let platforms: Vec<_> = resolution
            .execution_group_platforms()
            .map(|(g, p, skipped)| (g.dupe(), p.map(|p| p.cfg().dupe()), skipped.len()))
            .collect();
        assert_eq!(platforms, vec![(link(), Some(cfg("link")), 0)]);
    }

    #[test]
    fn test_platform_for_group() {
        let resolution = partial("default").finalize_with_execution_groups(
            BuckFixedSortedMap::new(),
            vec![ExecutionGroupResolutionPartial {
                group: link(),
                partial: ExecutionPlatformResolutionPartial::new(None, Vec::new()),
                exec_dep_cfgs: BuckFixedSortedMap::new(),
            }],
            true,
        );
        assert_eq!(
            resolution
                .platform_for_group(&ExecutionGroup::Default)
                .unwrap()
                .map(|p| p.cfg().dupe()),
            Some(cfg("default"))
        );
        assert!(
            resolution
                .platform_for_group(&ExecutionGroup::Named(link()))
                .unwrap()
                .is_none()
        );
        assert!(
            ExecutionPlatformResolution::unspecified()
                .platform_for_group(&ExecutionGroup::Named(link()))
                .is_err()
        );
    }

    #[test]
    fn test_resolution_for_group() {
        let resolution = partial("default").finalize_with_execution_groups(
            BuckFixedSortedMap::from_iter([(tool(), cfg("default_tool"))]),
            vec![ExecutionGroupResolutionPartial {
                group: link(),
                partial: partial("link"),
                exec_dep_cfgs: BuckFixedSortedMap::from_iter([(tool(), cfg("link_tool"))]),
            }],
            true,
        );
        let for_link = resolution
            .resolution_for_group(&ExecutionGroup::Named(link()))
            .unwrap();
        assert_eq!(for_link.base_cfg().cfg(), &cfg("link"));
        assert_eq!(
            for_link.cfg_for_exec_dep(&tool()).unwrap(),
            cfg("link_tool")
        );
        assert!(!for_link.has_execution_groups());
        assert_eq!(
            resolution
                .resolution_for_group(&ExecutionGroup::Default)
                .unwrap(),
            resolution
        );
        let other = NamedExecutionGroup::new(
            "Resources".to_owned(),
            CellPath::testing_new("root//defs.bzl"),
        );
        assert!(
            resolution
                .resolution_for_group(&ExecutionGroup::Named(other))
                .is_err()
        );
        let unspecified = ExecutionPlatformResolution::unspecified();
        assert_eq!(
            unspecified
                .resolution_for_group(&ExecutionGroup::Named(link()))
                .unwrap(),
            unspecified,
            "an unspecified resolution is the same for every group"
        );
    }

    #[test]
    fn test_default_execution_group_declared_without_named_groups() {
        let undeclared = partial("default").finalize_with_execution_groups(
            BuckFixedSortedMap::new(),
            Vec::new(),
            false,
        );
        assert!(!undeclared.default_execution_group_declared());
        let declared = partial("default").finalize_with_execution_groups(
            BuckFixedSortedMap::new(),
            Vec::new(),
            true,
        );
        assert!(declared.default_execution_group_declared());
    }

    #[test]
    fn test_undeclared_execution_group() {
        let resolution = partial("default").finalize_with_execution_groups(
            BuckFixedSortedMap::new(),
            vec![ExecutionGroupResolutionPartial {
                group: link(),
                partial: partial("link"),
                exec_dep_cfgs: BuckFixedSortedMap::new(),
            }],
            true,
        );
        let other = NamedExecutionGroup::new(
            "Resources".to_owned(),
            CellPath::testing_new("root//defs.bzl"),
        );
        let err = resolution
            .base_cfg_for_group(&ExecutionGroup::Named(other))
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("Execution group `Resources` is not declared by this rule. Declared groups: [`Link`]"),
            "{err}"
        );
    }

    #[test]
    fn test_undeclared_execution_group_with_declared_name() {
        let resolution = partial("default").finalize_with_execution_groups(
            BuckFixedSortedMap::new(),
            vec![ExecutionGroupResolutionPartial {
                group: link(),
                partial: partial("link"),
                exec_dep_cfgs: BuckFixedSortedMap::new(),
            }],
            true,
        );
        let other_link =
            NamedExecutionGroup::new("Link".to_owned(), CellPath::testing_new("root//other.bzl"));
        let err = resolution
            .base_cfg_for_group(&ExecutionGroup::Named(other_link))
            .unwrap_err();
        assert!(
            err.to_string().contains(
                "Execution group `Link` is not declared by this rule. Declared groups: [`Link`]. \
                 The declared `Link` is a different group with the same name: each \
                 `exec_group()` call creates its own group"
            ),
            "{err}"
        );
    }
}
