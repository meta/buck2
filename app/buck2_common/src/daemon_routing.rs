/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_core::rollout_percentage::RolloutPercentage;
use serde::Deserialize;
use serde::Serialize;

use crate::init::MATERIALIZER_V2_ROLLOUT_VERSION;
use crate::legacy_configs::configs::LegacyBuckConfig;
use crate::legacy_configs::key::BuckconfigKeyRef;

/// How a client selects the daemon that serves its tenant.
///
/// Read from the `[buck2]` section of the root buckconfig, so every client of a checkout agrees on
/// it before any of them talks to a daemon, and carried in the daemon's startup options, so a
/// daemon only serves tenants routed the same way.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    Allocative,
    Serialize,
    Deserialize
)]
pub enum DaemonRouting {
    /// One daemon per `(project root, isolation directory)`.
    #[default]
    Unshared,
    /// Compatible tenants share one daemon, selected by the compatibility key of the client's
    /// release and startup configuration. Each tenant keeps a proxy in its unshared daemon
    /// directory, so clients of other releases evict it the way they kill any other daemon.
    Shared,
}

impl DaemonRouting {
    /// Reads `buck2.share_daemon`, a `RolloutPercentage`: `true` or a `hostname:<fraction>` share.
    ///
    /// The per-process random form is rejected because every client on a host must select the
    /// same daemon. Hosts that cannot run a shared daemon stay unshared whatever the share says, so
    /// one setting can roll out across a fleet.
    pub fn from_config(config: &LegacyBuckConfig) -> buck2_error::Result<Self> {
        let share_daemon = config
            .parse::<RolloutPercentage>(BuckconfigKeyRef {
                section: "buck2",
                property: "share_daemon",
            })?
            .unwrap_or_else(RolloutPercentage::never);
        if !share_daemon.is_stable_per_host() {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "`buck2.share_daemon` must be a bool or a `hostname:<fraction>`: every client on a host must select the same daemon"
            ));
        }
        if share_daemon.roll() && Self::host_supports_sharing(config)? {
            Ok(Self::Shared)
        } else {
            Ok(Self::Unshared)
        }
    }

    /// Shared daemons run on Linux with the deferred materializer.
    fn host_supports_sharing(config: &LegacyBuckConfig) -> buck2_error::Result<bool> {
        let disk_state_materializer = config
            .parse::<u32>(BuckconfigKeyRef {
                section: "buck2",
                property: "materializer_v2_version",
            })?
            .is_some_and(|wanted| MATERIALIZER_V2_ROLLOUT_VERSION >= wanted);
        Ok(cfg!(target_os = "linux") && !disk_state_materializer)
    }

    pub fn is_shared(&self) -> bool {
        matches!(self, Self::Shared)
    }

    /// The value reported as `daemon_routing` in command telemetry.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Unshared => "unshared",
            Self::Shared => "shared",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_configs::configs::testing::parse;

    fn routing(config: &str) -> buck2_error::Result<DaemonRouting> {
        DaemonRouting::from_config(&parse(&[("config", config)], "config")?)
    }

    #[test]
    fn unshared_by_default() {
        assert_eq!(routing("").unwrap(), DaemonRouting::Unshared);
        assert_eq!(
            routing("[buck2]\nshare_daemon = false").unwrap(),
            DaemonRouting::Unshared
        );
        assert_eq!(
            routing("[buck2]\nshare_daemon = hostname:0.0").unwrap(),
            DaemonRouting::Unshared
        );
    }

    #[test]
    fn shared_where_supported() {
        let expected = if cfg!(target_os = "linux") {
            DaemonRouting::Shared
        } else {
            DaemonRouting::Unshared
        };
        assert_eq!(routing("[buck2]\nshare_daemon = true").unwrap(), expected);
        assert_eq!(
            routing("[buck2]\nshare_daemon = hostname:1.0").unwrap(),
            expected
        );
        // A rollout version above this binary's leaves the deferred materializer in place.
        assert_eq!(
            routing(&format!(
                "[buck2]\nshare_daemon = true\nmaterializer_v2_version = {}",
                MATERIALIZER_V2_ROLLOUT_VERSION + 1
            ))
            .unwrap(),
            expected
        );
    }

    #[test]
    fn unsupported_hosts_stay_unshared() {
        assert_eq!(
            routing("[buck2]\nshare_daemon = true\nmaterializer_v2_version = 0").unwrap(),
            DaemonRouting::Unshared
        );
    }

    #[test]
    fn rejects_random_shares() {
        assert!(routing("[buck2]\nshare_daemon = daemon:0.5").is_err());
    }
}
