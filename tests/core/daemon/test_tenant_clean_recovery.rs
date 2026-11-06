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
use std::time::Duration;

use buck2_e2e::prelude::*;
use buck2_e2e::process::daemon_is_alive;
use buck2_e2e::process::wait_for_exit;
use serde_json::Value;

async fn wait_for_daemon_exit(pid: u32) {
    assert!(
        wait_for_exit(pid, Duration::from_secs(10)).await,
        "daemon pid {pid} is still alive"
    );
}

async fn exited_process_pid() -> anyhow::Result<u32> {
    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("cmd");
        command.args(["/c", "exit 0"]);
        command
    } else {
        tokio::process::Command::new("true")
    };
    let mut process = command.spawn()?;
    let pid = process.id().unwrap();
    assert!(process.wait().await?.success());
    assert!(!daemon_is_alive(pid));
    Ok(pid)
}

fn tenant_cache(buck: &Buck) -> PathBuf {
    buck.cwd().join("buck-out").join("v2").join("cache")
}

fn read_if_exists(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    Ok(if path.exists() {
        Some(std::fs::read(path)?)
    } else {
        None
    })
}

fn info_pid(info: &str) -> anyhow::Result<u32> {
    let info: Value = serde_json::from_str(info)?;
    Ok(u32::try_from(info["pid"].as_u64().unwrap())?)
}

async fn shared_clean_requires_repair_dead_daemon_corrupt_info(
    buck: &Buck,
    metadata: &str,
) -> anyhow::Result<()> {
    buck.targets(&["//:rule"]).await?;
    assert!(tenant_cache(buck).is_dir());
    let daemon_dir = buck.daemon_dir().await?;
    let info_path = daemon_dir.join("buckd.info");
    let original_info = std::fs::read_to_string(&info_path)?;
    let pid = info_pid(&original_info)?;
    buck.kill().await?;
    wait_for_daemon_exit(pid).await;
    let result = async {
        if metadata == "missing" {
            std::fs::remove_file(&info_path)?;
        } else {
            std::fs::write(&info_path, "not JSON")?;
        }
        let damaged_info = read_if_exists(&info_path)?;
        buck.clean(&[]).await.expect_failure();
        assert!(tenant_cache(buck).is_dir());
        assert!(!daemon_is_alive(pid));
        assert_eq!(read_if_exists(&info_path)?, damaged_info);
        anyhow::Ok(())
    }
    .await;
    std::fs::write(&info_path, &original_info)?;
    result?;
    buck.clean(&[]).await?;
    assert!(!tenant_cache(buck).exists());
    buck.targets(&["//:rule"]).await?;
    assert!(tenant_cache(buck).is_dir());
    Ok(())
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_shared_clean_requires_repair_dead_daemon_corrupt_info_missing(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_clean_requires_repair_dead_daemon_corrupt_info(buck, "missing").await
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_shared_clean_requires_repair_dead_daemon_corrupt_info_not_json(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_clean_requires_repair_dead_daemon_corrupt_info(buck, "not-json").await
}

async fn shared_clean_refuses_live_daemon_corrupt_info(
    buck: &Buck,
    metadata: &str,
    pid_file: &str,
) -> anyhow::Result<()> {
    buck.targets(&["//:rule"]).await?;
    assert!(tenant_cache(buck).is_dir());
    let daemon_dir = buck.daemon_dir().await?;
    let info_path = daemon_dir.join("buckd.info");
    let pid_path = daemon_dir.join("buckd.pid");
    let original_info = std::fs::read_to_string(&info_path)?;
    let original_pid = std::fs::read_to_string(&pid_path)?;
    let pid = info_pid(&original_info)?;
    let result = async {
        if metadata == "missing" {
            std::fs::remove_file(&info_path)?;
        } else if metadata == "not-json" {
            std::fs::write(&info_path, "not JSON")?;
        } else {
            let mut info: Value = serde_json::from_str(&original_info)?;
            info["pid"] = exited_process_pid().await?.into();
            info["endpoint"] = "tcp:127.0.0.1:0".into();
            std::fs::write(&info_path, serde_json::to_string(&info)?)?;
        }
        if pid_file == "exited" {
            std::fs::write(&pid_path, exited_process_pid().await?.to_string())?;
        }
        buck.clean(&[]).await.expect_failure();
        assert!(daemon_is_alive(pid));
        assert!(tenant_cache(buck).is_dir());
        anyhow::Ok(())
    }
    .await;
    std::fs::write(&info_path, &original_info)?;
    std::fs::write(&pid_path, &original_pid)?;
    let killed = buck.kill().await;
    result?;
    killed?;
    Ok(())
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_shared_clean_refuses_live_daemon_corrupt_info_unchanged_missing(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_clean_refuses_live_daemon_corrupt_info(buck, "missing", "unchanged").await
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_shared_clean_refuses_live_daemon_corrupt_info_unchanged_not_json(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_clean_refuses_live_daemon_corrupt_info(buck, "not-json", "unchanged").await
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_shared_clean_refuses_live_daemon_corrupt_info_unchanged_unreachable_dead_pid(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_clean_refuses_live_daemon_corrupt_info(buck, "unreachable-dead-pid", "unchanged").await
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_shared_clean_refuses_live_daemon_corrupt_info_exited_missing(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_clean_refuses_live_daemon_corrupt_info(buck, "missing", "exited").await
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_shared_clean_refuses_live_daemon_corrupt_info_exited_not_json(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_clean_refuses_live_daemon_corrupt_info(buck, "not-json", "exited").await
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_shared_clean_refuses_live_daemon_corrupt_info_exited_unreachable_dead_pid(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_clean_refuses_live_daemon_corrupt_info(buck, "unreachable-dead-pid", "exited").await
}
