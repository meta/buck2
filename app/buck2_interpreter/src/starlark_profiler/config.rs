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
use async_trait::async_trait;
use buck2_common::pattern::parse_from_cli::parse_patterns_from_cli_args_typed;
use buck2_core::package::PackageLabel;
use buck2_core::pattern::package::PackagePredicate;
use buck2_core::pattern::pattern::ParsedPatternPredicate;
use buck2_core::pattern::pattern_type::ConfiguredProvidersPatternExtra;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::pattern::unparsed::UnparsedPatternPredicate;
use buck2_fs::paths::abs_path::AbsPathBuf;
use dice::DiceComputations;
use dice::DiceProjectionComputations;
use dice::DiceTransactionUpdater;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::Key;
use dice::NoValueSerialize;
use dice::OkPagableValueSerialize;
use dice::PagableValueSerialize;
use dice::ProjectionKey;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use itertools::Itertools;
use pagable::Pagable;
use pagable::pagable_typetag;
use ref_cast::RefCast;
use regex::Regex;
use starlark::eval::ProfileMode;

use crate::dice::starlark_provider::StarlarkEvalKind;
use crate::starlark_profiler::mode::StarlarkProfileMode;

/// Global profiling configuration.
#[derive(PartialEq, Eq, Clone, Debug, Allocative, Pagable)]
#[derive(Default)]
pub enum StarlarkProfilerConfiguration {
    /// No profiling.
    #[default]
    None,
    /// Profile loading of one `BUCK`.
    ProfileLoading(
        ProfileMode,
        UnparsedPatternPredicate<ConfiguredProvidersPatternExtra>,
    ),
    /// Profile analysis for given patterns.
    ProfileAnalysis(
        ProfileMode,
        UnparsedPatternPredicate<ConfiguredProvidersPatternExtra>,
    ),
    /// Profile BXL
    ProfileBxl(ProfileMode),
    /// Profile any evaluation with StarlarkEvalKind matching pattern
    ProfilePattern(ProfileMode, ProfileRegex, AbsPathBuf),
}

#[derive(Clone, Debug, Allocative, Pagable)]
pub struct ProfileRegex(#[allocative(skip)] Regex);
impl ProfileRegex {
    pub fn new(patterns: &[String]) -> buck2_error::Result<Self> {
        Ok(Self(Regex::new(&patterns.iter().join("|"))?))
    }
    fn matches(&self, arg: &StarlarkEvalKind) -> bool {
        self.0.is_match(&arg.to_string())
    }
}
impl Eq for ProfileRegex {}
impl PartialEq for ProfileRegex {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}

#[derive(PartialEq, Eq, Clone, Debug, Allocative, Pagable)]
enum StarlarkProfilerConfigurationResolved {
    None,
    ProfileLastLoading(ProfileMode, PackagePredicate),
    ProfileAnalysis(ProfileMode, ParsedPatternPredicate<TargetPatternExtra>),
    ProfileBxl(ProfileMode),
    ProfilePattern(ProfileMode, ProfileRegex),
}

impl StarlarkProfilerConfigurationResolved {
    fn mode_for(&self, kind: &StarlarkEvalKind) -> StarlarkProfileMode {
        let mode = match self {
            Self::None => None,
            Self::ProfileLastLoading(mode, packages) => match kind {
                StarlarkEvalKind::LoadBuildFile(package) if packages.matches(package.dupe()) => {
                    Some(mode)
                }
                _ => None,
            },
            Self::ProfileAnalysis(mode, patterns) => match kind {
                StarlarkEvalKind::Analysis(target) if patterns.matches(target.unconfigured()) => {
                    Some(mode)
                }
                _ => None,
            },
            Self::ProfileBxl(mode) => match kind {
                StarlarkEvalKind::Bxl(..) => Some(mode),
                _ => None,
            },
            Self::ProfilePattern(mode, pattern) => pattern.matches(kind).then_some(mode),
        };
        match mode {
            Some(mode) => StarlarkProfileMode::Profile(mode.dupe()),
            None => StarlarkProfileMode::None,
        }
    }

    /// False only if `mode_for` profiles no evaluation in the scope.
    fn may_profile(&self, scope: &ProfiledEvalScope) -> bool {
        match self {
            Self::None => false,
            Self::ProfileLastLoading(_, packages) => match scope {
                ProfiledEvalScope::LoadBuildFile(package) => packages.matches(package.dupe()),
                _ => false,
            },
            Self::ProfileAnalysis(_, patterns) => match scope {
                ProfiledEvalScope::Analysis(package) => patterns.matches_package(package.dupe()),
                _ => false,
            },
            Self::ProfileBxl(_) => matches!(scope, ProfiledEvalScope::Bxl),
            Self::ProfilePattern(..) => true,
        }
    }
}

#[derive(
    Debug,
    derive_more::Display,
    Copy,
    Clone,
    Dupe,
    Eq,
    PartialEq,
    Hash,
    Allocative,
    Pagable
)]
#[display("{:?}", self)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct StarlarkProfilerConfigurationResolvedKey;

