/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use buck2_e2e::InvocationRecord;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::json_get;
use serde_json::Value;
use serde_json::json;

/// Whether the invocation record says the call was direct: `None` if it does not say.
fn agent_direct_call(record: &Value) -> Option<bool> {
    json_get(record, &["agent_direct_call"]).map(|v| v.as_bool().unwrap())
}

/// Non-enforced client with valid agent context should succeed.
#[buck_test]
async fn test_agent_context_valid_non_enforced(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:pass", "--agent-context", "intent=build,attempt=1"])
        .await?;
    Ok(())
}

/// Non-enforced client with invalid values should still succeed (stored as-is).
#[buck_test]
async fn test_agent_context_non_enforced_no_validation(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:pass",
        "--agent-context",
        "intent=garbage_value,attempt=1",
    ])
    .await?;
    Ok(())
}

/// Enforced client with all required fields and valid values should succeed.
#[buck_test]
async fn test_agent_context_enforced_valid(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:pass",
        "--client-metadata",
        "id=test_enforced_client",
        "--agent-context",
        "intent=build,attempt=1",
    ])
    .await?;
    Ok(())
}

/// Enforced client with required + optional fields should succeed.
#[buck_test]
async fn test_agent_context_enforced_with_optional_field(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:pass",
        "--client-metadata",
        "id=test_enforced_client",
        "--agent-context",
        "intent=fix,attempt=2,prior_error=missing_target",
    ])
    .await?;
    Ok(())
}

