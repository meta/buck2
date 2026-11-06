/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! In-process test runner for `InternalRunnerTestInfo`.
//!
//! Drives the listing → parse_test_listing callback → per-test execution flow
//! entirely within the Buck2 process, using the `TestOrchestrator` trait
//! directly (no gRPC, no subprocess).

use std::time::Duration;

use buck2_build_api::interpreter::rule_defs::provider::builtin::internal_runner_test_info::InternalRunnerTestInfo;
use buck2_build_api::interpreter::rule_defs::provider::builtin::internal_runner_test_info::OwnedInternalRunnerTestInfo;
use buck2_error::BuckErrorContext;
use buck2_hash::BuckFixedSortedMap;
use buck2_test_api::data::ArgValue;
use buck2_test_api::data::ArgValueContent;
use buck2_test_api::data::ExecuteResponse;
use buck2_test_api::data::ExecutionStatus;
use buck2_test_api::data::ExecutionStream;
use buck2_test_api::data::ExternalRunnerSpec;
use buck2_test_api::data::ExternalRunnerSpecValue;
use buck2_test_api::data::LocalResourceType;
use buck2_test_api::data::RequiredLocalResources;
use buck2_test_api::data::TestResult;
use buck2_test_api::data::TestStage;
use buck2_test_api::data::TestStatus;
use buck2_test_api::protocol::TestOrchestrator;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use host_sharing::HostSharingRequirements;