#[async_trait]
impl Key for StarlarkProfilerConfigurationResolvedKey {
    type Value = buck2_error::Result<Arc<StarlarkProfilerConfigurationResolved>>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        let configuration = ctx.compute(&StarlarkProfilerConfigurationKey).await?;
        let new = match configuration {
            StarlarkProfilerConfiguration::None => StarlarkProfilerConfigurationResolved::None,
            StarlarkProfilerConfiguration::ProfileLoading(mode, patterns) => match patterns {
                UnparsedPatternPredicate::Any => {
                    StarlarkProfilerConfigurationResolved::ProfileLastLoading(
                        mode.dupe(),
                        PackagePredicate::Any,
                    )
                }
                UnparsedPatternPredicate::AnyOf(patterns) => {
                    let patterns = parse_patterns_from_cli_args_typed::<
                        ConfiguredProvidersPatternExtra,
                    >(ctx, patterns)
                    .await?;
                    let patterns = patterns
                        .into_iter()
                        .map(|p| p.into_package_pattern_ignore_target())
                        .collect();
                    StarlarkProfilerConfigurationResolved::ProfileLastLoading(
                        mode.dupe(),
                        PackagePredicate::AnyOf(patterns),
                    )
                }
            },
            StarlarkProfilerConfiguration::ProfileAnalysis(mode, patterns) => {
                match patterns {
                    UnparsedPatternPredicate::Any => {
                        StarlarkProfilerConfigurationResolved::ProfileAnalysis(
                            mode.dupe(),
                            ParsedPatternPredicate::Any,
                        )
                    }
                    UnparsedPatternPredicate::AnyOf(patterns) => {
                        let patterns = parse_patterns_from_cli_args_typed::<
                            ConfiguredProvidersPatternExtra,
                        >(ctx, patterns)
                        .await?;
                        let patterns = patterns
                            .into_iter()
                            .map(|p| {
                                p.map(|_| {
                                    // Drop the extra because:
                                    // - we don't use providers when profiling analysis
                                    // - configuration may create issues when there are transitions;
                                    //   it might return more than user requested without
                                    //   (e.g. target and host analysis), but practically this is not an issue.
                                    TargetPatternExtra
                                })
                            })
                            .collect();
                        StarlarkProfilerConfigurationResolved::ProfileAnalysis(
                            mode.dupe(),
                            ParsedPatternPredicate::AnyOf(patterns),
                        )
                    }
                }
            }
            StarlarkProfilerConfiguration::ProfileBxl(mode) => {
                StarlarkProfilerConfigurationResolved::ProfileBxl(mode.dupe())
            }
            StarlarkProfilerConfiguration::ProfilePattern(mode, pattern, _) => {
                StarlarkProfilerConfigurationResolved::ProfilePattern(mode.dupe(), pattern.clone())
            }
        };
        Ok(Arc::new(new))
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

#[derive(
    Debug,
    derive_more::Display,
    Clone,
    Eq,
    PartialEq,
    Hash,
    Allocative,
    RefCast,
    Pagable
)]
#[repr(transparent)]
#[pagable_typetag(dice::DiceProjectionDyn)]
struct StarlarkProfileModeForKind(StarlarkEvalKind);

