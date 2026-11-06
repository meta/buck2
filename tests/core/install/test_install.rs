/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::read_timestamps;

/// Copy pre-built installer binaries into the sandbox project directory.
fn setup_sandbox(buck: &Buck) -> anyhow::Result<()> {
    for (var, name) in [
        ("INSTALLER_BIN", "installer_bin"),
        (
            "FORWARDED_PARAMS_INSTALLER_BIN",
            "forwarded_params_installer_bin",
        ),
        ("EXTRA_ARGS_VALIDATOR_BIN", "extra_args_validator_bin"),
        ("EARLY_EXIT_INSTALLER_BIN", "early_exit_installer_bin"),
    ] {
        let dst = buck.cwd().join(name);
        std::fs::copy(std::env::var(var)?, &dst)?;
        std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755))?;
    }

    // Create etc_hosts as a symlink to /etc/hosts (tests symlink resolution by rsync -aL)
    std::os::unix::fs::symlink("/etc/hosts", buck.cwd().join("etc_hosts"))?;
    Ok(())
}

/// A fresh `install_test` directory in `tmp_path`, and the installer arguments that name it.
fn install_dir(tmp_path: &Path) -> anyhow::Result<(std::path::PathBuf, String)> {
    let tmp_dir = tmp_path.join("install_test");
    std::fs::create_dir(&tmp_dir)?;
    let dst = format!("{}/", tmp_dir.display());
    Ok((tmp_dir, dst))
}

fn errors(record: &serde_json::Value) -> &[serde_json::Value] {
    record["errors"].as_array().map_or(&[], Vec::as_slice)
}

#[buck_test]
async fn test_success_install(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    setup_sandbox(buck)?;
    let (tmp_dir, dst) = install_dir(tmp_path)?;
    buck.install(&["root//:installer_test", "--", "--dst", &dst])
        .await?;
    assert!(tmp_dir.join("artifact_a").exists());
    assert!(tmp_dir.join("artifact_b").exists());
    assert!(tmp_dir.join("etc_hosts").exists());
    assert!(!tmp_dir.join("etc_hosts").is_symlink());
    Ok(())
}