/// Run the full test lifecycle for a single target in-process:
/// 1. Execute the test binary in listing mode
/// 2. Parse listing output via the Starlark `parse_test_listing` callback
/// 3. Report discovered tests, and any results the listing already decided
/// 4. Execute each remaining test individually
/// 5. Report each test result
pub async fn run_internal_test(
    orchestrator: &dyn TestOrchestrator,
    spec: ExternalRunnerSpec,
    listing_spec: ExternalRunnerSpec,
    provider: &OwnedInternalRunnerTestInfo,
    timeout: Duration,
) -> buck2_error::Result<()> {
    let target_handle = spec.target.handle;
    let suite = spec.target.target.clone();

    // `'v`-branded views of the provider must not be held across awaits (only the `OwnedFrozen`
    // may be), so views are derived in scopes that end before the next await.
    let required_local_resources = RequiredLocalResources {
        resources: provider
            .as_ref()
            .value()
            .as_ref()
            .required_local_resources()
            .map(|r| LocalResourceType {
                name: r.name.clone(),
            })
            .collect(),
    };

    // Step 1: Execute listing using the dedicated listing command
    let listing_command = build_command_from_spec(&listing_spec);
    let listing_stage = TestStage::Listing {
        suite: suite.clone(),
        cacheable: true,
    };

    let listing_response = orchestrator
        .execute2(
            listing_stage,
            target_handle,
            listing_command,
            build_env_from_spec(&listing_spec),
            timeout,
            HostSharingRequirements::default(),
            Vec::new(),
            None,
            required_local_resources.clone(),
            false,
        )
        .await
        .buck_error_context("Listing execution failed")?;

    let listing_result = match listing_response {
        ExecuteResponse::Result(r) => r,
        ExecuteResponse::Cancelled(_) => return Ok(()),
    };

    // Check listing exit status before parsing
    match &listing_result.status {
        ExecutionStatus::Finished { exitcode } if *exitcode != 0 => {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "Test listing exited with code {}",
                exitcode
            ));
        }
        ExecutionStatus::TimedOut { .. } => {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "Test listing timed out"
            ));
        }
        _ => {}
    }

    let listing_output = match &listing_result.stdout {
        ExecutionStream::Inline(bytes) => String::from_utf8_lossy(bytes).to_string(),
    };

    // Step 2: Parse listing output via Starlark callback
    let discovered_tests =
        InternalRunnerTestInfo::parse_test_listing_output(provider, &listing_output)
            .buck_error_context("Failed to parse test listing output")?;

    // Step 3: Report discovered tests
    let test_names: Vec<String> = discovered_tests.iter().map(|t| t.name.clone()).collect();
    orchestrator
        .report_tests_discovered(target_handle, suite.clone(), test_names)
        .await
        .buck_error_context("Failed to report discovered tests")?;

    let mut tests_to_run = Vec::with_capacity(discovered_tests.len());
    for entry in discovered_tests {
        match entry.preset_result {
            Some(preset_result) => {
                let test_result = TestResult {
                    target: target_handle,
                    name: entry.name,
                    status: preset_result.status,
                    msg: preset_result.message,
                    duration: None,
                    details: String::new(),
                    max_memory_used_bytes: None,
                };
                orchestrator
                    .report_test_result(test_result)
                    .await
                    .buck_error_context("Failed to report test result")?;
            }
            None => tests_to_run.push(entry),
        }
    }

    // Step 4+5: Execute and report each discovered test.
    // FuturesUnordered pipelines execute2 calls so the next test's orchestrator
    // dispatch overlaps with result parsing of the previous test.
    let mut futures = FuturesUnordered::new();

    for entry in &tests_to_run {
        let test_stage = TestStage::Testing {
            suite: suite.clone(),
            testcases: vec![entry.filter.clone()],
            variant: None,
            repeat_count: None,
        };

        let mut test_command = build_command_from_spec(&spec);
        test_command.push(ArgValue {
            content: ArgValueContent::ExternalRunnerSpecValue(ExternalRunnerSpecValue::Verbatim(
                entry.filter.clone(),
            )),
            format: None,
        });

        let entry_name = entry.name.clone();
        let env = build_env_from_spec(&spec);
        let required_local_resources = required_local_resources.clone();

        futures.push(async move {
            let test_response = orchestrator
                .execute2(
                    test_stage,
                    target_handle,
                    test_command,
                    env,
                    timeout,
                    HostSharingRequirements::default(),
                    Vec::new(),
                    None,
                    required_local_resources.clone(),
                    false,
                )
                .await;
            (entry_name, test_response)
        });
    }

    while let Some((entry_name, test_response)) = futures.next().await {
        match test_response {
            Ok(ExecuteResponse::Result(result)) => {
                let exit_code = match result.status {
                    ExecutionStatus::Finished { exitcode } => exitcode,
                    ExecutionStatus::TimedOut { .. } => {
                        // For timeouts, report immediately without calling the callback
                        let test_result = TestResult {
                            target: target_handle,
                            name: entry_name,
                            status: TestStatus::TIMEOUT,
                            msg: Some("Test timed out".to_owned()),
                            duration: Some(result.execution_time),
                            details: format_execution_output(&result.stdout, &result.stderr),
                            max_memory_used_bytes: result.max_memory_used_bytes,
                        };
                        orchestrator
                            .report_test_result(test_result)
                            .await
                            .buck_error_context("Failed to report test result")?;
                        continue;
                    }
                };

                let stdout_str = match &result.stdout {
                    ExecutionStream::Inline(bytes) => String::from_utf8_lossy(bytes).to_string(),
                };
                let stderr_str = match &result.stderr {
                    ExecutionStream::Inline(bytes) => String::from_utf8_lossy(bytes).to_string(),
                };

                let result_entries = InternalRunnerTestInfo::parse_test_result_output(
                    provider,
                    &stdout_str,
                    &stderr_str,
                    exit_code,
                )
                .buck_error_context("Failed to parse test result output")?;

                if result_entries.is_empty() {
                    // Parser returned no results — synthesize a fallback based on
                    // exit code to prevent silent test disappearance.
                    let status = if exit_code == 0 {
                        TestStatus::PASS
                    } else {
                        TestStatus::FAIL
                    };
                    let test_result = TestResult {
                        target: target_handle,
                        name: entry_name,
                        status,
                        msg: Some(format!(
                            "Parser returned no results (exit code {})",
                            exit_code
                        )),
                        duration: Some(result.execution_time),
                        details: format_execution_output(&result.stdout, &result.stderr),
                        max_memory_used_bytes: result.max_memory_used_bytes,
                    };
                    orchestrator
                        .report_test_result(test_result)
                        .await
                        .buck_error_context("Failed to report test result")?;
                } else {
                    for res in result_entries {
                        let test_result = TestResult {
                            target: target_handle,
                            name: res.name,
                            status: res.status,
                            msg: res.message,
                            duration: res.duration,
                            details: res.details.unwrap_or_default(),
                            max_memory_used_bytes: result.max_memory_used_bytes,
                        };
                        orchestrator
                            .report_test_result(test_result)
                            .await
                            .buck_error_context("Failed to report test result")?;
                    }
                }
            }
            Ok(ExecuteResponse::Cancelled(_)) => {
                let test_result = TestResult {
                    target: target_handle,
                    name: entry_name,
                    status: TestStatus::OMITTED,
                    msg: Some("Cancelled".to_owned()),
                    duration: None,
                    details: String::new(),
                    max_memory_used_bytes: None,
                };
                orchestrator
                    .report_test_result(test_result)
                    .await
                    .buck_error_context("Failed to report test result")?;
            }
            Err(e) => {
                let test_result = TestResult {
                    target: target_handle,
                    name: entry_name,
                    status: TestStatus::FATAL,
                    msg: Some(format!("{:#}", e)),
                    duration: None,
                    details: String::new(),
                    max_memory_used_bytes: None,
                };
                orchestrator
                    .report_test_result(test_result)
                    .await
                    .buck_error_context("Failed to report test result")?;
            }
        }
    }

    Ok(())
}

