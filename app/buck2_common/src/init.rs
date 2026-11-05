/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::str::FromStr;
use std::time::Duration;

use allocative::Allocative;
use buck2_core::buck2_env;
use buck2_error::BuckErrorContext;
#[cfg(unix)]
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::file_name::FileName;
#[cfg(fbcode_build)]
use buck2_http::ProxyHostAllowlist;
use buck2_settings::BuckSettingChange;
use buck2_settings::DaemonBuckSettings;
use buck2_settings::Http;
use buck2_settings::MacosQosClass;
use buck2_settings::ResourceControl;
use dice::EvictionPolicy;
use dice::PagableStorageBackend;
use dice::PageOutPolicy;
use dupe::Dupe;
use serde::Deserialize;
use serde::Serialize;

use crate::legacy_configs::configs::LegacyBuckConfig;
use crate::legacy_configs::key::BuckconfigKeyRef;

/// Helper enum to categorize the kind of timeout we get from the startup config.
#[derive(Clone, Debug)]
pub enum Timeout {
    /// Timeout value is set in the config, use that.
    Value(Duration),
    /// Timeout value was not set in config, apply the default.
    Default,
    /// Timeout value was explicitly set to 0, meaning we shouldn't use a timeout.
    NoTimeout,
}

impl Timeout {
    pub fn new(value: Option<Duration>) -> Self {
        match value {
            Some(Duration::ZERO) => Self::NoTimeout,
            Some(value) => Self::Value(value),
            None => Self::Default,
        }
    }
}

#[derive(
    Allocative,
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq
)]
pub struct HttpConfig {
    /// Stays a buckconfig key: a list, which settings do not carry yet.
    #[cfg(fbcode_build)]
    pub proxy_env_allowlist: ProxyHostAllowlist,
    connect_timeout_ms: Option<u64>,
    read_timeout_ms: Option<u64>,
    write_timeout_ms: Option<u64>,
    pub http2: bool,
    pub max_redirects: Option<usize>,
    pub max_concurrent_requests: Option<usize>,
}

impl HttpConfig {
    pub fn from_settings(http: &Http, config: &LegacyBuckConfig) -> buck2_error::Result<Self> {
        let mut buckconfig = config;
        Ok(Self {
            #[cfg(fbcode_build)]
            proxy_env_allowlist: config
                .parse_list::<String>(BuckconfigKeyRef {
                    section: "http",
                    property: "proxy_env_allowlist",
                })?
                .unwrap_or_default()
                .try_into()?,
            connect_timeout_ms: http.connect_timeout_ms(&mut buckconfig)?,
            read_timeout_ms: http.read_timeout_ms(&mut buckconfig)?,
            write_timeout_ms: http.write_timeout_ms(&mut buckconfig)?,
            http2: http.http2(&mut buckconfig)?,
            max_redirects: http.max_redirects(&mut buckconfig)?,
            max_concurrent_requests: http.max_concurrent_requests(&mut buckconfig)?,
        })
    }

    pub fn connect_timeout(&self) -> Timeout {
        match self.connect_timeout_ms.map(Duration::from_millis) {
            Some(Duration::ZERO) => Timeout::NoTimeout,
            Some(value) => Timeout::Value(value),
            None => Timeout::Default,
        }
    }

    pub fn read_timeout(&self) -> Timeout {
        match self.read_timeout_ms.map(Duration::from_millis) {
            Some(Duration::ZERO) => Timeout::NoTimeout,
            Some(value) => Timeout::Value(value),
            None => Timeout::Default,
        }
    }

    pub fn write_timeout(&self) -> Timeout {
        match self.write_timeout_ms.map(Duration::from_millis) {
            Some(Duration::ZERO) => Timeout::NoTimeout,
            Some(value) => Timeout::Value(value),
            None => Timeout::Default,
        }
    }
}

