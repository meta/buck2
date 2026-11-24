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
use buck2_fs::paths::file_name::FileName;
use buck2_settings_derive::buck_settings_section;
use buck2_wrapper_common::DEFAULT_ISOLATION_DIR;
use dice::EvictionPolicy;
use dice::EvictionStrategy;
use dice::PagableStorageBackend;
use dice::PageOutPolicy;
use serde::Deserialize;
use serde::Serialize;

/// Which daemons page out on idle, by isolation dir.
#[derive(
    Allocative,
    Clone,
    Copy,
    Debug,
    Default,
    Deserialize,
    Serialize,
    PartialEq,
    Eq
)]
#[serde(rename_all = "snake_case")]
pub enum PageOutOnIdleIsolationDirScope {
    NonDefault,
    #[default]
    All,
}

/// Paging the values of the daemon's dependency graph (DICE) out to disk,
/// and reading them back as commands need them.
// A bump of the version must ship with a `[hydration.<version>]` rollout
// stanza carrying every key of the previous one; this section is the one that
// has lost its rollout to a bump before.
#[buck_settings_section(name = "hydration", version = 4, scope = daemon)]
pub struct Hydration {
    /// Whether DICE values can be paged out to on-disk storage. Implied by
    /// `page_out_on_idle`.
    #[setting(
        internal_default = false,
        oss_default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    enable_paging: Option<bool>,
    /// Whether values read back from disk (registered actions, analysis
    /// results and, in Meta-internal builds, Starlark fields) are decoded
    /// when they are first used, instead of all at once.
    #[setting(
        internal_default = true,
        oss_default = true,
        overridable_in(CommandLine, LocalSettings)
    )]
    defer_field_reads: Option<bool>,
    /// Whether the daemon pages DICE out when it goes idle.
    #[setting(
        internal_default = false,
        oss_default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    page_out_on_idle: Option<bool>,
    /// Which daemons page out when idle: `all`, or `non_default` to leave the
    /// daemon of the default isolation directory alone.
    #[setting(
        internal_default = PageOutOnIdleIsolationDirScope::All,
        oss_default = PageOutOnIdleIsolationDirScope::All,
        overridable_in(CommandLine, LocalSettings)
    )]
    page_out_on_idle_isolation_dir_scope: Option<PageOutOnIdleIsolationDirScope>,
    /// Idle page-out runs only when at least this many GiB of disk are free.
    #[setting(
        internal_default = 100,
        oss_default = 100,
        overridable_in(CommandLine, LocalSettings)
    )]
    page_out_min_free_disk_gb: Option<u64>,
    /// Which values a page-out writes. `once_per_value` writes each value
    /// once, so a value recomputed or read back from disk stays backed by a
    /// row and can leave memory again without a write. `once_per_key` pages a
    /// key out at most once: a value recomputed or read back afterwards stays
    /// in memory.
    #[setting(
        internal_default = PageOutPolicy::OncePerValue,
        oss_default = PageOutPolicy::OncePerValue,
        overridable_in(CommandLine, LocalSettings)
    )]
    page_out_policy: Option<PageOutPolicy>,
    /// When the values a page-out writes leave memory. `on_idle` evicts them
    /// as they are written; `on_memory_pressure` keeps them in memory, backed
    /// by their rows, until the daemon sees memory pressure. `on_idle_lru`
    /// and `on_memory_pressure_lru` evict the values least recently used
    /// first, and `on_idle_lru` leaves writes in memory for a later idle
    /// reclaim.
    #[setting(
        internal_default = EvictionPolicy::OnMemoryPressure(EvictionStrategy::Default),
        oss_default = EvictionPolicy::OnMemoryPressure(EvictionStrategy::Default),
        overridable_in(CommandLine, LocalSettings)
    )]
    eviction_policy: Option<EvictionPolicy>,
    /// The on-disk backend for pagable DICE storage: `sqlite`, `sled`, or
    /// `noop`, which serializes values and discards them.
    #[setting(
        internal_default = PagableStorageBackend::Sqlite,
        oss_default = PagableStorageBackend::Sqlite,
        overridable_in(CommandLine, LocalSettings)
    )]
    pagable_storage_backend: Option<PagableStorageBackend>,
    /// Whether the SQLite backend reads a shard's key file into the page cache
    /// whole once key lookups in it have started going to disk.
    #[setting(
        internal_default = true,
        oss_default = true,
        overridable_in(CommandLine, LocalSettings)
    )]
    key_read_ahead: Option<bool>,
}

