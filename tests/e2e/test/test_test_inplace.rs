/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![expect(
    non_snake_case,
    reason = "The tests keep the names the Python harness gave the parametrized cases, `True` and `False` included"
)]

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use buck2_e2e::BuckError;
use buck2_e2e::BuckProcess;
use buck2_e2e::ansi::remove_ansi_escape_sequences;
use buck2_e2e::mode::Mode;
use buck2_e2e::mode::get_mode_from_platform;
use buck2_e2e::prelude::*;
use buck2_e2e::process::daemon_is_alive;
use buck2_e2e::utils::filter_events;
use regex::Regex;
use serde_json::Value;

const MAC_AND_WINDOWS: [&str; 2] = ["darwin", "windows"];

/// Whether the buck2 under test is the deployed one, as the Python harness's `is_deployed_buck2()`
/// tells. The tests the Python module defined only for another buck2 pass without running any
/// command with it: they skip the final `buck2 clean` too, which the deployed buck2 cannot run in a
/// project outside the repository.
fn is_deployed_buck2() -> bool {
    std::env::var_os("TEST_EXECUTABLE").is_some_and(|executable| executable == "buck2")
}

/// Whether `pattern` matches somewhere in `text`, as Python's `re.search` without flags tells.
fn re_search(pattern: &str, text: &str) -> bool {
    Regex::new(pattern)
        .unwrap_or_else(|e| panic!("Invalid regex `{pattern}`: {e}"))
        .is_match(text)
}

#[cfg(unix)]
fn interrupt(process: &BuckProcess) -> anyhow::Result<()> {
    process.send_signal(buck2_e2e::process::Signal::Interrupt)
}

#[cfg(not(unix))]
fn interrupt(_process: &BuckProcess) -> anyhow::Result<()> {
    unreachable!("the tests that interrupt `buck2 test` do not run on Windows")
}

// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_sh_test(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["fbcode//buck2/tests/targets/rules/sh_test:test"])
        .await?;

    buck.test(&["fbcode//buck2/tests/targets/rules/sh_test:test_fail"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"1 TESTS FAILED\n(\s)+✗ fbcode//buck2/tests/targets/rules/sh_test:test_fail - unmanaged",
        );
    Ok(())
}

// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_sh_test_remote_checks(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/sh_test:test",
            "--remote-only",
        ])
        .await
        .expect_failure();
    result.assert_stderr_matches("RemoteOnly.*is incompatible");
    assert!(result.stderr.contains("Skip 1"));
    assert!(!result.stderr.contains("FATAL"));
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/sh_test:test_remote_implicit",
        "--local-only",
    ])
    .await?;
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/sh_test:test_remote_implicit",
        "--remote-only",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("RemoteOnly.*is incompatible");
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/sh_test:test_remote_explicit",
        "--local-only",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("LocalOnly.*is incompatible");
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/sh_test:test_remote_explicit",
        "--remote-only",
    ])
    .await?;
    Ok(())
}

// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_test_build_fail(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["hewwo_buck"])
        .await
        .expect_failure()
        .assert_stderr_matches("does not exist");

    buck.test(&["fbcode//buck2/tests/targets/rules/sh_test:test"])
        .await?;
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["darwin"])]
async fn test_cpp_test(buck: &Buck) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);
    buck.test(&["fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass", mode])
        .await?;

    buck.test(&["fbcode//buck2/tests/targets/rules/cxx:cpp_test_fail", mode])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"1 TESTS FAILED\n(\s)+✗ fbcode//buck2/tests/targets/rules/cxx:cpp_test_fail - Simple\.Fail",
        );

    buck.test(&[
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_local_only",
        mode,
    ])
    .await?;

    buck.test(&[
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_local_only",
        mode,
        "--remote-only",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"The desired execution strategy \(.RemoteOnly.\) is incompatible with the executor config that was selected",
    );
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_cpp_stress_runs(buck: &Buck) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);
    let res = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            mode,
            "--",
            "--stress-runs=10",
        ])
        .await?;

    assert!(
        res.stderr.contains("Pass 10"),
        "Expected stress runs to be run"
    );
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_cpp_stress_runs_deterministic_paths(buck: &Buck) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);
    let res = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            mode,
            "--",
            "--stress-runs=10",
        ])
        .await?;

    assert!(
        res.stderr.contains("Pass 10"),
        "Expected stress runs to be run"
    );
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["darwin"])]
async fn test_cpp_test_fdb_message(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_fail",
        get_mode_from_platform(Mode::Dev),
        "--",
        "--color",
        "off",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(r"Run \$ fdb buck test <args> to debug");
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = MAC_AND_WINDOWS)]
async fn test_python_test(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["fbcode//buck2/tests/targets/rules/python/test:test"])
        .await?;

    buck.test(&["fbcode//buck2/tests/targets/rules/python/test:test_env"])
        .await?;

    buck.test(&["fbcode//buck2/tests/targets/rules/python/test:test_fail"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"1 TESTS FAILED\n(\s)+✗ fbcode//buck2/tests/targets/rules/python/test:test_fail - test",
        );

    buck.test(&["fbcode//buck2/tests/targets/rules/python/test:test_fatal"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"1 TESTS FATALS\n(\s)+⚠ fbcode//buck2/tests/targets/rules/python/test:test_fatal - test",
        );
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = MAC_AND_WINDOWS)]
async fn test_python_test_with_remote_execution(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["fbcode//buck2/tests/targets/rules/python/test:test_remote_execution"])
        .await?;
    buck.test(&["fbcode//buck2/tests/targets/rules/python/test:test_remote_execution_fail"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"1 TESTS FAILED\n(\s)+✗ fbcode//buck2/tests/targets/rules/python/test:test_remote_execution_fail - test",
        );
    buck.test(&["fbcode//buck2/tests/targets/rules/python/test:test_remote_execution_fatal"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"1 TESTS FATALS\n(\s)+⚠ fbcode//buck2/tests/targets/rules/python/test:test_remote_execution_fatal - test",
        );
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = MAC_AND_WINDOWS)]
async fn test_python_needed_coverage(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/python/needed_coverage:test_pass",
        "fbcode//buck2/tests/targets/rules/python/needed_coverage:test_pass_specific_file",
    ])
    .await?;
    buck.test(&["fbcode//buck2/tests/targets/rules/python/needed_coverage:test_fail"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "ERROR: Actual coverage [0-9.]*% is smaller than expected 100.% for file",
        );
    buck.test(&["fbcode//buck2/tests/targets/rules/python/needed_coverage:test_fail_fractional"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "ERROR: Actual coverage [0-9.]*% is smaller than expected [0-9.]*% for file",
        );
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = MAC_AND_WINDOWS)]
async fn test_tests_attribute(buck: &Buck) -> anyhow::Result<()> {
    let lib_tests = buck
        .test(&["fbcode//buck2/tests/targets/rules/python/test:lib"])
        .await?;
    assert!(remove_ansi_escape_sequences(&lib_tests.stderr).contains("Pass 1"));
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = MAC_AND_WINDOWS)]
async fn test_tests_attribute_ignore(buck: &Buck) -> anyhow::Result<()> {
    let lib_tests = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:lib",
            "--ignore-tests-attribute",
        ])
        .await?;
    assert!(remove_ansi_escape_sequences(&lib_tests.stderr).contains("NO TESTS RAN"));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_listing_failure(buck: &Buck) -> anyhow::Result<()> {
    let output = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/broken:broken",
            get_mode_from_platform(Mode::Dev),
        ])
        .await
        .expect_failure();
    assert!(re_search(r"Listing Fail 1", &output.stderr));
    assert!(re_search(
        r"(?s)1 LISTINGS FAILED\n(\s)+⚠ fbcode//buck2/tests/targets/rules/python/broken:broken( \(cfg:.*?\))?\n",
        &output.stderr,
    ));
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_python_import_error_with_static_listing_builtin_runner(
    buck: &Buck,
) -> anyhow::Result<()> {
    let output = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/broken:broken_with_static_listing_builtin_runner",
            get_mode_from_platform(Mode::Dev),
        ])
        .await
        .expect_failure();

    assert!(re_search("(?s)2 TESTS FATALS", &output.stderr));
    assert!(re_search(
        r"(?s)test_\d \(buck2.tests.targets.rules.python.broken.broken_import.TestCase\)",
        &output.stderr,
    ));
    assert!(!re_search(
        "(?s)unittest.loader._FailedTest",
        &output.stderr
    ));
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_python_import_error_with_static_listing_new_provider(
    buck: &Buck,
) -> anyhow::Result<()> {
    let output = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/broken:broken_with_static_listing_new_adapter",
            get_mode_from_platform(Mode::Dev),
        ])
        .await
        .expect_failure();
    assert!(re_search("(?s)2 TESTS FATALS", &output.stderr));
    assert!(!re_search(
        "(?s)unittest.loader._FailedTest",
        &output.stderr
    ));
    assert!(re_search(
        r"(?s)test_\d \(buck2.tests.targets.rules.python.broken.broken_import.TestCase\)",
        &output.stderr,
    ));
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_python_import_error_with_static_listing_new_provider_bundle(
    buck: &Buck,
) -> anyhow::Result<()> {
    let output = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/broken:broken_with_static_listing_new_adapter_bundle",
            get_mode_from_platform(Mode::Dev),
        ])
        .await
        .expect_failure();
    assert!(re_search("(?s)1 TESTS FATALS", &output.stderr));
    assert!(re_search(
        r"(?s)buck2/tests/targets/rules/python/broken:broken_with_static_listing_new_adapter_bundle - main",
        &output.stderr,
    ));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_tests_dedupe(buck: &Buck) -> anyhow::Result<()> {
    let lib_tests = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:lib",
            "fbcode//buck2/tests/targets/rules/python/test:tests_for_lib",
            get_mode_from_platform(Mode::Dev),
        ])
        .await?;
    assert!(remove_ansi_escape_sequences(&lib_tests.stderr).contains("Pass 1"));
    Ok(())
}

