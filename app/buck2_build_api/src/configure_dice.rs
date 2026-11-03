/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use buck2_common::dice::cells::SetCellResolver;
use buck2_common::dice::data::SetIoProvider;
use buck2_common::ignores::ignore_set::IgnoreSet;
use buck2_common::ignores::ignore_set::SetIgnoreSet;
use buck2_common::io::IoProvider;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::legacy_configs::dice::SetLegacyConfigs;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_common::tenting::SetTentingAclProvider;
use buck2_common::tenting::TentingAclProvider;
use buck2_core::rollout_percentage::RolloutPercentage;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::digest_config::SetDigestConfig;
use buck2_settings::DaemonBuckSettings;
use buck2_settings::DiceBuckSettings;
use buck2_settings::dice::SetBuckSettings;
use buck2_settings::dice::SetDiceBuckSettings;
use dice::BranchId;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceData;
use dice::DiceDataBuilder;
use dice::DiceStorage;
use dice::EvictionPolicy;
use dice::PagableStorageBackend;
use dice::PageOutPolicy;
use starlark::pagable::DeferredFieldReadsEnabled;

use crate::actions::execute::dice_data::SetInvalidationTrackingConfig;
use crate::build::detailed_aggregated_metrics::dice::SetDetailedAggregatedMetricsHandle;
use crate::build::detailed_aggregated_metrics::events::DetailedAggregatedMetricsHandle;

/// Configuration which is fixed for the lifetime of a DICE engine.
///
/// Every repository root attached to a shared engine must resolve to the same
/// values. Root-specific facilities belong in [`DiceData`]; inputs that affect
/// cached values must be tracked through injected keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiceEngineConfiguration {
    dice_state_path: Option<PathBuf>,
    pagable_storage_backend: PagableStorageBackend,
    page_out_policy: PageOutPolicy,
    eviction_policy: EvictionPolicy,
    defer_field_reads: bool,
}

/// Inputs used to resolve the settings shared by every root in an engine.
pub struct DiceEngineConfigurationInputs<'a> {
    pub dice_state_path: Option<&'a Path>,
    pub pagable_storage_backend: PagableStorageBackend,
    pub page_out_policy: PageOutPolicy,
    pub eviction_policy: EvictionPolicy,
    pub defer_field_reads: bool,
}

impl DiceEngineConfiguration {
    /// Resolve configuration and environment overrides before opening an engine.
    pub fn resolve(inputs: DiceEngineConfigurationInputs<'_>) -> buck2_error::Result<Self> {
        let DiceEngineConfigurationInputs {
            dice_state_path,
            pagable_storage_backend,
            page_out_policy,
            eviction_policy,
            defer_field_reads,
        } = inputs;
        let dice_state_path = match std::env::var_os("BUCK2_DICE_DB_PATH") {
            Some(path) => Some(PathBuf::from(path)),
            None => dice_state_path.map(Path::to_path_buf),
        };
        // Backend overrides and deferred reads apply only when paging is enabled.
        let (pagable_storage_backend, defer_field_reads) = if dice_state_path.is_some() {
            (
                pagable_storage_backend.with_env_override().map_err(|e| {
                    buck2_error::conversion::from_any_with_tag(
                        e,
                        buck2_error::ErrorTag::Environment,
                    )
                })?,
                defer_field_reads,
            )
        } else {
            (PagableStorageBackend::default(), false)
        };

        Ok(Self {
            page_out_policy: if dice_state_path.is_some() {
                page_out_policy
            } else {
                PageOutPolicy::default()
            },
            eviction_policy: if dice_state_path.is_some() {
                eviction_policy
            } else {
                EvictionPolicy::default()
            },
            dice_state_path,
            pagable_storage_backend,
            defer_field_reads,
        })
    }

    /// Resolved paging path, including any environment override.
    pub fn dice_state_path(&self) -> Option<&Path> {
        self.dice_state_path.as_deref()
    }

    /// Open storage and construct an engine with default root data.
    pub fn build(&self, data: DiceData) -> buck2_error::Result<Arc<Dice>> {
        let mut dice = DiceDataBuilder::from(data);

        // Opt-in pagable storage, enabling `Dice::page_out()` to serialize node
        // values to disk (backend chosen by `pagable_storage_backend`) via `buck2
        // debug hydration`. `dice_state_path` is `Some` when
        // `hydration.enable_paging` is set (its value is the default path,
        // under buck-out). The `BUCK2_DICE_DB_PATH` override (used by benchmarks)
        // takes precedence and picks the path.
        if let Some(path) = &self.dice_state_path {
            let mut storage =
                DiceStorage::open(path, self.pagable_storage_backend).map_err(|e| {
                    buck2_error::conversion::from_any_with_tag(
                        e,
                        buck2_error::ErrorTag::Environment,
                    )
                })?;
            storage.set_page_out_policy(self.page_out_policy);
            storage.set_eviction_policy(self.eviction_policy);
            if self.defer_field_reads {
                storage
                    .storage_context()
                    .get_or_init(|| DeferredFieldReadsEnabled);
            }
            dice.set_pagable_storage(storage);
        }

        Ok(dice.build(DetectCycles::Enabled))
    }

