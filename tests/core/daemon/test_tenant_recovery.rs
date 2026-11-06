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

use buck2_e2e::BuckError;
use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;
use buck2_e2e::process::daemon_is_alive;
use buck2_e2e::process::wait_for_exit;
use serde_json::Value;

async fn wait_for_daemon_exit(pid: u32) {
    assert!(
        wait_for_exit(pid, Duration::from_secs(10)).await,
        "daemon pid {pid} is still alive after 10s"
    );
}

async fn invoke(buck: &Buck, command: &str) -> Result<BuckOutput, BuckError> {
    let args: &[&str] = match command {
        "targets" => &["targets", "//:rule"],
        "kill" => &["kill"],
        "clean" => &["clean"],
        "killall" => &["killall", "--repo"],
        _ => panic!("unknown command `{command}`"),
    };
    buck.command(args).await
}

fn damage_metadata(daemon_dir: &Path, metadata: &str) -> anyhow::Result<()> {
    let info_path = daemon_dir.join("buckd.info");
    if metadata == "missing-info" {
        std::fs::remove_file(&info_path)?;
    } else if metadata == "malformed-info" {
        std::fs::write(&info_path, "not JSON")?;
    } else {
        let mut info: Value = serde_json::from_str(&std::fs::read_to_string(&info_path)?)?;
        info["endpoint"] = "tcp:127.0.0.1:0".into();
        std::fs::write(&info_path, serde_json::to_string(&info)?)?;
        let pid_path = daemon_dir.join("buckd.pid");
        if metadata == "missing-pid" {
            std::fs::remove_file(&pid_path)?;
        } else {
            assert_eq!(metadata, "mismatched-pid");
            std::fs::write(&pid_path, (info["pid"].as_u64().unwrap() + 1).to_string())?;
        }
    }
    Ok(())
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

async fn shared_recovery_refuses_uncertain_metadata(
    buck: &Buck,
    command: &str,
    alive: bool,
    metadata: &str,
) -> anyhow::Result<()> {
    buck.targets(&["//:rule"]).await?;
    let daemon_dir = buck.daemon_dir().await?;
    let info_path = daemon_dir.join("buckd.info");
    let pid_path = daemon_dir.join("buckd.pid");
    let original_info = std::fs::read_to_string(&info_path)?;
    let original_pid = std::fs::read_to_string(&pid_path)?;
    let pid = info_pid(&original_info)?;
    let cache = buck.cwd().join("buck-out").join("v2").join("cache");
    assert!(cache.is_dir());
    if !alive {
        buck.kill().await?;
        wait_for_daemon_exit(pid).await;
    }
    let result = async {
        damage_metadata(&daemon_dir, metadata)?;
        let damaged_info = read_if_exists(&info_path)?;
        let damaged_pid = read_if_exists(&pid_path)?;
        invoke(buck, command).await.expect_failure();
        assert!(cache.is_dir());
        assert_eq!(daemon_is_alive(pid), alive);
        assert_eq!(read_if_exists(&info_path)?, damaged_info);
        assert_eq!(read_if_exists(&pid_path)?, damaged_pid);
        anyhow::Ok(())
    }
    .await;
    std::fs::write(&info_path, &original_info)?;
    std::fs::write(&pid_path, &original_pid)?;
    let cleaned = buck.clean(&[]).await;
    result?;
    cleaned?;
    assert!(!cache.exists());
    buck.targets(&["//:rule"]).await?;
    Ok(())
}

const REFUSES_UNCERTAIN_METADATA_ENV: [(&str, &str); 2] = [
    ("BUCK2_SHARE_DAEMON", "true"),
    ("BUCK2_TESTING_CHECKER_INTERVAL_SECONDS", "120"),
];

/// The cases of `test_shared_recovery_refuses_uncertain_metadata`, as `(command, alive, metadata)`.
macro_rules! refuses_uncertain_metadata_cases {
    ($(#[$attr:meta] async fn $name:ident => ($command:literal, $alive:literal, $metadata:literal);)*) => {
        $(
            #[$attr]
            #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
            async fn $name(buck: &Buck) -> anyhow::Result<()> {
                shared_recovery_refuses_uncertain_metadata(buck, $command, $alive, $metadata).await
            }
        )*
    };
}

refuses_uncertain_metadata_cases! {
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_info_False_targets
        => ("targets", false, "missing-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_info_False_kill
        => ("kill", false, "missing-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_info_False_clean
        => ("clean", false, "missing-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_info_False_killall
        => ("killall", false, "missing-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_info_True_targets
        => ("targets", true, "missing-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_info_True_kill
        => ("kill", true, "missing-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_info_True_clean
        => ("clean", true, "missing-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_info_True_killall
        => ("killall", true, "missing-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_malformed_info_False_targets
        => ("targets", false, "malformed-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_malformed_info_False_kill
        => ("kill", false, "malformed-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_malformed_info_False_clean
        => ("clean", false, "malformed-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_malformed_info_False_killall
        => ("killall", false, "malformed-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_malformed_info_True_targets
        => ("targets", true, "malformed-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_malformed_info_True_kill
        => ("kill", true, "malformed-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_malformed_info_True_clean
        => ("clean", true, "malformed-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_malformed_info_True_killall
        => ("killall", true, "malformed-info");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_pid_False_targets
        => ("targets", false, "missing-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_pid_False_kill
        => ("kill", false, "missing-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_pid_False_clean
        => ("clean", false, "missing-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_pid_False_killall
        => ("killall", false, "missing-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_pid_True_targets
        => ("targets", true, "missing-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_pid_True_kill
        => ("kill", true, "missing-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_pid_True_clean
        => ("clean", true, "missing-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_missing_pid_True_killall
        => ("killall", true, "missing-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_mismatched_pid_False_targets
        => ("targets", false, "mismatched-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_mismatched_pid_False_kill
        => ("kill", false, "mismatched-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_mismatched_pid_False_clean
        => ("clean", false, "mismatched-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_mismatched_pid_False_killall
        => ("killall", false, "mismatched-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_mismatched_pid_True_targets
        => ("targets", true, "mismatched-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_mismatched_pid_True_kill
        => ("kill", true, "mismatched-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_mismatched_pid_True_clean
        => ("clean", true, "mismatched-pid");
    #[buck_test(data_dir = "test_daemon_data", env = REFUSES_UNCERTAIN_METADATA_ENV)]
    async fn test_shared_recovery_refuses_uncertain_metadata_mismatched_pid_True_killall
        => ("killall", true, "mismatched-pid");
}

async fn shared_recovery_accepts_consistent_exited_metadata(
    buck: &Buck,
    command: &str,
) -> anyhow::Result<()> {
    buck.targets(&["//:rule"]).await?;
    let pid = status_pid(buck).await?;
    buck.kill().await?;
    wait_for_daemon_exit(pid).await;
    invoke(buck, command).await?;
    buck.targets(&["//:rule"]).await?;
    assert_ne!(status_pid(buck).await?, pid);
    Ok(())
}

async fn status_pid(buck: &Buck) -> anyhow::Result<u32> {
    let status: Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    Ok(u32::try_from(
        status["process_info"]["pid"].as_u64().unwrap(),
    )?)
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_shared_recovery_accepts_consistent_exited_metadata_targets(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_recovery_accepts_consistent_exited_metadata(buck, "targets").await
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_shared_recovery_accepts_consistent_exited_metadata_kill(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_recovery_accepts_consistent_exited_metadata(buck, "kill").await
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_shared_recovery_accepts_consistent_exited_metadata_clean(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_recovery_accepts_consistent_exited_metadata(buck, "clean").await
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_shared_recovery_accepts_consistent_exited_metadata_killall(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_recovery_accepts_consistent_exited_metadata(buck, "killall").await
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [
        ("BUCK2_SHARE_DAEMON", "true"),
        ("BUCK2_TESTING_CHECKER_INTERVAL_SECONDS", "120"),
    ],
)]
async fn test_killall_repo_surfaces_inactivity_guidance(buck: &Buck) -> anyhow::Result<()> {
    let mut restore: Option<(PathBuf, String)> = None;
    let result = async {
        buck.targets(&["//:rule"]).await?;
        let daemon_dir = buck.daemon_dir().await?;
        let info_path = daemon_dir.join("buckd.info");
        let pid_path = daemon_dir.join("buckd.pid");
        let original_info = std::fs::read_to_string(&info_path)?;
        restore = Some((daemon_dir.clone(), original_info.clone()));
        let original_pid = std::fs::read(&pid_path)?;
        let pid = info_pid(&original_info)?;
        damage_metadata(&daemon_dir, "malformed-info")?;
        let failure = invoke(buck, "killall").await.expect_failure();
        failure.assert_stderr_matches("Cannot verify shared daemon inactivity");
        assert!(
            failure
                .stderr
                .contains("restore consistent buckd.info and buckd.pid before retrying")
        );
        assert_eq!(std::fs::read(&pid_path)?, original_pid);
        assert!(daemon_is_alive(pid));
        anyhow::Ok(())
    }
    .await;
    if let Some((daemon_dir, original_info)) = &restore {
        std::fs::write(daemon_dir.join("buckd.info"), original_info)?;
    }
    let cleaned = buck.clean(&[]).await;
    result?;
    cleaned?;
    Ok(())
}