async fn label_filtering(buck: &Buck, build_filtered: bool) -> anyhow::Result<()> {
    let mut cmd = vec!["fbcode//buck2/tests/targets/rules/label_test_filtering:"];
    if build_filtered {
        cmd.push("--build-filtered");
    }
    let with = |args: &[&'static str]| [cmd.as_slice(), args].concat();

    buck.test(&cmd)
        .await
        .expect_failure()
        .assert_stderr_matches("1 TESTS FAILED");

    buck.test(&with(&["--exclude", "label-pass"]))
        .await
        .expect_failure()
        .assert_stderr_matches("1 TESTS FAILED");

    buck.test(&with(&["--include", "label-fail"]))
        .await
        .expect_failure()
        .assert_stderr_matches("1 TESTS FAILED");

    buck.test(&with(&[
        "--include",
        "label-fail",
        "--exclude",
        "label-pass",
    ]))
    .await
    .expect_failure()
    .assert_stderr_matches("1 TESTS FAILED");

    buck.test(&cmd)
        .await
        .expect_failure()
        .assert_stderr_matches("1 TESTS FAILED");

    buck.test(&with(&["--include", "label-pass"])).await?;

    buck.test(&with(&["--exclude", "label-fail"])).await?;

    buck.test(&with(&["--include", "!label-fail"])).await?;

    buck.test(&with(&[
        "--include",
        "label-fail",
        "--exclude",
        "label-fail",
        "--always-exclude",
    ]))
    .await?;

    buck.test(&with(&["--include", "!label-fail", "label-fail"]))
        .await?;
    Ok(())
}

// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_label_filtering_True(buck: &Buck) -> anyhow::Result<()> {
    label_filtering(buck, true).await
}

// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_label_filtering_False(buck: &Buck) -> anyhow::Result<()> {
    label_filtering(buck, false).await
}

#[buck_test(inplace = true, skip_for_os = MAC_AND_WINDOWS)]
async fn test_name_filtering(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/python/test/...",
        "--",
        "test_env",
    ])
    .await?;

    buck.test(&[
        "fbcode//buck2/tests/targets/rules/python/test/...",
        "--",
        "test_fail",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("1 TESTS FAILED");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_compile_error(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&[
        "fbcode//buck2/tests/targets/compile_error:cpp_test_compile_error",
        get_mode_from_platform(Mode::Dev),
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("#error Compile error.*1 BUILDS FAILED.*NO TESTS RAN");
    Ok(())
}

// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_cwd(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["fbcode//buck2/tests/targets/rules/sh_test:test_cwd"])
        .await?;
    Ok(())
}

// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_default_label_filtering(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/sh_test:test_fail_extended",
        "--",
        "--extended-tests",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("1 TESTS FAILED");

    // Ignores it by default
    buck.test(&["fbcode//buck2/tests/targets/rules/sh_test:test_fail_extended"])
        .await?;
    Ok(())
}

// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_stress_runs(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/sh_test:test_fail",
        "--",
        "--stress-runs",
        "10",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("10 TESTS FAILED");
    Ok(())
}

// Not-in-place tests cannot run with deployed buck2
#[buck_test(
    inplace = false,
    data_dir = "testsof",
    env = [("BUCK_LOG", "buck2_test::command=debug")],
    skip_final_kill = is_deployed_buck2()
)]
async fn test_target_compatibility(buck: &Buck) -> anyhow::Result<()> {
    if is_deployed_buck2() {
        return Ok(());
    }

    let out = buck
        .test(&["//...", "--target-platforms", "//:platform_default_tests"])
        .await?;

    assert!(out.stderr.contains("target incompatible node"));

    buck.test(&[
        "//:foo_extra_test",
        "--target-platforms",
        "//:platform_default_tests",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("incompatible");
    Ok(())
}

// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_external_runner_test_info_options(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["fbcode//buck2/tests/targets/rules/external_runner_test_info/..."])
        .await?;
    Ok(())
}

// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_allow_tests_on_re(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/external_runner_test_info/...",
        "--unstable-allow-tests-on-re",
    ])
    .await?;
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_incompatible_tests_do_not_run_on_re(buck: &Buck) -> anyhow::Result<()> {
    // Trying to run a `local_only = True` action on RE is an infra failure
    // (Buck2InfraError), not a fatal error.
    let result = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/external_runner_test_info:invalid_test",
            "-c",
            "external_runner_test_info.declare_invalid_test=1",
        ])
        .await?;
    assert!(result.stderr.contains("Infra Failure 1"));
    assert!(
        result
            .stderr
            .contains("Trying to execute a `local_only = True` action on remote executor")
    );
    assert!(!result.stderr.contains("FATAL"));
    Ok(())
}

