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
use std::time::Instant;

use buck2_artifact::actions::key::ActionKey;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_build_signals::env::DeferredBuildSignals;
use buck2_build_signals::env::NodeDuration;
use buck2_build_signals::env::WaitingData;
use buck2_core::package::PackageLabel;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_events::span::SpanId;
use buck2_util::late_binding::LateBinding;
use dice::ActivationTracker;
use dice::UserComputationData;
use dupe::Dupe;

use crate::artifact_groups::ArtifactGroup;
use crate::artifact_groups::ResolvedArtifactGroupBuildSignalsKey;

pub static CREATE_BUILD_SIGNALS: LateBinding<
    fn() -> (BuildSignalsInstaller, Box<dyn DeferredBuildSignals>),
> = LateBinding::new("CREATE_BUILD_SIGNALS");

pub fn create_build_signals() -> (BuildSignalsInstaller, Box<dyn DeferredBuildSignals>) {
    (CREATE_BUILD_SIGNALS.get().unwrap())()
}

/// Everything we need to setup build signals when starting a command.
#[derive(Clone, Dupe)]
pub struct BuildSignalsInstaller {
    pub build_signals: Arc<dyn BuildSignals>,
    pub activation_tracker: Arc<dyn ActivationTracker>,
}

/// Send notifications to the build signals backend.
pub trait BuildSignals: Send + Sync + 'static {
    fn top_level_target(
        &self,
        label: ConfiguredTargetLabel,
        artifacts: Vec<ResolvedArtifactGroupBuildSignalsKey>,
    );

    /// Record a final materialization event for the critical path.
    ///
    /// `from_group` is the top-level `ArtifactGroup` that triggered this materialization.
    /// When the group is a transitive set projection, the critical path dependency is
    /// recorded against the tset ensure step rather than the individual build action.
    fn final_materialization(
        &self,
        artifact: BuildArtifact,
        from_group: ArtifactGroup,
        duration: NodeDuration,
        span_id: Option<SpanId>,
        waiting_data: WaitingData,
    );

    fn test_listing(
        &self,
        target: ConfiguredTargetLabel,
        suite: String,
        duration: NodeDuration,
        deps: &[ActionKey],
        waiting_data: WaitingData,
    );

    fn test_execution(
        &self,
        target: ConfiguredTargetLabel,
        suite: String,
        testcases: &[String],
        variant: Option<String>,
        duration: NodeDuration,
        deps: &[ActionKey],
        waiting_data: WaitingData,
    );

    /// Record an installer run that consumed the artifacts built by `deps`.
    fn install(
        &self,
        installer: ConfiguredProvidersLabel,
        targets: Vec<ConfiguredTargetLabel>,
        duration: NodeDuration,
        deps: &[ActionKey],
        waiting_data: WaitingData,
    );

    /// A package load requested at `requested_at` on behalf of a node of
    /// `discovered_by`, by code that drives loads outside any tracked computation
    /// (query traversals). Must be called after the request returns. Credits the
    /// load to that package's load on the critical path if the request started it.
    fn load_discovered(
        &self,
        package: PackageLabel,
        discovered_by: PackageLabel,
        requested_at: Instant,
    );
}

pub trait SetBuildSignals {
    fn set_build_signals(&mut self, sender: Arc<dyn BuildSignals>);
}

impl SetBuildSignals for UserComputationData {
    fn set_build_signals(&mut self, sender: Arc<dyn BuildSignals>) {
        self.data.set(sender);
    }
}

pub trait HasBuildSignals {
    fn get_build_signals(&self) -> Option<&Arc<dyn BuildSignals>>;
}

impl HasBuildSignals for UserComputationData {
    fn get_build_signals(&self) -> Option<&Arc<dyn BuildSignals>> {
        self.data.get::<Arc<dyn BuildSignals>>().ok()
    }
}
