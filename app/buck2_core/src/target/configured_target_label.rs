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
use std::fmt::{self};
use std::hash::Hash;
use std::str;

use allocative::Allocative;
use buck2_data::ToProtoMessage;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serialize;
use serde::Serializer;
use strong_hash::StrongHash;

use crate::configuration::data::ConfigurationData;
use crate::configuration::pair::Configuration;
use crate::package::PackageLabel;
use crate::target::label::label::TargetLabel;
use crate::target::name::TargetNameRef;

/// 'ConfiguredTargetLabel' are 'TargetLabel's with an 'Configuration' attached.
/// These uniquely map to nodes of the build graph with 'Configuration's
/// applied.
#[derive(
    Copy, Clone, Dupe, Hash, Eq, PartialEq, Ord, PartialOrd, Allocative, StrongHash, Pagable
)]
pub struct ConfiguredTargetLabel {
    pub(crate) target: TargetLabel,
    pub(crate) cfg_pair: Configuration,
}

impl Display for ConfiguredTargetLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.target, self.cfg())?;
        if let Some(exec_cfg) = self.exec_cfg() {
            write!(f, " ({exec_cfg})")?;
        }
        Ok(())
    }
}

impl Debug for ConfiguredTargetLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl ConfiguredTargetLabel {
    #[inline]
    pub fn pkg(&self) -> PackageLabel {
        self.target.pkg()
    }

    #[inline]
    pub fn name(&self) -> &TargetNameRef {
        self.target.name()
    }

    #[inline]
    pub fn unconfigured(&self) -> &TargetLabel {
        &self.target
    }

    #[inline]
    pub fn cfg_pair(&self) -> &Configuration {
        &self.cfg_pair
    }

    #[inline]
    pub fn cfg(&self) -> &ConfigurationData {
        self.cfg_pair.cfg()
    }

    #[inline]
    pub fn exec_cfg(&self) -> Option<&ConfigurationData> {
        self.cfg_pair.exec_cfg()
    }

    /// Sets the exec cfg to the given one
    pub fn with_exec_cfg(&self, new_exec_cfg: ConfigurationData) -> Self {
        Self {
            target: self.target.dupe(),
            cfg_pair: Configuration::new(self.cfg().dupe(), Some(new_exec_cfg)),
        }
    }

    pub fn testing_parse(label: &str, cfg: ConfigurationData) -> ConfiguredTargetLabel {
        TargetLabel::testing_parse(label).configure(cfg)
    }
}

impl Serialize for ConfiguredTargetLabel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl ToProtoMessage for ConfiguredTargetLabel {
    type Message = buck2_data::ConfiguredTargetLabel;

    fn as_proto(&self) -> Self::Message {
        buck2_data::ConfiguredTargetLabel {
            label: Some(self.unconfigured().as_proto()),
            configuration: Some(self.cfg().as_proto()),
            execution_configuration: self.exec_cfg().map(ToProtoMessage::as_proto),
        }
    }
}