#[buck_test(inplace = true, env = [("TEST_MAKE_IT_FAIL", "1")])]
async fn test_env_var_filtering(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/python/test:test",
        get_mode_from_platform(Mode::Dev),
    ])
    .await?;

    buck.test(&[
        "fbcode//buck2/tests/targets/rules/python/test:test",
        get_mode_from_platform(Mode::Dev),
        "--",
        "--env",
        "TEST_MAKE_IT_FAIL=1",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("1 TESTS FAILED");
    Ok(())
}

/// The environment of the test's executable in the configuration `--no-run-output-test-commands-for-fdb`
/// wrote to `out`, after checking that the test supports debugging.
fn debuggable_executable_env(out: &Path) -> anyhow::Result<serde_json::Map<String, Value>> {
    let config: Value = serde_json::from_str(&std::fs::read_to_string(out)?)?;

    // Expect python/test:test target to support debugging. Executable field is populated only when debugging is supported.
    let debuggers = config
        .get("debuggers")
        .with_context(|| format!("No `debuggers` in {config}"))?;
    assert!(
        !debuggers
            .as_array()
            .with_context(|| format!("`debuggers` is not a list: {debuggers}"))?
            .is_empty()
    );
    let executable = config
        .get("executable")
        .with_context(|| format!("No `executable` in {config}"))?;
    Ok(executable
        .get("env")
        .and_then(Value::as_object)
        .with_context(|| format!("No `env` object in the executable {executable}"))?
        .clone())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_prepare_for_local_execution_env_with_env_cli_parameter(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let out = tmp_path.join("out");
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/python/test:test",
        "--",
        "--env",
        "EXTRA_VAR=foo",
        "--no-run-output-test-commands-for-fdb",
        out.to_str().context("A non-UTF-8 path")?,
    ])
    .await?;

    let env = debuggable_executable_env(&out)?;
    assert!(env.contains_key("PWD"));
    assert!(env.contains_key("EXTRA_VAR"));
    Ok(())
}

// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"], env = [("EXTRA_VAR", "foo")])]
async fn test_prepare_for_local_execution_env(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let out = tmp_path.join("out");
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/python/test:test",
        "--",
        "--no-run-output-test-commands-for-fdb",
        out.to_str().context("A non-UTF-8 path")?,
    ])
    .await?;

    let env = debuggable_executable_env(&out)?;
    assert!(env.contains_key("PWD"));
    assert!(!env.contains_key("EXTRA_VAR"));
    Ok(())
}

#[buck_test(inplace = true, env = [("BUCK2_TEST_TPX_USE_TCP", "true")])]
async fn test_tcp(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/python/test:test",
        get_mode_from_platform(Mode::Dev),
    ])
    .await?;
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_passing_test_names_are_not_shown(buck: &Buck) -> anyhow::Result<()> {
    // Passing test headers are not shown unless we pass --print-passing-details explicitly.
    let tests = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:test",
            get_mode_from_platform(Mode::Dev),
        ])
        .await?;
    assert!(
        !tests
            .stderr
            .contains("Pass: fbcode//buck2/tests/targets/rules/python/test:test - test")
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_failing_test_names_are_shown(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/python/test:test",
        get_mode_from_platform(Mode::Dev),
        "--",
        "--env",
        "TEST_ENV=fail",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Fail: fbcode//buck2/tests/targets/rules/python/test:test - test");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_no_print_passing_details(buck: &Buck) -> anyhow::Result<()> {
    // Without --print-passing-details, test headers and stdout is NOT displayed.
    let tests = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:test",
            get_mode_from_platform(Mode::Dev),
        ])
        .await?;
    assert!(
        !tests
            .stderr
            .contains("Pass: fbcode//buck2/tests/targets/rules/python/test:test - test")
    );
    assert!(!tests.stderr.contains("TESTED!"));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_print_passing_details(buck: &Buck) -> anyhow::Result<()> {
    // With --print-passing-details, test headers and stdout is displayed.
    let tests = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:test",
            get_mode_from_platform(Mode::Dev),
            "--",
            "--print-passing-details",
        ])
        .await?;
    assert!(
        tests
            .stderr
            .contains("Pass: fbcode//buck2/tests/targets/rules/python/test:test - test")
    );
    assert!(tests.stderr.contains("TESTED!"));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_no_no_print_details(buck: &Buck) -> anyhow::Result<()> {
    // Without --no-print-details the stack trace is displayed.
    let failure = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:test",
            get_mode_from_platform(Mode::Dev),
            "--",
            "--env",
            "TEST_ENV=fail",
        ])
        .await
        .expect_failure();
    assert!(remove_ansi_escape_sequences(&failure.stderr).contains("AssertionError: 41 != 42"));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_no_print_details(buck: &Buck) -> anyhow::Result<()> {
    // With --no-print-details the stack trace is not displayed.
    let tests = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:test",
            "--",
            "--env",
            "TEST_ENV=fail",
            "--no-print-details",
        ])
        .await
        .expect_failure();
    assert!(!tests.stderr.contains("AssertionError: 41 != 42"));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_bundle_sharding(buck: &Buck) -> anyhow::Result<()> {
    let tests = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:multi_tests",
            get_mode_from_platform(Mode::Dev),
        ])
        .await?;
    assert!(tests.stderr.contains("Pass 4"));
    Ok(())
}