#[derive(
    Allocative,
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq
)]
pub struct SystemWarningConfig {
    /// A threshold that is used to determine the percent of memory buck2 uses to display memory pressure warnings.
    /// If None, we don't warn the user.
    /// The corresponding buckconfig is `buck2_system_warning.memory_pressure_threshold_percent`.
    pub memory_pressure_threshold_percent: Option<u64>,
    /// A threshold that is used to determine remaining disk space buck2 uses to display disk space warnings.
    /// If None, we don't warn the user.
    /// The corresponding buckconfig is `buck2_system_warning.remaining_disk_space_threshold`.
    pub remaining_disk_space_threshold_gb: Option<u64>,
    /// Minimum number of bytes downloaded to measure average download speed.
    /// If None, we don't warn the user.
    /// The corresponding buckconfig is `buck2_system_warning.min_re_download_bytes_threshold`.
    pub min_re_download_bytes_threshold: Option<u64>,
    /// A threshold that is used to determine if download speed is too low and display a warning.
    /// If None, we don't warn the user.
    /// The corresponding buckconfig is `buck2_system_warning.avg_re_download_bytes_per_sec_threshold`.
    pub avg_re_download_bytes_per_sec_threshold: Option<u64>,
    /// A regex that controls which targets are opted into the vpn check.
    /// The corresponding buckconfig is `buck2_health_check.optin_vpn_check_targets_regex`.
    pub optin_vpn_check_targets_regex: Option<String>,
    /// Whether to enable the stable revision check.
    pub enable_stable_revision_check: Option<bool>,
    /// Run the health checks in a separate process.
    pub enable_health_check_process_isolation: Option<bool>,
}

impl SystemWarningConfig {
    pub fn from_config(config: &LegacyBuckConfig) -> buck2_error::Result<Self> {
        let memory_pressure_threshold_percent = config.parse(BuckconfigKeyRef {
            section: "buck2_system_warning",
            property: "memory_pressure_threshold_percent",
        })?;
        let remaining_disk_space_threshold_gb = config.parse(BuckconfigKeyRef {
            section: "buck2_system_warning",
            property: "remaining_disk_space_threshold_gb",
        })?;
        let min_re_download_bytes_threshold = config.parse(BuckconfigKeyRef {
            section: "buck2_system_warning",
            property: "min_re_download_bytes_threshold",
        })?;
        let avg_re_download_bytes_per_sec_threshold = config.parse(BuckconfigKeyRef {
            section: "buck2_system_warning",
            property: "avg_re_download_bytes_per_sec_threshold",
        })?;
        let optin_vpn_check_targets_regex = config.parse(BuckconfigKeyRef {
            section: "buck2_health_check",
            property: "optin_vpn_check_targets_regex",
        })?;
        let enable_stable_revision_check = config.parse(BuckconfigKeyRef {
            section: "buck2_health_check",
            property: "enable_stable_revision_check",
        })?;
        let enable_health_check_process_isolation = config.parse(BuckconfigKeyRef {
            section: "buck2_health_check",
            property: "enable_health_check_process_isolation",
        })?;
        Ok(Self {
            memory_pressure_threshold_percent,
            remaining_disk_space_threshold_gb,
            min_re_download_bytes_threshold,
            avg_re_download_bytes_per_sec_threshold,
            optin_vpn_check_targets_regex,
            enable_stable_revision_check,
            enable_health_check_process_isolation,
        })
    }

    pub fn serialize(&self) -> buck2_error::Result<String> {
        serde_json::to_string(&self).buck_error_context("Error serializing SystemWarningConfig")
    }

    pub fn deserialize(s: &str) -> buck2_error::Result<Self> {
        serde_json::from_str::<Self>(s)
            .buck_error_context("Error deserializing SystemWarningConfig")
    }
}

