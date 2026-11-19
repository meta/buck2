/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::LazyLock;

use allocative::Allocative;
use buck2_hash::BuckHasher;
use dupe::Dupe;
use pagable::Pagable;
use static_interner::Intern;
use static_interner::interner;
use strong_hash::StrongHash;

use crate::configuration::data::ConfigurationData;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum ConfigurationError {
    #[error("`ConfigurationPair` has unexpected `exec_cfg`")]
    HasExecCfg,
}

#[derive(
    Debug, Allocative, Hash, Eq, PartialEq, Ord, PartialOrd, Pagable, StrongHash
)]
struct ConfigurationPairData {
    cfg: ConfigurationData,
    /// Usually this is None, but for toolchain deps where the exec_cfg isn't picked it is set
    exec_cfg: Option<ConfigurationData>,
}

/// Pair of `cfg` and `exec_cfg`.
/// These two are added to `TargetLabel` to make `ConfiguredTargetLabel`.
#[derive(
    Debug, Copy, Clone, Dupe, Hash, Eq, PartialEq, Ord, PartialOrd, Allocative, StrongHash, Pagable
)]
pub struct Configuration(Intern<ConfigurationPairData>);

interner!(INTERNER, BuckHasher, ConfigurationPairData);

impl Configuration {
    #[inline]
    pub fn new(cfg: ConfigurationData, exec_cfg: Option<ConfigurationData>) -> Configuration {
        Configuration(INTERNER.intern(ConfigurationPairData { cfg, exec_cfg }))
    }

    #[inline]
    pub fn cfg(&self) -> &ConfigurationData {
        &self.0.cfg
    }

    #[inline]
    pub fn exec_cfg(&self) -> Option<&ConfigurationData> {
        self.0.exec_cfg.as_ref()
    }

    #[inline]
    pub fn check_no_exec_cfg(&self) -> buck2_error::Result<ConfigurationNoExec> {
        if self.exec_cfg().is_some() {
            return Err(ConfigurationError::HasExecCfg.into());
        }
        Ok(ConfigurationNoExec(self.dupe()))
    }
}

/// `ConfigurationPair` where `exec_cfg` is always `None`.
#[derive(
    Debug,
    Copy,
    Clone,
    Dupe,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    Allocative,
    derive_more::Display,
    strong_hash::StrongHash,
    Pagable
)]
#[display("{}", self.cfg())]
pub struct ConfigurationNoExec(Configuration);

impl ConfigurationNoExec {
    #[inline]
    pub fn new(cfg: ConfigurationData) -> ConfigurationNoExec {
        ConfigurationNoExec(Configuration::new(cfg, None))
    }

    #[inline]
    pub fn unbound() -> ConfigurationNoExec {
        static UNBOUND: LazyLock<ConfigurationNoExec> =
            LazyLock::new(|| ConfigurationNoExec::new(ConfigurationData::unbound()));
        UNBOUND.dupe()
    }

    #[inline]
    pub fn unspecified_exec() -> Self {
        static UNSPECIFIED_EXEC: LazyLock<ConfigurationNoExec> =
            LazyLock::new(|| ConfigurationNoExec::new(ConfigurationData::unspecified_exec()));
        UNSPECIFIED_EXEC.dupe()
    }

    #[inline]
    pub fn unbound_exec() -> Self {
        static UNBOUND_EXEC: LazyLock<ConfigurationNoExec> =
            LazyLock::new(|| ConfigurationNoExec::new(ConfigurationData::unbound_exec()));
        UNBOUND_EXEC.dupe()
    }

    #[inline]
    pub fn unspecified() -> Self {
        static UNSPECIFIED: LazyLock<ConfigurationNoExec> =
            LazyLock::new(|| ConfigurationNoExec::new(ConfigurationData::unspecified()));
        UNSPECIFIED.dupe()
    }

    #[inline]
    pub fn testing_new() -> Self {
        static TESTING_NEW: LazyLock<ConfigurationNoExec> =
            LazyLock::new(|| ConfigurationNoExec::new(ConfigurationData::testing_new()));
        TESTING_NEW.dupe()
    }

    #[inline]
    pub fn cfg_pair(&self) -> &Configuration {
        &self.0
    }

    #[inline]
    pub fn cfg(&self) -> &ConfigurationData {
        self.cfg_pair().cfg()
    }

    #[inline]
    pub fn make_toolchain(&self, exec_cfg: &ConfigurationNoExec) -> ConfigurationWithExec {
        ConfigurationWithExec::new(self.cfg().dupe(), exec_cfg.cfg().dupe())
    }
}

/// `ConfigurationPair` where `exec_cfg` is always `Some`.
#[derive(Debug, Clone, Dupe, Eq, PartialEq, Hash, Ord, PartialOrd, Allocative)]
pub struct ConfigurationWithExec(Configuration);

impl ConfigurationWithExec {
    #[inline]
    pub fn new(cfg: ConfigurationData, exec_cfg: ConfigurationData) -> ConfigurationWithExec {
        ConfigurationWithExec(Configuration::new(cfg, Some(exec_cfg)))
    }

    #[inline]
    pub fn cfg(&self) -> &ConfigurationData {
        self.cfg_pair().cfg()
    }

    #[inline]
    pub fn exec_cfg(&self) -> &ConfigurationData {
        self.cfg_pair()
            .exec_cfg()
            .expect("`exec_cfg` is always `Some`")
    }

    #[inline]
    pub fn cfg_pair(&self) -> &Configuration {
        &self.0
    }
}
