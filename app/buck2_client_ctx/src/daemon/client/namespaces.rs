/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Housekeeping of the unidaemon namespace directories: a shared daemon that has exited leaves
//! its namespace directory behind, and clients remove such directories when they are safely
//! judged dead.

use std::time::Duration;

use buck2_common::daemon_compatibility::DaemonCompatibilityKey;
use buck2_common::daemon_dir::DaemonDir;
use buck2_common::invocation_paths::InvocationPaths;
use buck2_common::invocation_paths::UNIDAEMON_NAMESPACES_DIR;
use buck2_common::invocation_paths::UnidaemonLifetimeLock;
use buck2_error::BuckErrorOptionContext;
use buck2_fs::paths::file_name::FileName;

use crate::daemon::client::BuckdLifecycleLock;
use crate::daemon::client::connect::BuckdProcessInfo;
use crate::startup_deadline::StartupDeadline;

/// Removes the namespace directories of shared daemons that have exited, except `keep`.
/// Housekeeping: nothing the caller depends on is in them, so a namespace it cannot judge or
/// remove is left in place.
pub async fn prune_exited_unidaemon_namespaces(
    paths: &InvocationPaths,
    keep: Option<&DaemonCompatibilityKey>,
) {
    let Ok(root) = paths
        .roots
        .common_buckd_dir()
        .map(|root| root.join(FileName::unchecked_new(UNIDAEMON_NAMESPACES_DIR)))
    else {
        return;
    };
    let Ok(mut entries) = tokio::fs::read_dir(&root).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Some(key) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<DaemonCompatibilityKey>().ok())
        else {
            continue;
        };
        if keep == Some(&key) {
            continue;
        }
        if let Err(error) = prune_exited_unidaemon_namespace(paths, &key).await {
            tracing::debug!("Left unidaemon namespace `{key}` in place: {error:#}");
        }
    }
}

async fn prune_exited_unidaemon_namespace(
    paths: &InvocationPaths,
    key: &DaemonCompatibilityKey,
) -> buck2_error::Result<()> {
    let dir = paths.unidaemon_dir(key)?;
    if unidaemon_namespace_may_be_live(paths, key, &dir).await? {
        return Ok(());
    }
    let Ok(lifecycle) = BuckdLifecycleLock::lock_with_timeout(
        dir.clone(),
        StartupDeadline::duration_from_now(Duration::from_millis(100))?,
    )
    .await
    else {
        return Ok(());
    };
    // Starters hold the lifecycle lock, so asking again under it catches a daemon started since.
    if unidaemon_namespace_may_be_live(paths, key, &dir).await? {
        return Ok(());
    }
    remove_unidaemon_namespace_dir(&lifecycle).await
}

/// Removes the namespace daemon directory that `lifecycle` locks, and the namespace with it when
/// nothing else is left there. The caller has established that no daemon of the namespace is
/// running, or is that daemon on its way out.
pub async fn remove_unidaemon_namespace_dir(
    lifecycle: &BuckdLifecycleLock,
) -> buck2_error::Result<()> {
    lifecycle.clean_daemon_dir(false)?;
    let dir = lifecycle.daemon_dir();
    tokio::fs::remove_dir_all(&dir.path).await?;
    let _ignored = tokio::fs::remove_dir(
        dir.path
            .parent()
            .internal_error("Namespace has no parent")?,
    )
    .await;
    Ok(())
}

/// Whether a daemon may still run in the namespace: one holds its lifetime lock, or its record
/// names a process that may be running, as it does for a daemon of a release without the lock.
async fn unidaemon_namespace_may_be_live(
    paths: &InvocationPaths,
    key: &DaemonCompatibilityKey,
    dir: &DaemonDir,
) -> buck2_error::Result<bool> {
    if UnidaemonLifetimeLock::is_held(paths, key).await? {
        return Ok(true);
    }
    match BuckdProcessInfo::load_if_exists(dir) {
        Ok(Some(info)) => info.may_be_running(),
        Ok(None) => Ok(false),
        // A record that cannot be read is no evidence that its daemon is gone.
        Err(_) => Ok(true),
    }
}