#[derive(Allocative, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceControlConfig {
    /// A config to determine if the resource control should be activated or not.
    /// The corresponding buckconfig is `buck2_resource_control.status` that can take
    /// one of `{off | if_available | required}`.
    pub status: ResourceControlStatus,
    /// If resource control is enabled, buck needs to get a cgroup to run in from somewhere - this is
    /// where.
    pub init: ResourceControlInit,
    /// Maximum allowed memory usage for all work buck2 manages.
    ///
    /// Accepts either a number of bytes or a percentage of the available resources.
    ///
    /// The corresponding buckconfig is `buck2_resource_control.memory_max`.
    pub memory_max: Option<String>,
    /// Like `memory_max`, but controls cgroupv2's `memory.high`
    ///
    /// The corresponding buckconfig is `buck2_resource_control.memory_high`.
    pub memory_high: Option<String>,
    /// A memory threshold that any action is allowed to allocate.
    pub memory_max_per_action: Option<String>,
    /// A memory threshold that any action is allowed to reach before being throttled.
    pub memory_high_per_action: Option<String>,
    /// Memory high limit for all actions.
    ///
    /// Mainly for testing purpose.
    pub memory_high_actions: Option<String>,
    /// Memory max limit for all actions.
    ///
    /// Mainly for testing purpose.
    pub memory_max_actions: Option<String>,
    /// Value buck2 writes to `memory.swap.max` on the actions-pool cgroup (the cgroup shared by all
    /// running actions); the kernel enforces it from there. Accepts a byte count or `max`.
    ///
    /// Setting it to `0` disables swap for actions, so once `memory_high_actions` /
    /// `memory_max_actions` is reached the kernel must reclaim by stalling the action rather than
    /// paging its pages out. That stall is what produces the sustained memory pressure the
    /// scheduler needs to trigger suspension.
    ///
    /// Mainly for testing purpose.
    #[serde(default)]
    pub memory_swap_max_actions: Option<String>,
    /// Enable suspension when memory pressure is high.
    pub enable_suspension: bool,
    pub experimental_suspension_algo_variant: Option<u8>,
    pub preferred_action_suspend_strategy: ActionSuspendStrategy,
}

impl ResourceControlConfig {
    pub fn testing_default() -> Self {
        Self::from_settings(&ResourceControl::default(), &LegacyBuckConfig::empty()).unwrap()
    }
}

#[derive(Allocative, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ActionSuspendStrategy {
    CgroupFreeze,
    KillAndRetry,
}

impl FromStr for ActionSuspendStrategy {
    type Err = buck2_error::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "kill_and_retry" => Ok(Self::KillAndRetry),
            "cgroup_freeze" => Ok(Self::CgroupFreeze),
            _ => Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "Invalid suspend strategy: `{}`",
                s
            )),
        }
    }
}

#[derive(
    Allocative,
    Clone,
    Copy,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq
)]
pub enum ResourceControlStatus {
    #[default]
    /// The resource is not controlled or limited.
    Off,
    /// The resource is controlled by `systemd` if it's available on the system, otherwise off.
    IfAvailable,
    /// The resource is controlled by `systemd`. If it is not available on the system,
    /// buck2 errors it out and the command returns with an error exit code.
    Required,
}

impl FromStr for ResourceControlStatus {
    type Err = buck2_error::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "off" => Ok(Self::Off),
            "if_available" => Ok(Self::IfAvailable),
            "required" => Ok(Self::Required),
            _ => Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "Invalid resource control status: `{}`",
                s
            )),
        }
    }
}

#[derive(Allocative, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResourceControlInit {
    Systemd,
    #[cfg(unix)]
    Cgroup(AbsNormPathBuf),
}

impl FromStr for ResourceControlInit {
    type Err = buck2_error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "systemd" {
            return Ok(ResourceControlInit::Systemd);
        }
        #[cfg(unix)]
        if let Some(p) = s.strip_prefix("cgroup:") {
            return Ok(ResourceControlInit::Cgroup(AbsNormPathBuf::from(
                p.to_owned(),
            )?));
        }
        Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "Unknown resource control initializer: `{}`",
            s
        ))
    }
}

/// The current version of the resource control algorithm. Say you have some important change to the
/// algo that fixes a bug. Incrementing this to `N + 1` and setting the
/// `buck2_resource_control.enable_suspension_if_min_algo_version` buckconfig to `N + 1` enables
/// suspension only if your bug fix is actually included in the version of buck in use
const RESOURCE_CONTROL_ALGO_VERSION: u32 = 6;

