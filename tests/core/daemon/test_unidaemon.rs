/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Daemon sharing across checkouts: clients whose startup constraints agree join one daemon.

use std::path::Path;

use buck2_e2e::prelude::*;
use common_unidaemon::assert_daemon_exits;
use common_unidaemon::bool_cases;
use common_unidaemon::daemon_pid;
use common_unidaemon::isolated_buck;
use common_unidaemon::unidaemon_checkouts;
use regex::Regex;

/// Builds `//:tenant_value` in `tenant` and checks that the output holds `value` and lies in
/// the tenant's own checkout.
async fn assert_tenant_value(tenant: &Buck, value: &str) -> anyhow::Result<()> {
    let result = tenant.build(&["//:tenant_value"]).await?;
    let output = result
        .build_report()?
        .output_for_target("//:tenant_value")?;
    assert_eq!(std::fs::read_to_string(&output)?, value);
    assert!(output.starts_with(tenant.cwd()));
    Ok(())
}

fn namespace_keys(tmp_path: &Path) -> anyhow::Result<Vec<String>> {
    std::fs::read_dir(
        tmp_path
            .join("home")
            .join(".buck")
            .join("buckd")
            .join(".unidaemons"),
    )?
    .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
    .collect::<anyhow::Result<_>>()
}

async fn kill_all(tenants: &[Buck]) -> anyhow::Result<()> {
    for tenant in tenants {
        tenant.kill().await?;
    }
    Ok(())
}

async fn unidaemon_compatibility_discovery(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, share_dice)?;
    let result = async {
        let mut pids = Vec::new();
        for (tenant, value) in tenants.iter().zip(["one", "two"]) {
            assert_tenant_value(tenant, value).await?;
            pids.push(daemon_pid(tenant).await?);
        }
        assert_eq!(pids[0], pids[1]);
        let keys = namespace_keys(tmp_path)?;
        assert_eq!(keys.len(), 1, "{keys:?}");
        assert!(Regex::new("^[0-9a-f]{64}$")?.is_match(&keys[0]), "{keys:?}");
        tenants[0].kill().await?;
        tenants[1].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[1]).await?, pids[0]);
        tenants[0].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[0]).await?, pids[0]);
        anyhow::Ok(pids[0])
    }
    .await;
    let killed = kill_all(&tenants).await;
    let pid = result?;
    killed?;
    assert_daemon_exits(pid).await;
    Ok(())
}

bool_cases! { [buck, tmp_path] unidaemon_compatibility_discovery {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_compatibility_discovery_False(false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_compatibility_discovery_True(true);
}}

async fn unidaemon_incompatible_settings_select_another_process(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, share_dice)?;
    let config = tenants[1].cwd().join(".buckconfig");
    let original = std::fs::read_to_string(&config)?;
    let alternative = isolated_buck(&tenants[1], "other-mode");
    let result = async {
        for tenant in &tenants {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let pid = daemon_pid(&tenants[0]).await?;
        std::fs::write(
            &config,
            format!("{original}\n[buck2]\nshare_dice = {}\n", !share_dice),
        )?;
        alternative.build(&["//:tenant_value"]).await?;
        assert_ne!(daemon_pid(&alternative).await?, pid);
        assert_eq!(namespace_keys(tmp_path)?.len(), 2);
        // Killed while its configuration still names its namespace: a client finds the
        // tenant's daemon by hashing the configuration it reads.
        alternative.kill().await?;
        std::fs::write(&config, &original)?;
        tenants[0].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[0]).await?, pid);
        anyhow::Ok(pid)
    }
    .await;
    let killed = async {
        alternative.kill().await?;
        std::fs::write(&config, &original)?;
        kill_all(&tenants).await
    }
    .await;
    let pid = result?;
    killed?;
    assert_daemon_exits(pid).await;
    Ok(())
}

bool_cases! { [buck, tmp_path] unidaemon_incompatible_settings_select_another_process {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_incompatible_settings_select_another_process_False(false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_incompatible_settings_select_another_process_True(true);
}}
