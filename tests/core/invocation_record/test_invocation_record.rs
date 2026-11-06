/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::golden::sanitize_stderr;
use buck2_e2e::prelude::*;
use serde_json::json;

// FIXME(JakobDegen): Flakey in CI
//
// Disabled; it would take `skip_for_os = ["windows"]` (TODO(T154836875)).
// async fn test_has_end_of_stream_false(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
//     let hang_path = tmp_path.join("hang_path");
//     let record = tmp_path.join("record.json");
//
//     let cmd = buck
//         .build(&[
//             ":hang",
//             "-c",
//             &format!("test.hang_path={}", hang_path.display()),
//             "--unstable-write-invocation-record",
//             record.to_str().unwrap(),
//             "--local-only",
//             "--no-remote-cache",
//         ])
//         .spawn();
//
//     for _ in 0..10 {
//         if hang_path.exists() {
//             break;
//         }
//         tokio::time::sleep(std::time::Duration::from_secs(1)).await;
//     }
//     if !hang_path.exists() {
//         eprintln!("{:?}", cmd.wait().await);
//         anyhow::bail!("Signal file never created: {}", hang_path.display());
//     }
//
//     std::process::Command::new("kill")
//         .args(["-INT", &cmd.id().unwrap().to_string()])
//         .status()?;
//     let _ignored = cmd.wait().await;
//
//     let record = buck2_e2e::InvocationRecord::read(&record)?;
//
//     assert!(!record["has_end_of_stream"].as_bool().unwrap());
//     assert!(!record["has_command_result"].as_bool().unwrap());
//     Ok(())
// }

#[buck_test(write_invocation_record = true)]
async fn test_has_end_of_stream_true(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.build(&[":pass"]).await?;

    let record = res.invocation_record()?;

    assert_eq!(record["has_end_of_stream"], true);
    assert_eq!(record["has_command_result"], true);
    assert_eq!(record["repo_path"], buck.cwd().to_str().unwrap());
    Ok(())
}

// TODO(T154836632)
#[buck_test(skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_has_no_command_result(buck: &Buck) -> anyhow::Result<()> {
    // Start the daemon
    buck.build(&[]).await?;

    let status: serde_json::Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    let pid = &status["process_info"]["pid"];

    let result = buck
        .build(&[
            ":kill",
            "-c",
            &format!("test.pid={pid}"),
            "--local-only",
            "--no-remote-cache",
        ])
        .await
        .expect_failure();
    result.assert_stderr_matches("Buck daemon event bus encountered an error");

    let record = result.invocation_record()?;

    assert_eq!(record["has_end_of_stream"], true);
    assert!(
        record
            .get("has_command_result")
            .is_some_and(|v| !v.as_bool().unwrap_or(false)),
        "{}",
        *record
    );

    golden(
        &sanitize_stderr(&result.stderr),
        "fixtures/test_has_no_command_result.golden.txt",
    );
    Ok(())
}

// TODO(T154836632)
#[buck_test(skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_metadata(buck: &Buck) -> anyhow::Result<()> {
    // Start the daemon
    let res = buck.build(&[]).await?;

    let record = res.invocation_record()?;

    assert!(record["metadata"]["strings"].get("username").is_some());
    Ok(())
}

// TODO(T154836632)
#[buck_test(skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_client_metadata(buck: &Buck) -> anyhow::Result<()> {
    // Start the daemon
    let res = buck
        .build(&["--client-metadata=foo=bar", "--client-metadata=id=baz"])
        .await?;

    let record = res.invocation_record()?;

    assert_eq!(
        record["client_metadata"],
        json!([
            {"key": "foo", "value": "bar"},
            {"key": "id", "value": "baz"},
        ])
    );

    assert_eq!(record["metadata"]["strings"]["client"], "baz");
    Ok(())
}

// TODO(T154836632)
#[buck_test(skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_client_metadata_env(buck: &Buck) -> anyhow::Result<()> {
    // Start the daemon
    let res = buck
        .build(&["--client-metadata=foo=bar", "--client-metadata=id=baz"])
        .env("BUCK2_CLIENT_METADATA", "env_foo=env_bar,id=foobar")
        .await?;

    let record = res.invocation_record()?;

    assert_eq!(
        record["client_metadata"],
        json!([
            {"key": "env_foo", "value": "env_bar"},
            {"key": "id", "value": "foobar"},
            {"key": "foo", "value": "bar"},
            {"key": "id", "value": "baz"},
        ])
    );

    assert_eq!(record["metadata"]["strings"]["client"], "baz");
    Ok(())
}