fn build_command_from_spec(spec: &ExternalRunnerSpec) -> Vec<ArgValue> {
    spec.command
        .iter()
        .map(|v| ArgValue {
            content: ArgValueContent::ExternalRunnerSpecValue(v.clone()),
            format: None,
        })
        .collect()
}

fn build_env_from_spec(spec: &ExternalRunnerSpec) -> BuckFixedSortedMap<String, ArgValue> {
    spec.env
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                ArgValue {
                    content: ArgValueContent::ExternalRunnerSpecValue(v.clone()),
                    format: None,
                },
            )
        })
        .collect()
}

fn format_execution_output(stdout: &ExecutionStream, stderr: &ExecutionStream) -> String {
    let stdout_str = match stdout {
        ExecutionStream::Inline(bytes) => String::from_utf8_lossy(bytes),
    };
    let stderr_str = match stderr {
        ExecutionStream::Inline(bytes) => String::from_utf8_lossy(bytes),
    };
    format!(
        "---- STDOUT ----\n{}\n---- STDERR ----\n{}\n",
        stdout_str, stderr_str
    )
}

#[cfg(test)]
mod tests {
    use buck2_core::cells::name::CellName;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
    use buck2_test_api::data::ConfiguredTarget;
    use buck2_test_api::data::ExecutionStream;
    use buck2_test_api::data::ExternalRunnerSpec;
    use buck2_test_api::data::ExternalRunnerSpecValue;
    use buck2_test_api::data::testing::ConfiguredTargetHandleExt;

    use super::*;

    fn make_spec(commands: Vec<&str>, env: Vec<(&str, &str)>) -> ExternalRunnerSpec {
        ExternalRunnerSpec {
            target: ConfiguredTarget {
                handle: ConfiguredTargetHandleExt::testing_new(0),
                cell: "root".to_owned(),
                package: "test".to_owned(),
                target: "my_test".to_owned(),
                configuration: "cfg".to_owned(),
                package_project_relative_path: ForwardRelativePathBuf::unchecked_new(
                    "test".to_owned(),
                ),
                test_config_unification_rollout: false,
                package_oncall: None,
            },
            test_type: "rust".to_owned(),
            command: commands
                .into_iter()
                .map(|c| ExternalRunnerSpecValue::Verbatim(c.to_owned()))
                .collect(),
            env: env
                .into_iter()
                .map(|(k, v)| {
                    (
                        k.to_owned(),
                        ExternalRunnerSpecValue::Verbatim(v.to_owned()),
                    )
                })
                .collect(),
            labels: vec![],
            contacts: vec![],
            oncall: None,
            working_dir_cell: CellName::testing_new("root"),
        }
    }

    #[test]
    fn test_build_command_from_spec() {
        let spec = make_spec(vec!["binary", "--flag"], vec![]);
        let cmd = build_command_from_spec(&spec);
        assert_eq!(cmd.len(), 2);
        assert_eq!(
            cmd[0].content,
            ArgValueContent::ExternalRunnerSpecValue(ExternalRunnerSpecValue::Verbatim(
                "binary".to_owned()
            ))
        );
        assert_eq!(
            cmd[1].content,
            ArgValueContent::ExternalRunnerSpecValue(ExternalRunnerSpecValue::Verbatim(
                "--flag".to_owned()
            ))
        );
    }

    #[test]
    fn test_build_env_from_spec() {
        let spec = make_spec(vec![], vec![("FOO", "bar"), ("BAZ", "qux")]);
        let env = build_env_from_spec(&spec);
        assert_eq!(env.len(), 2);
        assert!(env.contains_key("BAZ"));
        assert!(env.contains_key("FOO"));
    }

    #[test]
    fn test_format_execution_output() {
        let stdout = ExecutionStream::Inline(b"hello".to_vec());
        let stderr = ExecutionStream::Inline(b"world".to_vec());
        let output = format_execution_output(&stdout, &stderr);
        assert!(output.contains("hello"));
        assert!(output.contains("world"));
        assert!(output.contains("STDOUT"));
        assert!(output.contains("STDERR"));
    }

    #[test]
    fn test_format_execution_output_empty() {
        let stdout = ExecutionStream::Inline(vec![]);
        let stderr = ExecutionStream::Inline(vec![]);
        let output = format_execution_output(&stdout, &stderr);
        assert_eq!(output, "---- STDOUT ----\n\n---- STDERR ----\n\n");
    }
}
