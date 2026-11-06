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

use buck2_e2e::prelude::*;

fn tenants(buck: &Buck) -> Vec<Buck> {
    ["tenant-one", "tenant-two"]
        .into_iter()
        .map(|isolation| {
            let mut tenant = buck.clone();
            tenant.set_isolation_prefix(isolation);
            tenant
        })
        .collect()
}

async fn kill_tenants(tenants: &[Buck]) {
    for tenant in tenants {
        let _ignored = tenant.kill().await;
    }
}

/// Whether the test runs as root, as `id -u` says.
fn is_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "0")
}

#[cfg(unix)]
fn mode(path: &Path) -> anyhow::Result<u32> {
    use std::os::unix::fs::PermissionsExt;
    Ok(std::fs::metadata(path)?.permissions().mode() & 0o7777)
}

#[cfg(unix)]
fn chmod(path: &Path, mode: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn mode(_path: &Path) -> anyhow::Result<u32> {
    anyhow::bail!("No file modes on this platform")
}

#[cfg(not(unix))]
fn chmod(_path: &Path, _mode: u32) -> anyhow::Result<()> {
    anyhow::bail!("No file modes on this platform")
}

#[buck_test(
    data_dir = "test_daemon_data",
    skip_for_os = ["windows"],
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_killall_repo_reports_unreadable_daemon_root(buck: &Buck) -> anyhow::Result<()> {
    if is_root() {
        eprintln!("Skipped: unreadable-root killall assertions require a non-root user");
        return Ok(());
    }
    let tenants = tenants(buck);
    let mut project_dir: Option<PathBuf> = None;
    let mut original_mode: Option<u32> = None;
    let result = async {
        for tenant in &tenants {
            tenant.targets(&["//:rule"]).await?;
        }
        let dir = tenants[0].daemon_dir().await?.parent().unwrap().to_owned();
        let dir = project_dir.insert(dir);
        let restore = *original_mode.insert(mode(dir)?);
        chmod(dir, 0o000)?;
        let failure = buck.command(&["killall", "--repo"]).await;
        chmod(dir, restore)?;
        let failure = failure.expect_failure();
        failure.assert_stderr_matches("Repository tenant eviction failed");
        assert!(failure.stderr.contains("Evicted 0 tenant(s)"));
        assert!(failure.stderr.contains("failed for 1 daemon(s)"));
        assert!(
            failure
                .stderr
                .contains("Failed to inspect daemon directory")
        );
        for tenant in &tenants {
            assert!(
                !tenant
                    .status(&[])
                    .await?
                    .stderr
                    .contains("no buckd running")
            );
            tenant.targets(&["//:rule"]).await?;
        }
        anyhow::Ok(())
    }
    .await;
    if let (Some(project_dir), Some(original_mode)) = (&project_dir, original_mode) {
        chmod(project_dir, original_mode)?;
    }
    kill_tenants(&tenants).await;
    result
}