#[buck_test(
    write_invocation_record = true,
    env = [("BUCK_LOG", "buck2_server_commands::commands::install=debug")],
)]
async fn test_install_logging(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    setup_sandbox(buck)?;
    let (_tmp_dir, dst) = install_dir(tmp_path)?;
    let res = buck
        .install(&["root//:installer_test", "--", "--dst", &dst, "--delay", "1"])
        .await?;
    let invocation_record = res.invocation_record()?;

    let cmd_start_ts =
        read_timestamps(buck, &["Event", "data", "SpanStart", "data", "Command"]).await?[0];
    let action_end_timestamps = read_timestamps(
        buck,
        &["Event", "data", "SpanEnd", "data", "ActionExecution"],
    )
    .await?;

    let install_duration_ms = invocation_record["install_duration_us"]
        .as_f64()
        .expect("an install duration")
        / 1000.0;
    let cmd_duration_ms = invocation_record["command_duration_us"]
        .as_f64()
        .expect("a command duration")
        / 1000.0;

    // Check that installing takes at least as long as the added delay.
    assert!(install_duration_ms > 1.0 * 1000.0);
    // Check that the we aren't double counting any time between install and
    // building the last action (only meaningful when there are build actions).
    if let Some(&last_action_end_ts) = action_end_timestamps.last() {
        let time_to_last_action_ms = last_action_end_ts as f64 - cmd_start_ts as f64;
        assert!(time_to_last_action_ms + install_duration_ms < cmd_duration_ms);
    }

    assert_eq!(
        invocation_record["install_device_metadata"],
        serde_json::json!([{"entry": [{"key": "version", "value": "1"}]}])
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_install_logs_target_rule_type_names(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    setup_sandbox(buck)?;
    let (_tmp_dir, dst) = install_dir(tmp_path)?;
    let res = buck
        .install(&["root//:installer_test", "--", "--dst", &dst])
        .await?;
    let record = res.invocation_record()?;
    assert_eq!(
        record["target_rule_type_names"],
        serde_json::json!(["installer"])
    );
    Ok(())
}

#[buck_test(env = [("BUCK2_INSTALLER_SEND_TIMEOUT_S", "1")])]
async fn test_send_file_timeout(buck: &Buck, _tmp_path: &Path) -> anyhow::Result<()> {
    setup_sandbox(buck)?;
    buck.install(&["root//:installer_single_artifact", "--", "--delay", "30"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"Timed out after 1s waiting for installer to process artifact_a");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_artifact_fails_to_install(buck: &Buck) -> anyhow::Result<()> {
    setup_sandbox(buck)?;
    let res = buck
        .install(&["root//:installer_server_sends_error"])
        .await
        .expect_failure();
    res.assert_stderr_matches(r"Interaction with installer failed");
    let record = res.invocation_record()?;
    let errors = errors(&record);
    assert_eq!(errors.len(), 1);
    let error = &errors[0];
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Mocking failing to install")
    );
    assert_eq!(error["category"], "INFRA");
    assert!(
        error["category_key"]
            .as_str()
            .unwrap_or_default()
            .contains("INSTALLER_TAG")
    );

    let install_duration_ms = record["install_duration_us"]
        .as_f64()
        .expect("an install duration")
        / 1000.0;

    assert!(install_duration_ms > 0.0);
    assert_eq!(
        record["install_device_metadata"],
        serde_json::json!([{"entry": [{"key": "version", "value": "1"}]}])
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_fail_to_build_artifact(buck: &Buck) -> anyhow::Result<()> {
    setup_sandbox(buck)?;
    let res = buck
        .install(&["root//:bad_artifacts"])
        .await
        .expect_failure();
    res.assert_stderr_matches(r"Failed to build");
    let record = res.invocation_record()?;
    assert_eq!(errors(&record).len(), 1);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_install_id_mismatch(buck: &Buck) -> anyhow::Result<()> {
    setup_sandbox(buck)?;
    let res = buck
        .install(&["root//:installer_server_sends_wrong_install_info_response"])
        .await
        .expect_failure();
    res.assert_stderr_matches(r"doesn't match with the sent one");
    let record = res.invocation_record()?;
    assert_eq!(errors(&record).len(), 1);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_installer_needs_forwarded_params(buck: &Buck) -> anyhow::Result<()> {
    setup_sandbox(buck)?;
    let res = buck
        .install(&["root//:installer_server_requires_forwarded_params"])
        .await
        .expect_failure();
    res.assert_stderr_matches(r"-r_-e_-d_-s_-x_-a_-i_-w_-u_-k_must_be_passed_to_installer");
    let record = res.invocation_record()?;
    assert_eq!(errors(&record).len(), 1);
    Ok(())
}

#[buck_test]
async fn test_install_forwards_params(buck: &Buck) -> anyhow::Result<()> {
    setup_sandbox(buck)?;
    buck.install(&[
        "-r",
        "-e",
        "-d",
        "-s",
        "serial",
        "-x",
        "-a",
        "activity",
        "-i",
        "intent",
        "-w",
        "-u",
        "-k",
        "root//:installer_server_requires_forwarded_params",
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_install_forwards_params_long_form(buck: &Buck) -> anyhow::Result<()> {
    setup_sandbox(buck)?;
    buck.install(&[
        "--run",
        "--emulator",
        "--device",
        "--serial",
        "serial",
        "--all-devices",
        "--activity",
        "activity",
        "--intent-uri",
        "intent",
        "--wait-for-debugger",
        "--uninstall",
        "--keep",
        "root//:installer_server_requires_forwarded_params",
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_install_extra_args_ordered_after_builtin_args(buck: &Buck) -> anyhow::Result<()> {
    setup_sandbox(buck)?;
    buck.install(&[
        "-r",
        "root//:installer_validates_extra_args_order",
        "--",
        "--",
        "--expected-extra-arg",
    ])
    .await?;
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_fail_to_build_installer(buck: &Buck) -> anyhow::Result<()> {
    setup_sandbox(buck)?;
    let res = buck
        .install(&["root//:bad_installer_target"])
        .await
        .expect_failure();
    res.assert_stderr_matches(r"Failed to build installer");
    let record = res.invocation_record()?;
    assert_eq!(errors(&record).len(), 1);
    Ok(())
}

#[buck_test]
async fn test_installer_early_exit(buck: &Buck) -> anyhow::Result<()> {
    setup_sandbox(buck)?;
    buck.install(&["root//:installer_early_exit"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"Installer process exited with status");
    Ok(())
}