/// The current version of the daemon cgroup wrapping logic. Incrementing this to `N + 1` and
/// setting `buck2_resource_control.min_version_for_gated_status` buckconfig to `N + 1` enables the
/// gated default status (`buck2_resource_control.version_gated_default_status`, defaulting to
/// `if_available`) only if the bug fix is included in the version of buck in use.
const DAEMON_CGROUP_VERSION: u32 = 1;

impl ResourceControlConfig {
    pub fn from_settings(
        resource_control: &ResourceControl,
        config: &LegacyBuckConfig,
    ) -> buck2_error::Result<Self> {
        if let Some(env_conf) = buck2_env!(
            "BUCK2_TEST_RESOURCE_CONTROL_CONFIG",
            applicability = testing,
        )? {
            return Self::deserialize(env_conf);
        }
        let mut buckconfig = config;
        let status = match resource_control.status(&mut buckconfig)? {
            Some(status) => status.parse()?,
            None => {
                let gated = resource_control
                    .min_version_for_gated_status(&mut buckconfig)?
                    .is_some_and(|min_version| DAEMON_CGROUP_VERSION >= min_version);
                if gated {
                    match resource_control.version_gated_default_status(&mut buckconfig)? {
                        Some(status) => status.parse()?,
                        None => ResourceControlStatus::IfAvailable,
                    }
                } else {
                    ResourceControlStatus::Off
                }
            }
        };
        let init = match resource_control.init(&mut buckconfig)? {
            Some(init) => init.parse()?,
            None => ResourceControlInit::Systemd,
        };
        let enable_suspension = resource_control
            .enable_suspension(&mut buckconfig)?
            .unwrap_or(false)
            || resource_control
                .enable_suspension_if_min_algo_version(&mut buckconfig)?
                .is_some_and(|min_version| RESOURCE_CONTROL_ALGO_VERSION >= min_version);
        let preferred_action_suspend_strategy =
            match resource_control.preferred_action_suspend_strategy(&mut buckconfig)? {
                Some(strategy) => strategy.parse()?,
                None => ActionSuspendStrategy::KillAndRetry,
            };
        Ok(Self {
            status,
            init,
            memory_max: resource_control.memory_max(&mut buckconfig)?,
            memory_high: resource_control.memory_high(&mut buckconfig)?,
            memory_max_per_action: resource_control.memory_max_per_action(&mut buckconfig)?,
            memory_high_per_action: resource_control.memory_high_per_action(&mut buckconfig)?,
            memory_high_actions: resource_control.memory_high_actions(&mut buckconfig)?,
            memory_max_actions: resource_control.memory_max_actions(&mut buckconfig)?,
            memory_swap_max_actions: resource_control.memory_swap_max_actions(&mut buckconfig)?,
            enable_suspension,
            experimental_suspension_algo_variant: resource_control
                .experimental_suspension_algo_variant(&mut buckconfig)?,
            preferred_action_suspend_strategy,
        })
    }

    pub fn serialize(&self) -> buck2_error::Result<String> {
        serde_json::to_string(&self).buck_error_context("Error serializing ResourceControlConfig")
    }

    pub fn deserialize(s: &str) -> buck2_error::Result<Self> {
        serde_json::from_str::<Self>(s)
            .buck_error_context("Error deserializing ResourceControlConfig")
    }
}

#[derive(Allocative, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LogDownloadMethod {
    Manifold,
    Curl(String),
    None,
}

/// Pagable DICE storage settings, present (`Some`) only when paging is enabled by
/// the `hydration` Buck settings. `page_out_on_idle` also implies paging is
/// enabled. When present, the daemon
/// sets up on-disk storage during construction so `buck2 debug hydration` can
/// page node values out to / in from disk. Read at startup because it gates that
/// setup.
#[derive(Allocative, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HydrationConfig {
    /// On-disk backend for pagable storage.
    pub pagable_storage_backend: PagableStorageBackend,
    /// Automatically page the graph out to disk when the daemon goes idle.
    pub page_out_on_idle: bool,
    /// Idle page-out only runs when at least this many GiB of disk are free to
    /// write the paged-out values to.
    pub page_out_min_free_disk_gb: u64,
    /// Which values a page-out writes; see [`PageOutPolicy`].
    pub page_out_policy: PageOutPolicy,
    /// When the values a page-out writes leave memory; see [`EvictionPolicy`].
    pub eviction_policy: EvictionPolicy,
}