/// This test starts a test that writes its PID to a file then runs for 60
/// seconds. We test cancellation by sending a CTRL+C as soon as a test
/// starts. We then check that the process exited, and that nothing else
/// started (or if anything did, that they stopped).
// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_cancellation(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // Make sure we are ready to go
    buck.build(&[
        "fbcode//buck2/tests/targets/rules/python/test:cancellation",
        "--build-test-info",
    ])
    .await?;

    let pids_env = format!("PIDS={}", tmp_path.display());
    let tests = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:cancellation",
            "--",
            "--stress-runs",
            "10",
            "--env",
            "SLOW_DURATION=60",
            "--env",
            &pids_env,
        ])
        .spawn();

    let mut started = false;
    for _i in 0..30 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if std::fs::read_dir(tmp_path)?.next().is_some() {
            started = true;
            break;
        }
    }
    anyhow::ensure!(started, "Tests never started");

    interrupt(&tests)?;
    // Wait for the command to exit
    let _ignored = tests.wait().await;

    // Give stuff time to settle, PIDS don't necessarily disappear
    // instantly. Also, verify that we are not starting more tests.
    tokio::time::sleep(Duration::from_secs(5)).await;

    // At this point, nothing should be alive.
    for entry in std::fs::read_dir(tmp_path)? {
        let pid = entry?.file_name();
        let pid = pid.to_string_lossy();
        let pid: u32 = pid
            .parse()
            .with_context(|| format!("`{pid}` is not a pid"))?;
        anyhow::ensure!(!daemon_is_alive(pid), "PID existed: {pid}");
    }
    Ok(())
}