impl ProjectionKey for StarlarkProfileModeForKind {
    type DeriveFromKey = StarlarkProfilerConfigurationResolvedKey;

    type Value = buck2_error::Result<StarlarkProfileMode>;

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }

    fn compute(
        &self,
        configuration: &buck2_error::Result<Arc<StarlarkProfilerConfigurationResolved>>,
        _ctx: &DiceProjectionComputations,
    ) -> buck2_error::Result<StarlarkProfileMode> {
        Ok(configuration
            .as_ref()
            .map_err(|e| e.dupe())?
            .mode_for(&self.0))
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        })
    }
}

/// Global Starlark compiler instrumentation level.
///
/// We profile only leaf computations (`BUCK` files or analysis),
/// and this key defines instrumentation of all the Starlark files,
/// regardless of whether profiled entity depends on them or not.
/// It's easier to implement with single global key,
/// the downside is we invalidate parse results when we switch
/// between normal operation/profiling.
#[derive(
    Debug,
    derive_more::Display,
    Copy,
    Clone,
    Dupe,
    Eq,
    PartialEq,
    Hash,
    Allocative,
    Pagable
)]
#[display("{:?}", self)]
#[pagable_typetag(dice::DiceKeyDyn)]
pub struct StarlarkProfilerConfigurationKey;

impl InjectedKey for StarlarkProfilerConfigurationKey {
    type Value = StarlarkProfilerConfiguration;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        PagableValueSerialize::<Self::Value>::new()
    }
}

#[async_trait]
pub trait SetStarlarkProfilerInstrumentation {
    fn set_starlark_profiler_configuration(
        &mut self,
        instrumentation: StarlarkProfilerConfiguration,
    ) -> buck2_error::Result<()>;
}

#[async_trait]
pub trait GetStarlarkProfilerInstrumentation {
    async fn get_starlark_profiler_mode(
        &mut self,
        eval_kind: &StarlarkEvalKind,
    ) -> buck2_error::Result<StarlarkProfileMode>;
}

#[async_trait]
impl SetStarlarkProfilerInstrumentation for DiceTransactionUpdater {
    fn set_starlark_profiler_configuration(
        &mut self,
        configuration: StarlarkProfilerConfiguration,
    ) -> buck2_error::Result<()> {
        Ok(self.changed_to([(StarlarkProfilerConfigurationKey, configuration)])?)
    }
}

/// What a profiling configuration can single out about an evaluation: its
/// kind and, for a build file or an analysis, its package.
#[derive(
    Debug,
    derive_more::Display,
    Clone,
    Dupe,
    Eq,
    PartialEq,
    Hash,
    Allocative,
    Pagable
)]
#[display("{:?}", self)]
enum ProfiledEvalScope {
    LoadBuildFile(PackageLabel),
    Analysis(PackageLabel),
    Bxl,
    Other,
}

impl ProfiledEvalScope {
    fn of(kind: &StarlarkEvalKind) -> Self {
        match kind {
            StarlarkEvalKind::LoadBuildFile(package) => Self::LoadBuildFile(package.dupe()),
            StarlarkEvalKind::Analysis(target) => Self::Analysis(target.pkg().dupe()),
            StarlarkEvalKind::Bxl(_) => Self::Bxl,
            StarlarkEvalKind::Load(_)
            | StarlarkEvalKind::LoadPackageFile(_)
            | StarlarkEvalKind::Transition(_)
            | StarlarkEvalKind::AnonTarget(_)
            | StarlarkEvalKind::DynamicOutput(_)
            | StarlarkEvalKind::BxlDynamic(_)
            | StarlarkEvalKind::Unknown(_) => Self::Other,
        }
    }
}