/// Enforced client with invalid value for constrained field should fail
/// with the exact error message including valid values and description.
#[buck_test]
async fn test_agent_context_enforced_invalid_value(buck: &Buck) {
    buck.build(&[
        "//:pass",
        "--client-metadata",
        "id=test_enforced_client",
        "--agent-context",
        "intent=invalid_value,attempt=1",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(concat!(
        r"Invalid agent-context value `invalid_value` for key `intent`\.",
        r"\s+intent: The purpose of this buck2 invocation",
        r"\s+Valid values: build, test, query, fix, investigate",
    ));
}

/// Enforced client with unknown key should fail with sorted list of valid keys.
#[buck_test]
async fn test_agent_context_enforced_unknown_key(buck: &Buck) {
    buck.build(&[
        "//:pass",
        "--client-metadata",
        "id=test_enforced_client",
        "--agent-context",
        "intent=build,attempt=1,unknown_key=foo",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(concat!(
        r"Unknown agent-context key `unknown_key`\.",
        r"\s+Valid keys: attempt, intent, prior_error",
    ));
}

/// Enforced client missing required fields should list all missing fields sorted.
#[buck_test]
async fn test_agent_context_enforced_missing_required(buck: &Buck) {
    buck.build(&[
        "//:pass",
        "--client-metadata",
        "id=test_enforced_client",
        "--agent-context",
        "prior_error=missing_target",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(concat!(
        r"Missing required agent-context field\(s\):",
        r"\s+- attempt: Which attempt number this is for the same logical task",
        r"\s+- intent: The purpose of this buck2 invocation",
    ));
}

/// Enforced client with empty value for required field should report it as missing.
#[buck_test]
async fn test_agent_context_enforced_empty_value_counts_as_missing(buck: &Buck) {
    buck.build(&[
        "//:pass",
        "--client-metadata",
        "id=test_enforced_client",
        "--agent-context",
        "intent=,attempt=1",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(concat!(
        r"Missing required agent-context field\(s\):",
        r"\s+- intent: The purpose of this buck2 invocation",
    ));
}

/// Build without --agent-context should always succeed.
#[buck_test]
async fn test_agent_context_no_context_passes(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:pass"]).await?;
    Ok(())
}

/// Enforced client without --agent-context should succeed (not required in v1).
#[buck_test]
async fn test_agent_context_enforced_no_context_passes(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:pass", "--client-metadata", "id=test_enforced_client"])
        .await?;
    Ok(())
}

/// Malformed --agent-context should fail with the exact format error.
#[buck_test]
async fn test_agent_context_invalid_format(buck: &Buck) {
    buck.build(&["//:pass", "--agent-context", "not_a_valid_format"])
        .await
        .expect_failure()
        .assert_stderr_matches(concat!(
            r"Invalid agent-context format: `not_a_valid_format`\.",
            r" Each entry must be a `key=value` pair\.",
        ));
}

/// Non-snake_case key should fail with the exact key format error.
#[buck_test]
async fn test_agent_context_invalid_key_format(buck: &Buck) {
    buck.build(&["//:pass", "--agent-context", "InvalidKey=value"])
        .await
        .expect_failure()
        .assert_stderr_matches(concat!(
            r"Invalid agent-context key: `InvalidKey`\.",
            r" Keys must be snake_case identifiers\.",
        ));
}

#[buck_test(write_invocation_record = true)]
async fn test_agent_context_argfile_before_subcommand(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join("context.args"),
        "--agent-context=intent=build\n",
    )?;
    let result = buck
        .command(&[
            "@context.args",
            "--client-metadata",
            "id=test_enforced_client",
            "build",
            "//:pass",
            "--agent-context",
            "attempt=1",
        ])
        .await?;
    let record = result.invocation_record()?;
    let context = record["agent_context"].as_array().unwrap();
    assert_eq!(
        context
            .iter()
            .filter(|entry| entry["key"] == "intent" || entry["key"] == "attempt")
            .collect::<Vec<_>>(),
        [
            &json!({"key": "intent", "value": "build"}),
            &json!({"key": "attempt", "value": "1"}),
        ]
    );
    Ok(())
}

/// Agent context should appear in the invocation record.
#[buck_test(write_invocation_record = true)]
async fn test_agent_context_logged_to_invocation_record(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&["//:pass", "--agent-context", "intent=build,attempt=1"])
        .await?;

    let record = res.invocation_record()?;
    let agent_context = json_get(&record, &["agent_context"])
        .expect("agent_context should be present in invocation record");

    let entry_map: BTreeMap<&str, &Value> = agent_context
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["key"].as_str().unwrap(), &e["value"]))
        .collect();
    assert_eq!(entry_map["intent"], "build");
    assert_eq!(entry_map["attempt"], "1");
    assert_eq!(agent_direct_call(&record), None);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_agent_direct_call_logged_to_invocation_record(buck: &Buck) -> anyhow::Result<()> {
    for (value, expected) in [
        ("true", Some(true)),
        ("false", Some(false)),
        ("invalid", None),
    ] {
        let res = buck
            .build(&[
                "//:pass",
                "--client-metadata",
                "id=test_enforced_client",
                "--agent-context",
                &format!("intent=build,attempt=1,direct_call={value}"),
            ])
            .await?;
        let record = res.invocation_record()?;
        assert_eq!(agent_direct_call(&record), expected);
        assert!(
            record["agent_context"]
                .as_array()
                .unwrap()
                .contains(&json!({"key": "direct_call", "value": value}))
        );
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_agent_direct_call_last_value_wins(buck: &Buck) -> anyhow::Result<()> {
    for (value, expected) in [("false", Some(false)), ("invalid", None)] {
        let res = buck
            .build(&[
                "//:pass",
                "--agent-context",
                "direct_call=true",
                "--agent-context",
                &format!("direct_call={value}"),
            ])
            .await?;
        assert_eq!(agent_direct_call(&*res.invocation_record()?), expected);
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_agent_direct_call_alone_does_not_require_context(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&[
            "//:pass",
            "--client-metadata",
            "id=test_enforced_client",
            "--agent-context",
            "direct_call=true",
        ])
        .await?;
    assert_eq!(res.invocation_record()?["agent_direct_call"], true);
    Ok(())
}

/// The file name of the executable `path` resolves to.
fn real_name(path: impl AsRef<Path>) -> anyhow::Result<String> {
    Ok(std::fs::canonicalize(path)?
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned())
}

/// The interpreter `python3` runs as, to run a Python tool with: a launcher may stand in front of
/// it, and the tool must be the interpreter itself to be seen as one.
async fn python(buck: &Buck) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .env_clear()
        .envs(buck.envs())
        .output()
        .await?;
    anyhow::ensure!(output.status.success(), "`python3` failed: {output:?}");
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// Runs `buck2 build //:pass <extra>` under `agent`, through `prefix`, as a coding agent that
/// identifies itself would, and gives its exit code, stderr and invocation record.
async fn run(
    buck: &Buck,
    agent: &Path,
    record_path: &Path,
    prefix: &[String],
    extra: &[&str],
) -> anyhow::Result<(Option<i32>, String, InvocationRecord)> {
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(agent)
            .args(["-c", "\"$@\"; exit $?", "agent"])
            .args(prefix)
            .arg(buck.executable())
            .args(["build", "//:pass", "--unstable-write-invocation-record"])
            .arg(record_path)
            .args(extra)
            .current_dir(buck.cwd())
            .env_clear()
            .envs(buck.envs())
            .env("CODING_AGENT_METADATA", "id=codex,direct_call=true")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    Ok((
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        InvocationRecord::read(record_path)?,
    ))
}

// Ancestry inference is enabled only on Linux.
#[buck_test(skip_for_os = ["darwin", "windows"])]
#[ignore = "Ancestry inference requires a Meta internal build"] // @oss-enable
async fn test_detect_direct_agent_and_stop_at_tools(buck: &mut Buck) -> anyhow::Result<()> {
    // Prost serializes protobuf enum fields as integers in invocation-record JSON.
    let (caller_agent, caller_command_shell, caller_other) = (1, 2, 3);
    let caller_command_wrapper = 5;
    let (stop_agent, stop_other) = (1, 2);
    buck.remove_env("CODING_AGENT_METADATA");
    let buck = &*buck;
    // Copy a shell so its executable identity is stable without depending on an
    // installed agent or an LLM service. Keep it alive while Buck reads its parent.
    let agent = buck.cwd().join("codex");
    std::fs::copy("/bin/sh", &agent)?;
    let script = buck.cwd().join("tool.sh");
    std::fs::write(&script, "\"$@\"\nexit $?\n")?;
    let python_tool = buck.cwd().join("tool.py");
    std::fs::write(
        &python_tool,
        "import subprocess, sys\nsys.exit(subprocess.call(sys.argv[1:]))\n",
    )?;
    let exec_script = buck.cwd().join("exec_tool.sh");
    std::fs::write(&exec_script, "exec \"$@\"\n")?;
    let record_path = buck.cwd().join("record.json");
    let script = script.to_str().unwrap();

    // Reuse an agent-started daemon from tools and the test runner. Only the
    // client's ancestry contributes to inference; it does not change the explicit marker.
    let direct: Vec<String> = ["/bin/sh", "-c", "\"$@\"; exit $?", "shell"]
        .map(String::from)
        .to_vec();
    let (code, stderr, record) = run(buck, &agent, &record_path, &direct, &[]).await?;
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(agent_direct_call(&record), None);
    assert_eq!(record["agent_inferred_direct_call"], true);
    assert_eq!(
        record["agent_context"],
        json!([{"key": "id", "value": "codex"}])
    );
    let shell_name = real_name("/bin/sh")?;
    assert_eq!(
        record["agent_callers"],
        json!([
            {"kind": caller_command_shell, "executable": shell_name},
            {"kind": caller_agent, "executable": "codex"},
        ])
    );
    assert_eq!(record["agent_detection_stop_reason"], stop_agent);
    let status: Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    let daemon = status["process_info"]["pid"].as_u64().unwrap();

    let tools = [
        vec!["/bin/sh".to_owned(), script.to_owned()],
        vec![
            python(buck).await?,
            python_tool.to_str().unwrap().to_owned(),
        ],
    ];
    for prefix in std::iter::once(&direct).chain(&tools) {
        for value in [None, Some("true"), Some("false"), Some("invalid")] {
            let direct_call_arg = value.map(|value| format!("direct_call={value}"));
            let extra = match &direct_call_arg {
                None => vec![],
                Some(arg) => vec!["--agent-context", arg.as_str()],
            };
            let (code, stderr, record) = run(buck, &agent, &record_path, prefix, &extra).await?;
            assert_eq!(code, Some(0), "{stderr}");
            assert_eq!(
                agent_direct_call(&record),
                match value {
                    None | Some("invalid") => None,
                    Some(value) => Some(value == "true"),
                }
            );
            assert_eq!(record["agent_inferred_direct_call"], *prefix == direct);
            if *prefix == direct {
                assert_eq!(record["agent_detection_stop_reason"], stop_agent);
                assert_eq!(
                    record["agent_callers"].as_array().unwrap().last().unwrap(),
                    &json!({"kind": caller_agent, "executable": "codex"})
                );
            } else {
                assert_eq!(record["agent_detection_stop_reason"], stop_other);
                assert_eq!(
                    record["agent_callers"],
                    json!([{"kind": caller_other, "executable": real_name(&prefix[0])?}])
                );
            }
        }
    }

    let timeout: Vec<String> = ["/usr/bin/timeout", "60"].map(String::from).to_vec();
    let timeout_caller = json!({"kind": caller_command_wrapper, "executable": "timeout"});
    for prefix in [
        timeout.clone(),
        [&timeout[..], &direct[..]].concat(),
        [&direct[..], &timeout[..]].concat(),
    ] {
        let (code, stderr, record) = run(buck, &agent, &record_path, &prefix, &[]).await?;
        assert_eq!(code, Some(0), "{stderr}");
        assert_eq!(agent_direct_call(&record), None);
        assert_eq!(record["agent_inferred_direct_call"], true);
        assert_eq!(record["agent_detection_stop_reason"], stop_agent);
        let callers = record["agent_callers"].as_array().unwrap();
        assert_eq!(
            callers
                .iter()
                .filter(|caller| **caller == timeout_caller)
                .count(),
            1
        );
        assert_eq!(
            callers.last().unwrap(),
            &json!({"kind": caller_agent, "executable": "codex"})
        );
    }

    // timeout must not hide a script/tool boundary on either side of it.
    for tool in &tools {
        let tool_caller = json!({"kind": caller_other, "executable": real_name(&tool[0])?});
        for (prefix, expected_callers) in [
            ([&timeout[..], &tool[..]].concat(), json!([tool_caller])),
            (
                [&tool[..], &timeout[..]].concat(),
                json!([timeout_caller, tool_caller]),
            ),
        ] {
            let (code, stderr, record) = run(buck, &agent, &record_path, &prefix, &[]).await?;
            assert_eq!(code, Some(0), "{stderr}");
            assert_eq!(agent_direct_call(&record), None);
            assert_eq!(record["agent_inferred_direct_call"], false);
            assert_eq!(record["agent_detection_stop_reason"], stop_other);
            assert_eq!(record["agent_callers"], expected_callers);
        }
    }

    // exec and sourced scripts erase the visible tool boundary. These known
    // false positives must leave the explicit direct-call marker unset.
    for prefix in [
        vec![
            "/bin/sh".to_owned(),
            exec_script.to_str().unwrap().to_owned(),
        ],
        vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            format!(". \"{script}\""),
            "shell".to_owned(),
        ],
    ] {
        let (code, stderr, record) = run(buck, &agent, &record_path, &prefix, &[]).await?;
        assert_eq!(code, Some(0), "{stderr}");
        assert_eq!(agent_direct_call(&record), None);
        assert_eq!(record["agent_inferred_direct_call"], true);
    }

    let (code, stderr, record) = run(
        buck,
        &agent,
        &record_path,
        &[],
        &[
            "--agent-context",
            "direct_call=true",
            "--agent-context",
            "advice_ack=test_advice",
        ],
    )
    .await?;
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(record["agent_direct_call"], true);
    assert_eq!(record["agent_inferred_direct_call"], true);
    buck.build(&[
        "//:pass",
        "--unstable-write-invocation-record",
        record_path.to_str().unwrap(),
    ])
    .await?;
    let record = InvocationRecord::read(&record_path)?;
    assert_eq!(record["agent_inferred_direct_call"], false);
    assert_eq!(record["agent_detection_stop_reason"], stop_other);
    assert_eq!(
        record["agent_callers"].as_array().unwrap().last().unwrap()["kind"],
        caller_other
    );
    assert_eq!(agent_direct_call(&record), None);
    assert!(
        json_get(&record, &["agent_context"])
            .is_none_or(|context| context.as_array().is_some_and(Vec::is_empty))
    );
    let status: Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    assert_eq!(status["process_info"]["pid"], daemon);
    Ok(())
}