#[buck_test(skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_agent_context_from_env(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&[])
        .env(
            "CODING_AGENT_METADATA",
            "id=test_agent,invocation_id=test_inv_123",
        )
        .await?;

    let record = res.invocation_record()?;
    let agent_ctx: std::collections::HashMap<&str, &serde_json::Value> = record["agent_context"]
        .as_array()
        .expect("`agent_context` is a list")
        .iter()
        .map(|e| (e["key"].as_str().expect("a string key"), &e["value"]))
        .collect();

    assert_eq!(agent_ctx["id"], "test_agent");
    assert_eq!(agent_ctx["invocation_id"], "test_inv_123");
    Ok(())
}

// TODO(T154836632)
#[buck_test(skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_client_metadata_clean(buck: &Buck) -> anyhow::Result<()> {
    // Start the daemon
    let res = buck
        .clean(&["--client-metadata=foo=bar", "--client-metadata=id=baz"])
        .await?;

    let record = res.invocation_record()?;

    assert_eq!(
        record["client_metadata"],
        json!([
            {"key": "foo", "value": "bar"},
            {"key": "id", "value": "baz"},
        ])
    );

    assert_eq!(record["metadata"]["strings"]["client"], "baz");
    Ok(())
}

#[buck_test(skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_client_metadata_debug(buck: &Buck) -> anyhow::Result<()> {
    // buck.debug() doesn't start the daemon, so we need to start it with a build
    buck.build(&[]).await?;

    let res = buck
        .debug(&[
            "allocator-stats",
            "--client-metadata=foo=bar",
            "--client-metadata=id=baz",
        ])
        .await?;

    let record = res.invocation_record()?;

    assert_eq!(
        record["client_metadata"],
        json!([
            {"key": "foo", "value": "bar"},
            {"key": "id", "value": "baz"},
        ])
    );

    assert_eq!(record["metadata"]["strings"]["client"], "baz");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_action_error_message_in_record(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.build(&[":fail"]).await.expect_failure();

    let record = res.invocation_record()?;

    let error = record.single_error();
    assert_eq!(
        error["message"],
        "Failed to build 'root//:fail (<unspecified>)'"
    );
    assert!(
        error["telemetry_message"]
            .as_str()
            .unwrap()
            .contains("Hi from stderr!")
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_non_action_error_message_in_record(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.build(&[":missing_target"]).await.expect_failure();

    let record = res.invocation_record()?;

    let error = record.single_error();
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("Unknown target `missing_target` from package `root//`")
    );
    Ok(())
}

// TODO(T154836632)
#[buck_test(skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_rule_type_names_ci(buck: &Buck) -> anyhow::Result<()> {
    // Start the daemon
    let res = buck
        .build(&[":duplicate", ":and_a_two", ":last_three", ":a_one"])
        .env("CI", "true")
        .await?;

    let record = res.invocation_record()?;

    assert_eq!(
        record["target_rule_type_names"],
        json!(["one", "pass_", "two"])
    );
    Ok(())
}

// TODO(T154836632)
#[buck_test(skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_rule_type_names_sandcastle(buck: &Buck) -> anyhow::Result<()> {
    // Start the daemon
    let res = buck
        .build(&[":duplicate", ":and_a_two", ":last_three", ":a_one"])
        .env("SANDCASTLE", "my_fake_job")
        .await?;

    let record = res.invocation_record()?;

    assert_eq!(
        record["target_rule_type_names"],
        json!(["one", "pass_", "two"])
    );
    Ok(())
}

// TODO(T154836632)
#[buck_test(skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_rule_type_names_user(buck: &Buck) -> anyhow::Result<()> {
    // Start the daemon
    let res = buck.build(&[":and_a_two", ":last_three", ":a_one"]).await?;

    let record = res.invocation_record()?;

    assert_eq!(
        record["target_rule_type_names"],
        json!(["one", "pass_", "two"])
    );
    Ok(())
}