impl HydrationConfig {
    /// Returns `None` when the Buck settings do not enable paging.
    fn from_settings(settings: &DaemonBuckSettings) -> Option<Self> {
        let hydration = &settings.hydration;
        let page_out_on_idle = hydration.page_out_on_idle();
        if !(page_out_on_idle || hydration.enable_paging()) {
            return None;
        }
        Some(Self {
            pagable_storage_backend: hydration.pagable_storage_backend(),
            page_out_on_idle,
            page_out_min_free_disk_gb: hydration.page_out_min_free_disk_gb(),
            page_out_policy: hydration.page_out_policy(),
            eviction_policy: hydration.eviction_policy(),
        })
    }
}

/// Startup options and Buck Settings read by the client and passed to the daemon.
/// Reusing a daemon requires equal startup options and daemon-compatible Buck Settings.
///
/// The fields here are often raw String we get from the buckconfig, the daemon will do
/// deserialization once it receives them. That said, this is not a requirement.
///
/// Backwards compatibility on Serialize / Deserialize is not required: if the client cannot read
/// the DaemonStartupConfig provided by the daemon when it tries to connect, it will reject that
/// daemon and restart (and in fact it will probably not get that far since a version check is done
/// before parsing DaemonStartupConfig).
#[derive(Allocative, Clone, Debug, Serialize, Deserialize)]
pub struct DaemonStartupConfig {
    buck_settings: DaemonBuckSettings,
    // Keep the fields exposed by `buck2 status` at their existing JSON paths.
    #[serde(flatten)]
    pub startup_options: DaemonStartupOptions,
}

/// Effective startup values derived from buckconfig and Buck Settings.
/// Any difference requires a new daemon; Buck Settings have a separate compatibility check.
#[derive(Allocative, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonStartupOptions {
    /// Share one DICE engine across isolated tenant roots for this daemon's lifetime.
    pub share_dice: bool,
    pub num_tokio_workers: Option<usize>,
    pub daemon_buster: Option<String>,
    pub digest_algorithms: Option<String>,
    pub source_digest_algorithm: Option<String>,
    pub paranoid: bool,
    pub materializations: Option<String>,
    /// `buck2.materializer_v2_version`: the daemon runs the disk-state materializer when its
    /// `ROLLOUT_VERSION` is at least this, and the deferred one when this is unset.
    pub materializer_v2_version: Option<u32>,
    pub http: HttpConfig,
    pub resource_control: ResourceControlConfig,
    pub log_download_method: LogDownloadMethod,
    pub macos_qos_class: Option<MacosQosClass>,
    pub daemon_idle_timeout_s: Option<u64>,
    /// Pagable DICE storage settings, or `None` when paging is disabled.
    pub hydration: Option<HydrationConfig>,
}

impl DaemonStartupConfig {
    pub fn daemon_buck_settings(&self) -> &DaemonBuckSettings {
        &self.buck_settings
    }

    pub fn buck_settings_changes_from(&self, previous: &Self) -> Vec<BuckSettingChange> {
        self.buck_settings.changes_from(&previous.buck_settings)
    }