/// This test starts a test on RE, waits for it to start, cancels, then starts
/// again and verifies we don't wait for the test to finish.
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_cancellation_on_re(buck: &Buck) -> anyhow::Result<()> {
    // Make sure we are ready to go
    buck.build(&[
        "fbcode//buck2/tests/targets/rules/python/test:cancellation_re",
        get_mode_from_platform(Mode::Dev),
        "--build-test-info",
    ])
    .await?;

    // The test must still be sleeping by the time we cancel it, however long
    // RE took to schedule it.
    let slow_duration = 600;
    // RE scheduling latency is unbounded in practice, so allow plenty of time
    // for the test to start before giving up.
    let start_timeout = 300;

    let slow_duration_env = format!("SLOW_DURATION={slow_duration}");
    let tests = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:cancellation_re",
            get_mode_from_platform(Mode::Dev),
            "--unstable-force-tests-on-re",
            "--remote-only",
            "--no-remote-cache",
            "--",
            "--env",
            &slow_duration_env,
            "--env",
            "PIDS=/tmp",
        ])
        .spawn();

    async fn has_started(buck: &Buck) -> bool {
        // The log is truncated here so this can exit non-zero.
        let stdout = buck
            .log(&["what-ran"])
            .await
            .unwrap_or_else(BuckError::into_output)
            .stdout;

        // what-ran returns things that started
        stdout.contains("test.run")
    }

    let mut started = false;
    for _i in 0..start_timeout {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if has_started(buck).await {
            started = true;
            break;
        }
    }
    anyhow::ensure!(
        started,
        "Tests never started within {start_timeout} seconds"
    );

    interrupt(&tests)?;
    // Cancellation must tear the test down instead of waiting it out.
    let _ignored = tokio::time::timeout(Duration::from_secs(60), tests.wait())
        .await
        .context("The cancelled test command did not exit within 60 seconds")?;

    // Run a command that cannot execute concurrerntly and check it does not
    // wait for the cancelled test, which means we went idle.
    tokio::time::timeout(
        Duration::from_secs(10),
        buck.audit_config(&["-c", "foo.bar=True"]),
    )
    .await
    .context("`audit config` waited for the cancelled test")??;
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_timeout_local(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:timeout",
            "--local-only",
            "--no-remote-cache",
            "--",
            "--env",
            "SLOW_DURATION=60",
            "--timeout=5",
        ])
        .await
        .expect_failure();
    result.assert_stderr_matches("Timeout: fbcode//buck2/tests/targets/rules/python/test:timeout");
    assert!(result.stderr.contains("1 TESTS TIMED OUT"));
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_timeout_re(buck: &Buck) -> anyhow::Result<()> {
    // The target's `generate_build_info_json` action is `local_only` in modes
    // that use full build info, so it cannot be built under `--remote-only`.
    buck.build(&[
        "fbcode//buck2/tests/targets/rules/python/test:timeout_re",
        get_mode_from_platform(Mode::Dev),
        "-c",
        "build.execution_platform_rustup_rustc=true",
        "--build-test-info",
    ])
    .await?;

    let result = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:timeout_re",
            get_mode_from_platform(Mode::Dev),
            "-c",
            "build.execution_platform_rustup_rustc=true",
            "--unstable-allow-all-tests-on-re",
            "--remote-only",
            "--no-remote-cache",
            "--",
            "--env",
            "SLOW_DURATION=60",
            "--timeout=5",
        ])
        .await
        .expect_failure();
    result
        .assert_stderr_matches("Timeout: fbcode//buck2/tests/targets/rules/python/test:timeout_re");
    assert!(result.stderr.contains("1 TESTS TIMED OUT"));
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_timeout_and_failure_local(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:timeout_and_fail",
            "--local-only",
            "--no-remote-cache",
            "--",
            "--env",
            "SLOW_DURATION=60",
            "--timeout=5",
        ])
        .await
        .expect_failure();
    result.assert_stderr_matches("1 TESTS FAILED.*1 TESTS TIMED OUT");
    let stderr = remove_ansi_escape_sequences(&result.stderr);
    assert!(stderr.contains(
        "Tests finished: Pass 0. Fail 1. Timeout 1. Fatal 0. Skip 0. Omit 0. Infra Failure 0. Build failure 0"
    ));
    Ok(())
}

/// If an overall timeout is set, we expect that to result in OMITs
/// reported in Tpx, and Tpx does not set an error status for that.
///
/// We're OK with that, we will report how many OMITs there were.
/// The caller is expected to be aware of how this feature works.
#[buck_test(
    inplace = true,
    skip_for_os = ["windows"],
    skip_final_kill = is_deployed_buck2()
)]
async fn test_overall_timeout(buck: &Buck) -> anyhow::Result<()> {
    if is_deployed_buck2() {
        return Ok(());
    }

    buck.test(&[
        "fbcode//buck2/tests/targets/rules/python/test:timeout",
        "--local-only",
        "--no-remote-cache",
        "--overall-timeout",
        "5s",
        "--",
        "--env",
        "SLOW_DURATION=60",
    ])
    .await?;
    Ok(())
}

