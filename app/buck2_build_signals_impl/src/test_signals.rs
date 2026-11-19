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

use buck2_artifact::actions::key::ActionKey;
use buck2_build_signals::env::NodeDuration;
use buck2_build_signals::env::WaitingData;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use dupe::Dupe;

#[derive(Hash, Eq, PartialEq, Clone, Dupe, Debug)]
/// A key to uniquely identify a test execution.
/// This is used to specify action dependencies e.g. in critical path analysis.
pub(crate) struct TestExecutionBuildSignalKey {
    pub(crate) target: ConfiguredTargetLabel,
    pub(crate) suite: Arc<String>,
    pub(crate) testcases: Arc<Vec<String>>,
    pub(crate) variant: Option<Arc<String>>,
}

#[derive(Hash, Eq, PartialEq, Clone, Dupe, Debug)]
/// A key to uniquely identify a test listing build signal.
/// This is used to specify action dependencies e.g. in critical path analysis.
pub(crate) struct TestListingBuildSignalKey {
    pub(crate) target: ConfiguredTargetLabel,
    pub(crate) suite: Arc<String>,
}

/// Build signal emitted at the end of a test execution.
pub(crate) struct TestExecutionSignal {
    pub(crate) target: ConfiguredTargetLabel,
    pub(crate) suite: String,
    pub(crate) testcases: Vec<String>,
    pub(crate) variant: Option<String>,
    pub(crate) deps: Vec<ActionKey>,
    pub(crate) duration: NodeDuration,
    pub(crate) waiting_data: WaitingData,
}

/// Build signal emitted at the end of test discovery.
pub(crate) struct TestListingSignal {
    pub(crate) target: ConfiguredTargetLabel,
    pub(crate) suite: String,
    pub(crate) deps: Vec<ActionKey>,
    pub(crate) duration: NodeDuration,
    pub(crate) waiting_data: WaitingData,
}

#[derive(Hash, Eq, PartialEq, Clone, Dupe, Debug)]
/// A key to uniquely identify an installer run within a command.
pub(crate) struct InstallBuildSignalKey {
    pub(crate) installer: ConfiguredProvidersLabel,
    pub(crate) targets: Arc<Vec<ConfiguredTargetLabel>>,
}

/// Build signal emitted when an installer run finishes.
pub(crate) struct InstallSignal {
    pub(crate) installer: ConfiguredProvidersLabel,
    pub(crate) targets: Vec<ConfiguredTargetLabel>,
    pub(crate) deps: Vec<ActionKey>,
    pub(crate) duration: NodeDuration,
    pub(crate) waiting_data: WaitingData,
}