    pub fn new(
        config: &LegacyBuckConfig,
        settings: &DaemonBuckSettings,
        paranoid: bool,
    ) -> buck2_error::Result<Self> {
        // Resolved on the client, buckconfig fallbacks included, so the daemon
        // only ever sees the result.
        let mut buckconfig = config;

        let log_download_method = {
            let log_download = &settings.log_download;
            if log_download.log_use_manifold(&mut buckconfig)? {
                LogDownloadMethod::Manifold
            } else if let Some(log_url) = log_download.log_url(&mut buckconfig)? {
                if log_url.is_empty() {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "log_url is empty, but log_use_manifold is false"
                    ));
                }
                LogDownloadMethod::Curl(log_url)
            } else {
                LogDownloadMethod::None
            }
        };

        let startup_options = DaemonStartupOptions {
            share_dice: config
                .parse(BuckconfigKeyRef {
                    section: "buck2",
                    property: "share_dice",
                })?
                .unwrap_or(false),
            num_tokio_workers: settings.daemon.num_tokio_workers(&mut buckconfig)?,
            daemon_buster: settings.daemon.buster(&mut buckconfig)?,
            digest_algorithms: settings.digest.algorithms(&mut buckconfig)?,
            source_digest_algorithm: settings.digest.source_algorithm(&mut buckconfig)?,
            paranoid,
            materializations: settings.materializer.materializations(&mut buckconfig)?,
            materializer_v2_version: config.parse(BuckconfigKeyRef {
                section: "buck2",
                property: "materializer_v2_version",
            })?,
            http: HttpConfig::from_settings(&settings.http, config)?,
            resource_control: ResourceControlConfig::from_settings(
                &settings.resource_control,
                config,
            )?,
            log_download_method,
            macos_qos_class: {
                if buck2_env!("BUCK2_DISABLE_MACOS_QOS", bool)? {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "`BUCK2_DISABLE_MACOS_QOS` is not supported, set \
                         `daemon.macos_qos_class = \"skip_lowering\"` in `.bucksettings.toml` instead"
                    ));
                }
                settings.daemon.macos_qos_class(&mut buckconfig)?
            },
            daemon_idle_timeout_s: settings.daemon.idle_timeout_s(&mut buckconfig)?,
            hydration: HydrationConfig::from_settings(settings),
        };
        Ok(Self {
            buck_settings: settings.dupe(),
            startup_options,
        })
    }

    /// Returns the automatic idle page-out configuration for this daemon.
    /// Pagable storage may remain enabled even when idle page-out is out of scope.
    pub fn idle_page_out_config_for_isolation_dir(
        &self,
        isolation_dir: &FileName,
    ) -> Option<&HydrationConfig> {
        self.startup_options.hydration.as_ref().filter(|hydration| {
            hydration.page_out_on_idle
                && self
                    .buck_settings
                    .hydration
                    .page_out_on_idle_applies_to_isolation_dir(isolation_dir)
        })
    }

    pub fn serialize(&self) -> buck2_error::Result<String> {
        serde_json::to_string(&self).buck_error_context("Error serializing DaemonStartupConfig")
    }

    pub fn deserialize(s: &str) -> buck2_error::Result<Self> {
        serde_json::from_str::<Self>(s)
            .buck_error_context("Error deserializing DaemonStartupConfig")
    }

    pub fn testing_empty() -> Self {
        let startup_options = DaemonStartupOptions {
            share_dice: false,
            num_tokio_workers: None,
            daemon_buster: None,
            digest_algorithms: None,
            source_digest_algorithm: None,
            paranoid: false,
            materializations: None,
            materializer_v2_version: None,
            http: HttpConfig::default(),
            resource_control: ResourceControlConfig::testing_default(),
            log_download_method: if cfg!(fbcode_build) {
                LogDownloadMethod::Manifold
            } else {
                LogDownloadMethod::None
            },
            macos_qos_class: None,
            daemon_idle_timeout_s: None,
            hydration: None,
        };
        Self {
            buck_settings: DaemonBuckSettings::default(),
            startup_options,
        }
    }
}

#[cfg(test)]
mod tests {
    use buck2_settings::parser::resolve_setting_flags;
    use buck2_settings::parser::table;
    use indoc::indoc;

    use super::*;
    use crate::legacy_configs::configs::testing::parse;

