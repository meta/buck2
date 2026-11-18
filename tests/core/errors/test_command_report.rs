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
use std::time::Duration;

use buck2_e2e::json::Json;
use buck2_e2e::prelude::*;
use serde_json::Value;

fn read_report(path: &Path) -> anyhow::Result<Value> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

async fn command_report_test(
    buck: &Buck,
    tmp_path: &Path,
    name: &str,
    command: &[&str],
) -> anyhow::Result<()> {
    let report = tmp_path.join("command_report.json");
    let report_path = report.to_string_lossy();
    buck.build(&[&["--command-report-path", &report_path], command].concat())
        .await
        .expect_failure();

    let mut report = read_report(&report)?;
    report
        .as_object_mut()
        .expect("a command report object")
        .remove("trace_id")
        .expect("a `trace_id` in the command report");

    golden(
        &Json::from(report).dumps(Some(2), true),
        &format!("fixtures/{name}.golden.json"),
    );
    Ok(())
}

// Test build with a couple of build errors
#[buck_test]
async fn test_command_report_build_errors(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    command_report_test(
        buck,
        tmp_path,
        "test_command_report_build_errors",
        &[":fail1", ":fail2"],
    )
    .await
}

// Set Watchman timeout to 0 to mimic a Watchman Timeout error.
#[buck_test(
    extra_buck_config = [("buck2", "file_watcher", "watchman")],
    env = [("BUCK2_WATCHMAN_TIMEOUT", "0")],
)]
async fn test_command_report_watchman_error(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let report = tmp_path.join("command_report.json");
    buck.build(&[
        "--command-report-path",
        &report.to_string_lossy(),
        ":build_success",
    ])
    .await
    .expect_failure();

    let report = read_report(&report)?;

    assert_eq!(report["exit_code"], ExitCode::UserError as i32);
    assert!(
        report["error_messages"][0]
            .as_str()
            .unwrap_or_default()
            .contains("SyncableQueryHandler returned an error")
    );
    Ok(())
}

// Early client error that doesn't show up in invocation records
#[buck_test(env = [("BUCK2_TEST_INIT_DAEMON_ERROR", "true")])]
async fn test_command_report_init_daemon_error(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let report = tmp_path.join("command_report.json");
    buck.build(&[
        "--command-report-path",
        &report.to_string_lossy(),
        ":build_success",
    ])
    .await
    .expect_failure();

    let report = read_report(&report)?;

    assert_eq!(report["exit_code"], ExitCode::ConnectError as i32);
    assert!(
        report["error_messages"][0]
            .as_str()
            .unwrap_or_default()
            .contains("Injected init daemon error")
    );
    Ok(())
}

// Deliberately cause a daemon connection failure.
// This test case spawns a loose daemon that we can't connect to. On windows
// this loose daemon will keep holding onto buck-out files after test case finishes
// and prevent other processes from changing them, so set a termination timeout
// of 20 seconds so that this loose daemon gets killed before test case finishes.
#[buck_test(
    write_invocation_record = true,
    allow_daemon_panics = true,
    env = [("BUCK2_TEST_FAIL_BUCKD_AUTH", "true"), ("BUCK2_TERMINATE_AFTER", "15")],
)]
async fn test_exit_result_connection_error(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let report = tmp_path.join("command_report.json");
    let res = buck
        .build(&[
            "--command-report-path",
            &report.to_string_lossy(),
            ":build_success",
        ])
        .await
        .expect_failure();

    let record = res.invocation_record()?;
    let report = read_report(&report)?;

    assert_eq!(report["exit_code"], ExitCode::ConnectError as i32);
    assert!(
        report["error_messages"][0]
            .as_str()
            .unwrap_or_default()
            .contains("injected auth error")
    );
    assert_eq!(record["exit_result_name"], "CONNECT_ERROR");

    tokio::time::sleep(Duration::from_secs(20)).await; // Makes sure the daemon terminates before test case finishes
    Ok(())
}

// Late client error takes precedence over action errors
#[buck_test(
    write_invocation_record = true,
    env = [("BUCK2_TEST_BUILD_ERROR", "true")],
)]
async fn test_command_report_post_build_client_error(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let report = tmp_path.join("command_report.json");
    // Failed build that should have some action errors
    let res = buck
        .build(&["--command-report-path", &report.to_string_lossy(), ":fail1"])
        .await
        .expect_failure();

    let record = res.invocation_record()?;
    // There's only 1 error message and it's the late client error that's injected
    assert_eq!(
        record.single_error()["message"],
        "Injected Build Response Error"
    );

    let report = read_report(&report)?;

    assert_eq!(report["error_messages"].as_array().map(Vec::len), Some(1));
    assert_eq!(report["exit_code"], ExitCode::InfraError as i32);
    assert!(
        report["error_messages"][0]
            .as_str()
            .unwrap_or_default()
            .contains("Injected Build Response Error")
    );

    assert_eq!(record["exit_result_name"], "INFRA_ERROR");
    Ok(())
}

#[buck_test]
async fn test_cleanup_timeout(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let report = tmp_path.join("command_report.json");
    buck.targets(&["--command-report-path", &report.to_string_lossy(), ":"])
        .await?;

    let report = read_report(&report)?;

    // test commands have scribe logging disabled, which is reported as a finalizing error
    let finalizing_errors = report["finalizing_error_messages"]
        .as_array()
        .expect("finalizing error messages");
    assert_eq!(finalizing_errors.len(), 1);
    let finalizing_error = finalizing_errors[0].as_str().unwrap_or_default();
    assert!(finalizing_error.contains("'invocation recorder' failed to finalize"));
    assert!(finalizing_error.contains("Scribe sink not enabled"));
    Ok(())
}

// Should match behavior of command report test in buck wrapper
#[buck_test(data_dir = "empty_buckconfig")]
async fn test_empty_buckconfig(buck: &Buck, _tmp_path: &Path) -> anyhow::Result<()> {
    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    buck.targets(&[":"])
        .env("BUCK_WRAPPER_UUID", uuid)
        .await
        .expect_failure();
    let report_path = buck
        .cwd()
        .join("buck-out/v2/log")
        .join(uuid)
        .join("command_report.json");

    let report = read_report(&report_path)?;

    assert!(
        report["error_messages"][0]
            .as_str()
            .unwrap_or_default()
            .contains("Error creating cell resolver")
    );
    Ok(())
}