/// Whether the current command's profiling configuration can profile the
/// evaluations of a scope.
#[derive(
    Debug,
    derive_more::Display,
    Clone,
    Dupe,
    Eq,
    PartialEq,
    Hash,
    Allocative,
    Pagable
)]
#[display("{:?}", self)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct StarlarkProfilingScopeKey(ProfiledEvalScope);

#[async_trait]
impl Key for StarlarkProfilingScopeKey {
    type Value = bool;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        match ctx.compute(&StarlarkProfilerConfigurationResolvedKey).await {
            Ok(Ok(configuration)) => configuration.may_profile(&self.0),
            // The error is reported by `StarlarkProfileModeForKind`.
            Ok(Err(_)) | Err(_) => true,
        }
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        PagableValueSerialize::<Self::Value>::new()
    }
}

#[async_trait]
impl GetStarlarkProfilerInstrumentation for DiceComputations<'_> {
    async fn get_starlark_profiler_mode(
        &mut self,
        eval_kind: &StarlarkEvalKind,
    ) -> buck2_error::Result<StarlarkProfileMode> {
        if !*self
            .compute(&StarlarkProfilingScopeKey(ProfiledEvalScope::of(eval_kind)))
            .await?
        {
            return Ok(StarlarkProfileMode::None);
        }

        let cfg = self
            .compute_opaque(&StarlarkProfilerConfigurationResolvedKey)
            .await?;

        Ok(self.projection(&cfg, StarlarkProfileModeForKind::ref_cast(eval_kind))??)
    }
}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use buck2_core::bzl::ImportPath;
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::configuration::transition::id::TransitionId;
    use buck2_core::deferred::dyn_eval_kind_key::DynEvalKindKey;
    use buck2_core::pattern::package::PackagePattern;
    use buck2_core::pattern::pattern::ParsedPattern;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
    use buck2_core::target::name::TargetName;
    use buck2_util::arc_str::ThinArcStr;

    use super::*;
    use crate::paths::module::OwnedStarlarkModulePath;

    fn package(label: &str) -> PackageLabel {
        PackageLabel::testing_parse(label)
    }

    #[derive(
        Debug,
        derive_more::Display,
        Hash,
        Eq,
        PartialEq,
        Allocative,
        strong_hash::StrongHash,
        Pagable
    )]
    #[display("test_key")]
    #[pagable_typetag(DynEvalKindKey)]
    struct TestKey;

    fn kinds() -> Vec<StarlarkEvalKind> {
        let analysis = |label| {
            StarlarkEvalKind::Analysis(ConfiguredTargetLabel::testing_parse(
                label,
                ConfigurationData::testing_new(),
            ))
        };
        vec![
            analysis("root//a:t"),
            analysis("root//a:u"),
            analysis("root//b/x:y"),
            analysis("root//c:z"),
            analysis("root//d:w"),
            StarlarkEvalKind::LoadBuildFile(package("root//a")),
            StarlarkEvalKind::LoadBuildFile(package("root//b/x")),
            StarlarkEvalKind::LoadBuildFile(package("root//d")),
            StarlarkEvalKind::LoadPackageFile(package("root//a")),
            StarlarkEvalKind::Load(Arc::new(OwnedStarlarkModulePath::LoadFile(
                ImportPath::testing_new("root//a:defs.bzl"),
            ))),
            StarlarkEvalKind::Transition(Arc::new(TransitionId::MagicObject {
                path: ImportPath::testing_new("root//a:defs.bzl"),
                name: "transition".to_owned(),
            })),
            StarlarkEvalKind::AnonTarget(Arc::new(TestKey)),
            StarlarkEvalKind::DynamicOutput(Arc::new(TestKey)),
            StarlarkEvalKind::Bxl(Arc::new(TestKey)),
            StarlarkEvalKind::BxlDynamic(ThinArcStr::from("dynamic")),
            StarlarkEvalKind::Unknown(ThinArcStr::from("unknown")),
        ]
    }

    fn configurations() -> Vec<StarlarkProfilerConfigurationResolved> {
        let mode = ProfileMode::Statement;
        let under = |label: &str| package(label).as_cell_path().to_owned();
        let regex = |patterns: &[&str]| {
            ProfileRegex::new(&patterns.iter().map(|p| p.to_string()).collect::<Vec<_>>()).unwrap()
        };
        vec![
            StarlarkProfilerConfigurationResolved::None,
            StarlarkProfilerConfigurationResolved::ProfileLastLoading(
                mode.dupe(),
                PackagePredicate::Any,
            ),
            StarlarkProfilerConfigurationResolved::ProfileLastLoading(
                mode.dupe(),
                PackagePredicate::AnyOf(vec![
                    PackagePattern::Package(package("root//a")),
                    PackagePattern::Recursive(under("root//b")),
                ]),
            ),
            StarlarkProfilerConfigurationResolved::ProfileAnalysis(
                mode.dupe(),
                ParsedPatternPredicate::Any,
            ),
            StarlarkProfilerConfigurationResolved::ProfileAnalysis(
                mode.dupe(),
                ParsedPatternPredicate::AnyOf(vec![
                    ParsedPattern::Target(
                        package("root//a"),
                        TargetName::testing_new("t"),
                        TargetPatternExtra,
                    ),
                    ParsedPattern::Package(package("root//c")),
                    ParsedPattern::Recursive(under("root//b")),
                ]),
            ),
            StarlarkProfilerConfigurationResolved::ProfileBxl(mode.dupe()),
            StarlarkProfilerConfigurationResolved::ProfilePattern(
                mode.dupe(),
                regex(&["analysis/a/.*"]),
            ),
            StarlarkProfilerConfigurationResolved::ProfilePattern(
                mode.dupe(),
                regex(&["load_bzl/.*", "unknown"]),
            ),
            StarlarkProfilerConfigurationResolved::ProfilePattern(mode.dupe(), regex(&[".*"])),
        ]
    }

    #[test]
    fn test_scope_covers_every_profiled_evaluation() {
        let mut profiled = 0;
        for configuration in configurations() {
            for kind in kinds() {
                if configuration.mode_for(&kind) != StarlarkProfileMode::None {
                    profiled += 1;
                    assert!(
                        configuration.may_profile(&ProfiledEvalScope::of(&kind)),
                        "{configuration:?} profiles `{kind}` but its scope says no"
                    );
                }
            }
        }
        // Per configuration above: 0 + 3 + 2 + 5 + 3 + 1 + 2 + 2 + 16.
        assert_eq!(profiled, 34);
    }

    #[test]
    fn test_scope_is_per_package() {
        let configurations = configurations();
        let [_, _, loading, _, analysis, bxl, ..] = configurations.as_slice() else {
            unreachable!()
        };
        assert!(loading.may_profile(&ProfiledEvalScope::LoadBuildFile(package("root//b/x"))));
        assert!(!loading.may_profile(&ProfiledEvalScope::LoadBuildFile(package("root//d"))));
        assert!(!loading.may_profile(&ProfiledEvalScope::Analysis(package("root//a"))));
        assert!(analysis.may_profile(&ProfiledEvalScope::Analysis(package("root//a"))));
        assert!(!analysis.may_profile(&ProfiledEvalScope::Analysis(package("root//d"))));
        assert!(!analysis.may_profile(&ProfiledEvalScope::LoadBuildFile(package("root//a"))));
        assert!(bxl.may_profile(&ProfiledEvalScope::Bxl));
        assert!(!bxl.may_profile(&ProfiledEvalScope::Analysis(package("root//a"))));
    }
}