impl Hydration {
    pub fn page_out_on_idle_applies_to_isolation_dir(&self, isolation_dir: &FileName) -> bool {
        match self.page_out_on_idle_isolation_dir_scope() {
            PageOutOnIdleIsolationDirScope::NonDefault => {
                isolation_dir.as_str() != DEFAULT_ISOLATION_DIR
            }
            PageOutOnIdleIsolationDirScope::All => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BuckSettings;
    use crate::parser::resolve_setting_flags;
    use crate::parser::table;

    #[test]
    fn test_default_hydration_settings() {
        let hydration = BuckSettings::empty().daemon.hydration;
        assert!(!hydration.enable_paging());
        assert!(hydration.defer_field_reads());
        assert!(!hydration.page_out_on_idle());
        assert_eq!(hydration.page_out_min_free_disk_gb(), 100);
        assert_eq!(
            hydration.pagable_storage_backend(),
            PagableStorageBackend::Sqlite
        );
        assert_eq!(hydration.page_out_policy(), PageOutPolicy::OncePerValue);
        assert_eq!(
            hydration.eviction_policy(),
            EvictionPolicy::OnMemoryPressure(EvictionStrategy::Default)
        );
        assert!(hydration.key_read_ahead());
        assert!(
            hydration.page_out_on_idle_applies_to_isolation_dir(
                FileName::new(DEFAULT_ISOLATION_DIR)
                    .expect("The default isolation dir should be valid")
            )
        );
        assert!(hydration.page_out_on_idle_applies_to_isolation_dir(
            FileName::new("custom").expect("The test isolation dir should be valid")
        ));
    }

    #[test]
    fn test_hydration_settings() -> buck2_error::Result<()> {
        let settings = resolve_setting_flags(vec![table(
            "[hydration]\nenable_paging = true\ndefer_field_reads = true\npage_out_on_idle = false\npage_out_on_idle_isolation_dir_scope = \"non_default\"\npage_out_min_free_disk_gb = 0\npage_out_policy = \"once_per_key\"\neviction_policy = \"on_idle\"\npagable_storage_backend = \"sled\"\nkey_read_ahead = false",
        )])?;
        let hydration = &settings.daemon.hydration;
        assert!(hydration.enable_paging());
        assert!(hydration.defer_field_reads());
        assert!(!hydration.page_out_on_idle());
        assert_eq!(hydration.page_out_min_free_disk_gb(), 0);
        assert_eq!(
            hydration.pagable_storage_backend(),
            PagableStorageBackend::Sled
        );
        assert_eq!(hydration.page_out_policy(), PageOutPolicy::OncePerKey);
        assert_eq!(
            hydration.eviction_policy(),
            EvictionPolicy::OnIdle(EvictionStrategy::Default)
        );
        assert!(!hydration.key_read_ahead());
        assert!(
            !hydration.page_out_on_idle_applies_to_isolation_dir(
                FileName::new(DEFAULT_ISOLATION_DIR)
                    .expect("The default isolation dir should be valid")
            )
        );
        assert!(hydration.page_out_on_idle_applies_to_isolation_dir(
            FileName::new("custom").expect("The test isolation dir should be valid")
        ));
        Ok(())
    }

    #[test]
    fn eviction_policy_settings_preserve_names_and_strategies() -> buck2_error::Result<()> {
        for (name, expected) in [
            ("on_idle", EvictionPolicy::OnIdle(EvictionStrategy::Default)),
            (
                "on_memory_pressure",
                EvictionPolicy::OnMemoryPressure(EvictionStrategy::Default),
            ),
            ("on_idle_lru", EvictionPolicy::OnIdle(EvictionStrategy::Lru)),
            (
                "on_memory_pressure_lru",
                EvictionPolicy::OnMemoryPressure(EvictionStrategy::Lru),
            ),
        ] {
            let settings = resolve_setting_flags(vec![table(&format!(
                "[hydration]\neviction_policy = \"{name}\"",
            ))])?;
            let actual = settings.daemon.hydration.eviction_policy();
            assert_eq!(actual, expected);
            assert_eq!(serde_json::to_value(actual).unwrap(), name);
        }
        Ok(())
    }
}