// TODO(T154836632)
#[buck_test(skip_for_os = ["windows"], write_invocation_record = true)]
async fn test_rule_type_names_on_failure(buck: &Buck) -> anyhow::Result<()> {
    // Start the daemon
    let res = buck
        .build(&[":fail", ":last_three", ":a_one"])
        .await
        .expect_failure();

    let record = res.invocation_record()?;

    assert_eq!(
        record["target_rule_type_names"],
        json!(["fail", "one", "pass_"])
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_active_networks_kinds(buck: &Buck) -> anyhow::Result<()> {
    // Start the daemon
    let res = buck.build(&[]).await?;

    let record = res.invocation_record()?;

    assert!(record.get("active_networks_kinds").is_some());
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_peak_memory_and_disk(buck: &Buck) -> anyhow::Result<()> {
    // Start the daemon
    let res = buck.build(&[]).await?;

    let record = res.invocation_record()?;

    assert!(
        record.get("peak_used_disk_space_bytes").is_some()
            && record.get("peak_process_memory_bytes").is_some()
    );
    Ok(())
}

#[buck_test(setup_eden = true, skip_for_os = ["darwin"], write_invocation_record = true)]
async fn test_version_control_collector_slow(buck: &Buck) -> anyhow::Result<()> {
    // Force a 5 second sleep, hg commands should finish within that period of time
    let res = buck
        .build(&[":sleep", "--local-only", "--no-remote-cache"])
        .await?;

    let record = res.invocation_record()?;

    assert!(record.get("has_local_changes").is_some() && record.get("hg_revision").is_some());
    assert!(!record["hg_revision"].is_null());
    Ok(())
}

// NOTE: Delete or disable if flaky, ran a bunch of times on my devserver and it passes fine
#[buck_test(
    setup_eden = true,
    skip_for_os = ["darwin", "windows"],
    write_invocation_record = true
)]
async fn test_version_control_collector_fast(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.targets(&[":"]).await?;

    let record = res.invocation_record()?;

    assert!(record.get("has_local_changes").is_some() && record.get("hg_revision").is_some());
    assert!(!record["hg_revision"].is_null());
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_peak_stats(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&[":run", "--no-remote-cache", "--local-only"])
        .await?;

    let record = res.invocation_record()?;
    assert!(record.as_object().is_some_and(|r| !r.is_empty()));
    assert_eq!(record["max_in_progress_actions"], 1);
    assert_eq!(record["max_in_progress_local_actions"], 1);
    assert_eq!(record["max_in_progress_remote_actions"], 0);
    assert_eq!(record["max_in_progress_remote_uploads"], 0);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_parallelism_logging(buck: &Buck) -> anyhow::Result<()> {
    // Test multiple parallelism values
    let parallelism_values = [1, 4, 8];

    for parallelism in parallelism_values {
        // Test with different -j values to control concurrency
        let res = buck
            .build(&[":pass", "-j", &parallelism.to_string()])
            .await?;

        let record = res.invocation_record()?;

        // Verify that command_options is present and contains parallelism data
        let command_options = record
            .get("command_options")
            .expect("`command_options` is in the record");

        assert!(command_options.get("configured_parallelism").is_some());
        assert!(command_options.get("available_parallelism").is_some());

        // The configured parallelism should match what we passed via -j
        assert_eq!(command_options["configured_parallelism"], parallelism);

        // The available parallelism should be a positive integer (system dependent)
        let available = command_options["available_parallelism"].as_i64();
        assert!(available.is_some_and(|p| p > 0), "{command_options}");
    }

    // Test without -j flag - configured_parallelism should be null
    let res_no_j = buck.build(&[":pass"]).await?;

    let record_no_j = res_no_j.invocation_record()?;

    // Verify that command_options is present and contains parallelism data
    let command_options_no_j = record_no_j
        .get("command_options")
        .expect("`command_options` is in the record");

    assert!(command_options_no_j.get("configured_parallelism").is_some());
    assert!(command_options_no_j.get("available_parallelism").is_some());

    // When no -j is specified, configured_parallelism should equal available_parallelism
    assert_eq!(
        command_options_no_j["configured_parallelism"],
        command_options_no_j["available_parallelism"]
    );

    // The available parallelism should still be a positive integer (system dependent)
    let available = command_options_no_j["available_parallelism"].as_i64();
    assert!(available.is_some_and(|p| p > 0), "{command_options_no_j}");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_client_metadata_vscode_fallback(buck: &Buck) -> anyhow::Result<()> {
    // Test that vscode-fallback is set when VSCODE_PID is present. The harness gives commands an
    // empty stdin, so it is not a terminal.
    let res = buck.build(&[":pass"]).env("VSCODE_PID", "12345").await?;

    let record = res.invocation_record()?;

    // Should have vscode-fallback as client id
    let client_metadata_dict: std::collections::HashMap<&str, &serde_json::Value> =
        record["client_metadata"]
            .as_array()
            .expect("`client_metadata` is a list")
            .iter()
            .map(|m| (m["key"].as_str().expect("a string key"), &m["value"]))
            .collect();
    assert_eq!(
        client_metadata_dict.get("id").copied(),
        Some(&json!("vscode-fallback"))
    );
    assert_eq!(record["metadata"]["strings"]["client"], "vscode-fallback");
    Ok(())
}