    #[test]
    fn test_startup_config_serialization() -> buck2_error::Result<()> {
        let settings = resolve_setting_flags(vec![table(
            "[agent_materialization]\nenabled = true\n[analysis]\nrecord_requested_anon_targets = false",
        )])?;
        let mut config =
            DaemonStartupConfig::new(&LegacyBuckConfig::empty(), &settings.daemon, false)?;
        config.startup_options.daemon_buster = Some("test".to_owned());

        let serialized = config.serialize()?;
        let json: serde_json::Value = serde_json::from_str(&serialized)?;
        assert_eq!(json["daemon_buster"], "test");
        assert_eq!(
            json["buck_settings"]["analysis"]["record_requested_anon_targets"],
            false
        );
        assert!(json["buck_settings"].get("agent_materialization").is_none());
        assert!(json["buck_settings"].get("agent_interactive").is_none());
        assert!(json.get("startup_options").is_none());

        let deserialized = DaemonStartupConfig::deserialize(&serialized)?;
        assert_eq!(deserialized.startup_options, config.startup_options);
        assert_eq!(deserialized.daemon_buck_settings(), &settings.daemon);
        Ok(())
    }

    #[test]
    fn dice_sharing_is_a_daemon_startup_option() -> buck2_error::Result<()> {
        let empty = parse(&[("config", "")], "config")?;
        let control = DaemonStartupConfig::new(&empty, &DaemonBuckSettings::default(), false)?;
        assert!(!control.startup_options.share_dice);
        let configured = parse(&[("config", "[buck2]\nshare_dice = true")], "config")?;
        let treatment =
            DaemonStartupConfig::new(&configured, &DaemonBuckSettings::default(), false)?;
        assert!(treatment.startup_options.share_dice);
        assert_ne!(control.startup_options, treatment.startup_options);
        assert_eq!(
            treatment.startup_options,
            DaemonStartupConfig::deserialize(&treatment.serialize()?)?.startup_options
        );
        Ok(())
    }

    #[test]
    fn test_settings_win_over_buckconfig_fallbacks() -> buck2_error::Result<()> {
        let config = parse(
            &[(
                "config",
                indoc!(
                    r#"
                    [buck2]
                    daemon_buster = buckconfig
                    digest_algorithms = SHA1
                    source_digest_algorithm = SHA1
                    macos_qos_class = background
                    daemon_idle_timeout_s = 100
                    [build]
                    num_tokio_workers = 4
                    [http]
                    http2 = false
                    [buck2_resource_control]
                    memory_max = 50%
                    "#
                ),
            )],
            "config",
        )?;
        let fallback = DaemonStartupConfig::new(&config, &DaemonBuckSettings::default(), false)?;
        let options = &fallback.startup_options;
        assert_eq!(options.daemon_buster.as_deref(), Some("buckconfig"));
        assert_eq!(options.digest_algorithms.as_deref(), Some("SHA1"));
        assert_eq!(options.source_digest_algorithm.as_deref(), Some("SHA1"));
        assert_eq!(options.macos_qos_class, Some(MacosQosClass::Background));
        assert_eq!(options.daemon_idle_timeout_s, Some(100));
        assert_eq!(options.num_tokio_workers, Some(4));
        assert!(!options.http.http2);
        assert_eq!(options.resource_control.memory_max.as_deref(), Some("50%"));

        let settings = resolve_setting_flags(vec![table(indoc!(
            r#"
            [daemon]
            buster = "settings"
            idle_timeout_s = 60
            num_tokio_workers = 8
            macos_qos_class = "skip_lowering"
            [digest]
            algorithms = "BLAKE3-KEYED,SHA1"
            source_algorithm = "BLAKE3-KEYED"
            [http]
            http2 = true
            [resource_control]
            memory_max = "60%"
            "#
        ))])?;
        let from_settings = DaemonStartupConfig::new(&config, &settings.daemon, false)?;
        let options = &from_settings.startup_options;
        assert_eq!(options.daemon_buster.as_deref(), Some("settings"));
        assert_eq!(
            options.digest_algorithms.as_deref(),
            Some("BLAKE3-KEYED,SHA1")
        );
        assert_eq!(
            options.source_digest_algorithm.as_deref(),
            Some("BLAKE3-KEYED")
        );
        assert_eq!(options.macos_qos_class, Some(MacosQosClass::SkipLowering));
        assert_eq!(options.daemon_idle_timeout_s, Some(60));
        assert_eq!(options.num_tokio_workers, Some(8));
        assert!(options.http.http2);
        assert_eq!(options.resource_control.memory_max.as_deref(), Some("60%"));
        Ok(())
    }

