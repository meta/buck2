/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use dupe::Dupe;

use crate::configuration::data::ConfigurationData;
use crate::package::PackageLabel;
use crate::target::configured_target_label::ConfiguredTargetLabel;
use crate::target::label::label::TargetLabel;

/// A wrapper around a configured target label.
///
/// The semantics of this type are exactly the same as that of a configured target label, except
/// with one distinction - when the target being referred to is a toolchain target, the label will
/// still only be configured with a target platform, not an exec platform.
///
/// This is used to mark code which deals with configurations of toolchains, but does not actually
/// care about the toolchain's exec platform.
#[derive(
    Copy,
    Clone,
    Dupe,
    Debug,
    derive_more::Display,
    Hash,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    allocative::Allocative,
    pagable::Pagable
)]
pub struct TargetConfiguredTargetLabel(ConfiguredTargetLabel);

impl TargetConfiguredTargetLabel {
    pub fn new_without_exec_cfg(label: ConfiguredTargetLabel) -> Self {
        Self(label.unconfigured().configure(label.cfg().dupe()))
    }

    pub fn new_configure(label: &TargetLabel, cfg: ConfigurationData) -> Self {
        Self(label.configure(cfg))
    }

    pub fn unconfigured(&self) -> &TargetLabel {
        self.0.unconfigured()
    }

    pub fn cfg(&self) -> &ConfigurationData {
        self.0.cfg()
    }

    pub fn pkg(&self) -> PackageLabel {
        self.0.pkg()
    }

    /// Sets the exec configuration.
    ///
    /// Should only be used with toolchain targets.
    pub fn with_exec_cfg(&self, cfg: ConfigurationData) -> ConfiguredTargetLabel {
        self.0.with_exec_cfg(cfg)
    }

    pub fn inner(&self) -> &ConfiguredTargetLabel {
        &self.0
    }
}