/// A test that fails before the deadline fires still owns the exit code. Tpx's
/// exit code carries information we cannot reconstruct from the errors -- it
/// reserves values to request follow-up runs -- so an expired deadline must not
/// replace it.
#[buck_test(
    inplace = true,
    skip_for_os = ["windows"],
    skip_final_kill = is_deployed_buck2()
)]
async fn test_overall_timeout_keeps_test_executor_exit_code(buck: &Buck) -> anyhow::Result<()> {
    if is_deployed_buck2() {
        return Ok(());
    }

    // Build first so the deadline only has to cover execution, not compilation.
    buck.build(&[
        "fbcode//buck2/tests/targets/rules/python/test:test_fail",
        "fbcode//buck2/tests/targets/rules/python/test:timeout",
        "--local-only",
        "--no-remote-cache",
    ])
    .await?;

    let result = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:test_fail",
            "fbcode//buck2/tests/targets/rules/python/test:timeout",
            "--local-only",
            "--no-remote-cache",
            "--overall-timeout",
            "30s",
            "--",
            "--env",
            "SLOW_DURATION=120",
        ])
        .await
        .expect_failure();
    assert!(remove_ansi_escape_sequences(&result.stderr).contains("1 TESTS FAILED"));

    let executor_exit_codes = filter_events(
        buck,
        &["Result", "result", "test_response", "executor_exit_code"],
    )
    .await?;
    let [executor_exit_code] = executor_exit_codes.as_slice() else {
        panic!("Expected one executor exit code, got {executor_exit_codes:?}");
    };
    let executor_exit_code = executor_exit_code.as_i64().with_context(|| {
        format!("An executor exit code that is not a number: {executor_exit_code}")
    })?;
    assert_ne!(
        executor_exit_code, 0,
        "Expected Tpx to report the failing test"
    );
    assert_eq!(result.exit_code().map(i64::from), Some(executor_exit_code));
    Ok(())
}

async fn test_env(buck: &Buck, test: &str) -> anyhow::Result<()> {
    let test = format!("fbcode//buck2/tests/targets/rules/sh_test:{test}");

    buck.test(&[&test]).await?;

    // Check run also works. Note that those tests run from `fbcode` by default
    // so no chdir needed here.
    buck.run(&[&test]).await?;
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_test_env_requires_env(buck: &Buck) -> anyhow::Result<()> {
    test_env(buck, "requires_env").await
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_test_env_requires_env_location(buck: &Buck) -> anyhow::Result<()> {
    test_env(buck, "requires_env_location").await
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_exit_code(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .test(&["fbcode//buck2/tests/targets/rules/sh_test:test_fail"])
        .await
        .expect_failure();
    assert_eq!(result.exit_code(), Some(32));
    let result = buck.test(&["not//a/real:target"]).await.expect_failure();
    assert_eq!(result.exit_code(), Some(ExitCode::UserError as i32));
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_skip_missing_targets(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["fbcode//buck2/tests/targets/rules/python/test:not_a_thing"])
        .await
        .expect_failure()
        .assert_stderr_matches("Unknown target `not_a_thing`");

    let res = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:not_a_thing",
            "--skip-missing-targets",
        ])
        .await?;

    assert!(res.stderr.contains("Skipped 1 missing targets:"));
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_test_worker(buck: &Buck) -> anyhow::Result<()> {
    let worker_args = [
        "-c",
        "build.use_persistent_workers=True",
        "--local-only",
        "--no-remote-cache",
    ];
    buck.test(
        &[
            &worker_args[..],
            &["fbcode//buck2/tests/targets/rules/worker_grpc:worker_test"],
        ]
        .concat(),
    )
    .await?;
    Ok(())
}

#[buck_test(
    inplace = true,
    write_invocation_record = true,
    env = [("TEST_MAKE_IT_FAIL", "1")]
)]
async fn test_failed_tests_has_error_category(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python/test:test",
            get_mode_from_platform(Mode::Dev),
            "--",
            "--env",
            "TEST_MAKE_IT_FAIL=1",
        ])
        .await
        .expect_failure();
    res.assert_stderr_matches("1 TESTS FAILED");
    let record = res.invocation_record()?;
    let errors = record
        .get("errors")
        .and_then(Value::as_array)
        .with_context(|| format!("No `errors` in the invocation record {:?}", *record))?;

    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].get("category"), Some(&Value::from("USER")));
    if !is_deployed_buck2() {
        assert_eq!(
            errors[0].get("category_key"),
            Some(&Value::from("TEST_FAILED"))
        );
    }
    Ok(())
}