    #[test]
    fn test_hydration_settings_enable_idle_page_out() -> buck2_error::Result<()> {
        let config = parse(&[("config", "")], "config")?;
        let settings = resolve_setting_flags(vec![table("[hydration]\npage_out_on_idle = true")])?;

        let startup_config = DaemonStartupConfig::new(&config, &settings.daemon, false)?;
        let hydration = startup_config
            .idle_page_out_config_for_isolation_dir(
                FileName::new("v2").expect("The default isolation dir should be valid"),
            )
            .expect("Idle page-out should enable hydration");
        assert!(hydration.page_out_on_idle);
        Ok(())
    }

    #[test]
    fn test_idle_page_out_scope_selects_isolation_dirs() -> buck2_error::Result<()> {
        let config = parse(&[("config", "")], "config")?;
        let settings = resolve_setting_flags(vec![table(
            "[hydration]\nenable_paging = true\npage_out_on_idle = true\npage_out_on_idle_isolation_dir_scope = \"non_default\"",
        )])?;
        let startup_config = DaemonStartupConfig::new(&config, &settings.daemon, false)?;
        assert_eq!(
            startup_config.idle_page_out_config_for_isolation_dir(
                FileName::new("v2").expect("The default isolation dir should be valid"),
            ),
            None,
        );
        let hydration = startup_config
            .idle_page_out_config_for_isolation_dir(
                FileName::new("custom").expect("The test isolation dir should be valid"),
            )
            .expect("Idle page-out should be enabled outside the default daemon");
        assert!(hydration.page_out_on_idle);
        Ok(())
    }

    #[test]
    fn test_hydration_settings_page_out_policy() -> buck2_error::Result<()> {
        let config = parse(&[("config", "")], "config")?;
        let settings = resolve_setting_flags(vec![table(
            "[hydration]\nenable_paging = true\npage_out_policy = \"once_per_key\"\neviction_policy = \"on_idle\"",
        )])?;

        let startup_config = DaemonStartupConfig::new(&config, &settings.daemon, false)?;
        let hydration = startup_config
            .startup_options
            .hydration
            .expect("enable_paging should enable pagable storage");
        assert_eq!(hydration.page_out_policy, PageOutPolicy::OncePerKey);
        assert_eq!(hydration.eviction_policy, EvictionPolicy::OnIdle);
        Ok(())
    }

    #[test]
    fn test_legacy_hydration_buckconfig_is_ignored() -> buck2_error::Result<()> {
        let config = parse(
            &[(
                "config",
                indoc!(
                    r#"
                    [buck2_hydration]
                    enable_paging = true
                    page_out_on_idle = true
                    "#
                ),
            )],
            "config",
        )?;

        let startup_config =
            DaemonStartupConfig::new(&config, &DaemonBuckSettings::default(), false)?;
        assert_eq!(
            startup_config.startup_options.hydration, None,
            "Only the `hydration` Buck settings enable paging"
        );
        Ok(())
    }

    #[test]
    fn test_non_default_idle_page_out_scope() -> buck2_error::Result<()> {
        let config = parse(&[("config", "")], "config")?;
        let settings = resolve_setting_flags(vec![table(
            "[hydration]\npage_out_on_idle = true\npage_out_on_idle_isolation_dir_scope = \"non_default\"",
        )])?;
        let startup_config = DaemonStartupConfig::new(&config, &settings.daemon, false)?;

        assert!(
            startup_config.startup_options.hydration.is_some(),
            "The isolation scope must not disable pagable storage"
        );
        assert_eq!(
            startup_config.idle_page_out_config_for_isolation_dir(
                FileName::new("v2").expect("The default isolation dir should be valid"),
            ),
            None,
            "The non-default scope must disable idle page-out for the default daemon"
        );
        let custom_hydration = startup_config
            .idle_page_out_config_for_isolation_dir(
                FileName::new("custom").expect("The test isolation dir should be valid"),
            )
            .expect("Idle page-out should remain enabled for another daemon");
        assert!(
            custom_hydration.page_out_on_idle,
            "The non-default scope must retain idle page-out for another daemon"
        );
        Ok(())
    }
}