    /// Reject a root whose settings would change an already-running engine.
    pub fn ensure_compatible(&self, other: &Self) -> buck2_error::Result<()> {
        if self != other {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "repository root requested DICE engine configuration {other:?}, but the daemon is already using {self:?}"
            ));
        }
        Ok(())
    }
}

/// Repository inputs attached to a DICE root.
pub struct DiceRootConfiguration<'a> {
    pub io: Arc<dyn IoProvider>,
    pub digest_config: DigestConfig,
    pub ignore_set: Arc<IgnoreSet>,
    pub root_config: Option<&'a LegacyBuckConfig>,
    pub tenting_acl_provider: Option<Arc<dyn TentingAclProvider>>,
    pub buck_settings: DaemonBuckSettings,
}

/// Construct the immutable data associated with one repository root tree.
pub fn configure_dice_data_for_buck(
    configuration: DiceRootConfiguration<'_>,
) -> buck2_error::Result<DiceData> {
    let DiceRootConfiguration {
        io,
        digest_config,
        ignore_set,
        root_config,
        tenting_acl_provider,
        buck_settings,
    } = configuration;
    let mut data = DiceData::new();
    data.set_io_provider(io);
    data.set_digest_config(digest_config);
    data.set_ignore_set(ignore_set);
    data.set_buck_settings(buck_settings);
    data.set_tenting_acl_provider(tenting_acl_provider);
    let invalidation_tracking_enabled = match root_config {
        Some(c) => c
            .parse::<RolloutPercentage>(BuckconfigKeyRef {
                section: "buck2",
                property: "invalidation_tracking_enabled",
            })?
            .is_some_and(|v| v.roll()),
        None => false,
    };
    data.set_invalidation_tracking_config(invalidation_tracking_enabled);

    // Empty handle; a command enables the tracker lazily if it needs one.
    data.set_detailed_aggregated_metrics_handle(DetailedAggregatedMetricsHandle::new());
    Ok(data)
}

/// Configure engine-global DICE facilities around one initial root's data.
pub async fn configure_dice_for_buck(
    io: Arc<dyn IoProvider>,
    digest_config: DigestConfig,
    ignore_set: Arc<IgnoreSet>,
    root_config: Option<&LegacyBuckConfig>,
    tenting_acl_provider: Option<Arc<dyn TentingAclProvider>>,
    buck_settings: DaemonBuckSettings,
    // Path to open pagable DICE storage at, or `None` to leave paging disabled.
    dice_state_path: Option<&Path>,
    // On-disk backend for pagable storage (`hydration.pagable_storage_backend`).
    pagable_storage_backend: PagableStorageBackend,
    // Which values a page-out writes (`hydration.page_out_policy`).
    page_out_policy: PageOutPolicy,
    // When the values a page-out writes leave memory (`hydration.eviction_policy`).
    eviction_policy: EvictionPolicy,
    defer_field_reads: bool,
) -> buck2_error::Result<(Arc<Dice>, BranchId)> {
    let engine_config = DiceEngineConfiguration::resolve(DiceEngineConfigurationInputs {
        dice_state_path,
        pagable_storage_backend,
        page_out_policy,
        eviction_policy,
        defer_field_reads,
    })?;

    let data = configure_dice_data_for_buck(DiceRootConfiguration {
        io,
        digest_config,
        ignore_set,
        root_config,
        tenting_acl_provider,
        buck_settings,
    })?;
    let dice = engine_config.build(data)?;
    let root = dice.new_root().await;
    let mut dice_ctx = dice.updater(root);
    dice_ctx.set_none_cell_resolver()?;
    dice_ctx.set_none_legacy_config_external_data()?;
    // Defaults until the first command injects its own, so that nothing
    // computes against an uninjected key.
    dice_ctx.set_dice_buck_settings(DiceBuckSettings::default())?;
    dice_ctx.commit().await;

    Ok((dice, root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> DiceEngineConfiguration {
        DiceEngineConfiguration {
            dice_state_path: Some(PathBuf::from("dice-state")),
            pagable_storage_backend: PagableStorageBackend::Sqlite,
            page_out_policy: PageOutPolicy::default(),
            eviction_policy: EvictionPolicy::default(),
            defer_field_reads: false,
        }
    }

    #[test]
    fn engine_configuration_compatibility() {
        let expected = config();
        expected.ensure_compatible(&config()).unwrap();

        let mut different_path = config();
        different_path.dice_state_path = Some(PathBuf::from("other-dice-state"));
        assert!(expected.ensure_compatible(&different_path).is_err());

        let mut different_backend = config();
        different_backend.pagable_storage_backend = PagableStorageBackend::Sled;
        assert!(expected.ensure_compatible(&different_backend).is_err());

        let mut different_page_out_policy = config();
        different_page_out_policy.page_out_policy = PageOutPolicy::OncePerKey;
        assert!(
            expected
                .ensure_compatible(&different_page_out_policy)
                .is_err()
        );

        let mut different_eviction_policy = config();
        different_eviction_policy.eviction_policy = EvictionPolicy::OnIdle;
        assert!(
            expected
                .ensure_compatible(&different_eviction_policy)
                .is_err()
        );

        let mut different_deferred_reads = config();
        different_deferred_reads.defer_field_reads = true;
        assert!(
            expected
                .ensure_compatible(&different_deferred_reads)
                .is_err()
        );
    }
}
