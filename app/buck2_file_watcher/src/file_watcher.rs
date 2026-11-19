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
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_core::buck2_env;
use buck2_core::fs::project::ProjectRoot;
#[cfg(fbcode_build)]
use buck2_core::soft_error;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use dice::DiceTransactionUpdater;
use dupe::Dupe;

use crate::dep_files::DepFileCache;
#[cfg(fbcode_build)]
use crate::edenfs::interface::EdenFsFileWatcher;
use crate::fs_hash_crawler::FsHashCrawler;
use crate::ignores::FileWatcherIgnores;
use crate::mergebase::Mergebase;
use crate::notify::NotifyFileWatcher;
use crate::watchman::interface::WatchmanFileWatcher;

#[async_trait]
pub trait FileWatcher: Allocative + Send + Sync + 'static {
    /// Records what changed since the previous sync on `dice`.
    async fn sync(
        &self,
        dice: DiceTransactionUpdater,
    ) -> buck2_error::Result<(DiceTransactionUpdater, Mergebase, SyncOutcome)>;
}

/// Whether a sync accounted for everything that changed since the previous one.
#[derive(Copy, Clone, Dupe, Debug, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Every change since the previous sync was recorded on the updater.
    Incremental,
    /// The watcher lost track of what changed, so nothing computed before the sync may be
    /// reused.
    FreshInstance,
}

impl SyncOutcome {
    /// The outcome `stats` report.
    pub fn reported_by(stats: &buck2_data::FileWatcherStats) -> Self {
        if stats.fresh_instance {
            Self::FreshInstance
        } else {
            Self::Incremental
        }
    }
}

/// Parse the `dice_clear_on_mergebase_change` config, honoring both the buckconfig
/// and the `BUCK2_TEST_SKIP_DICE_CLEAR_ON_MERGEBASE_CHANGE` env var override.
pub(crate) fn dice_clear_on_mergebase_change(
    root_config: &LegacyBuckConfig,
) -> buck2_error::Result<bool> {
    let config_value = root_config
        .parse::<bool>(BuckconfigKeyRef {
            section: "buck2",
            property: "dice_clear_on_mergebase_change",
        })
        .buck_error_context("Failed to parse dice_clear_on_mergebase_change config")?
        .unwrap_or(true);
    let env_skip = buck2_env!(
        "BUCK2_TEST_SKIP_DICE_CLEAR_ON_MERGEBASE_CHANGE",
        bool,
        applicability = testing
    )
    .buck_error_context("Failed to parse BUCK2_TEST_SKIP_DICE_CLEAR_ON_MERGEBASE_CHANGE env")?;
    Ok(config_value && !env_skip)
}

impl dyn FileWatcher {
    /// Create a new FileWatcher. Note that this is not async, since it's called during daemon
    /// startup and shouldn't be doing any work that could warrant suspending.
    pub fn new(
        fb: fbinit::FacebookInit,
        project_root: &ProjectRoot,
        root_config: &LegacyBuckConfig,
        ignores: FileWatcherIgnores,
        dep_file_cache: Arc<dyn DepFileCache>,
    ) -> buck2_error::Result<Arc<dyn FileWatcher>> {
        if !project_root.root().as_path().exists() {
            return Err(buck2_error!(
                ErrorTag::MissingProjectRoot,
                "Project root `{}` does not exist. \
                 The directory may have been removed.",
                project_root.root()
            ));
        }

        #[cfg(fbcode_build)]
        let default = if detect_eden::is_eden(project_root.root().to_path_buf())? {
            "edenfs"
        } else {
            "watchman"
        };

        #[cfg(not(fbcode_build))]
        let default = "notify";

        let _allow_unused = fb;

        let watcher_conf = root_config
            .get(BuckconfigKeyRef {
                section: "buck2",
                property: "file_watcher",
            })
            .unwrap_or(default);

        let watcher_conf = if let "edenfs" = watcher_conf {
            #[cfg(fbcode_build)]
            match EdenFsFileWatcher::new(
                fb,
                project_root,
                root_config,
                ignores.clone(),
                dep_file_cache.clone(),
            ) {
                Ok(edenfs) => return Ok(Arc::new(edenfs)),
                Err(e) if e.has_tag(ErrorTag::IoNotConnected) => {
                    soft_error!("edenfs_watcher_creation_failure", e);
                    // fallback to watchman if failed to create edenfs watcher
                    "watchman"
                }
                Err(e) => return Err(e),
            }
            #[cfg(not(fbcode_build))]
            default
        } else {
            watcher_conf
        };

        match watcher_conf {
            "watchman" => Ok(Arc::new(
                WatchmanFileWatcher::new(project_root.root(), root_config, ignores, dep_file_cache)
                    .buck_error_context("Creating watchman file watcher")?,
            )),
            "notify" => Ok(Arc::new(
                NotifyFileWatcher::new(project_root, ignores)
                    .buck_error_context("Creating notify file watcher")?,
            )),
            "fs_hash_crawler" => Ok(Arc::new(
                FsHashCrawler::new(project_root, ignores)
                    .buck_error_context("Creating fs_crawler file watcher")?,
            )),
            other => Err(buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "Invalid buck2.file_watcher: {}",
                other
            )),
        }
    }
}
