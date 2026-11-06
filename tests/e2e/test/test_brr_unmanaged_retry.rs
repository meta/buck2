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

use anyhow::Context;
use buck2_e2e::BuckError;
use buck2_e2e::ansi::remove_ansi_escape_sequences;
use buck2_e2e::mode::Mode;
use buck2_e2e::mode::get_mode_from_platform;
use buck2_e2e::prelude::*;
use serde_json::Value;

// (target, broken_method) pairs exercised by the BRR roundtrip below. Each target
// is an unmanaged test (TestCaseSelector::None): TPX runs it as a black box with
// no listing, but the testpilot adapter still reports the individual method
// result "target - <broken_method> (...)". `broken_method` is the substring that
// must show up in the failure report's test_names and be preserved across the BRR
// retry. Add a (target, method) pair here, and a case of each test named after the
// target, to cover a new unmanaged target shape.
const BROKEN_UNMANAGED_TEST: (&str, &str) = (
    "fbcode//testinfra/playground/python/broken_unmanaged:broken_unmanaged_test",
    "test_the_test",
);

/// The `test_names` of a failure report, which has none if it has no such field.
fn report_test_names(report: &Value) -> anyhow::Result<Vec<&str>> {
    let Some(test_names) = report.get("test_names") else {
        return Ok(Vec::new());
    };
    test_names
        .as_array()
        .with_context(|| format!("`test_names` is not a list: {report}"))?
        .iter()
        .map(|name| {
            name.as_str()
                .with_context(|| format!("A test name that is not a string: {name}"))
        })
        .collect()
}

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_brr_roundtrip_unmanaged_failure_broken_unmanaged_test(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    brr_roundtrip_unmanaged_failure(buck, tmp_path, BROKEN_UNMANAGED_TEST).await
}

/// End-to-end BRR roundtrip for an unmanaged test.
///
/// The testpilot adapter reports the individual method name, so the BRR file
/// records "target - <broken_method> (...)" (not the suite synthetic). On retry,
/// the test is still unmanaged (TestCaseSelector::None), so the filter matches
/// the BRR name against the suite synthetic ("- main"/"- unmanaged"), which
/// does not match the individual name -- the suite is silently filtered out and
/// NO TESTS RAN.
///
/// This test asserts the correct behavior: the retry should actually run the
/// test and reproduce the failure.
async fn brr_roundtrip_unmanaged_failure(
    buck: &Buck,
    tmp_path: &Path,
    target_and_method: (&str, &str),
) -> anyhow::Result<()> {
    let (target, broken_method) = target_and_method;
    let mode = get_mode_from_platform(Mode::Dev);
    let report_file = tmp_path.join("report.json");
    let retry_report_file = tmp_path.join("retry_report.json");
    let report_file_arg = report_file.to_str().context("A non-UTF-8 path")?;
    let retry_report_file_arg = retry_report_file.to_str().context("A non-UTF-8 path")?;

    // Step 1: Run the test with a failure. TPX_PLAYGROUND_FAIL makes the
    // test method fail, so TPX records "target - <broken_method> (...)" as a
    // failed case.
    buck.test(&[
        target,
        mode,
        "--",
        "--env",
        "TPX_PLAYGROUND_FAIL=1",
        "--save-failures-for-retry-in-file",
        report_file_arg,
    ])
    .await
    .expect_failure();

    // Step 2: Verify the report was written and contains the individual test
    // method name, NOT just the suite synthetic. This is the key difference
    // from the suite-level tests in test_brr_suite_level_retry.rs.
    assert!(report_file.exists(), "Failure report was not written");
    let report: Value = serde_json::from_str(&std::fs::read_to_string(&report_file)?)?;
    let test_names = report_test_names(&report)?;
    let has_the_test = test_names.iter().any(|name| name.contains(broken_method));
    assert!(
        has_the_test,
        "Expected a '{broken_method}' entry in test_names, got: {test_names:?}"
    );

    // Step 3: Feed the report back as a BRR retry input (still with the
    // failure trigger), saving a retry report. The retry must reproduce the
    // failure, so a failed command is required here -- a passing retry means the
    // suite was not actually re-run.
    buck.test(&[
        target,
        mode,
        "--",
        "--env",
        "TPX_PLAYGROUND_FAIL=1",
        "--base-rev-retry-with-input-file",
        report_file_arg,
        "--save-failures-for-retry-in-file",
        retry_report_file_arg,
    ])
    .await
    .expect_failure();

    // Step 4: The retry must actually run the test (not silently filter it
    // out). If the suite was filtered out, the retry report file is either
    // missing or empty (TPX creates the file but writes nothing when the
    // suite is filtered out).
    let retry_report_text = if retry_report_file.exists() {
        std::fs::read_to_string(&retry_report_file)?
    } else {
        String::new()
    };
    assert!(
        !retry_report_text.trim().is_empty(),
        "Retry failure report is missing or empty -- the unmanaged suite was \
         likely filtered out because the BRR filter had the individual test \
         name (e.g. '{broken_method}') that does not match the suite synthetic \
         ('- main'/'- unmanaged') for TestCaseSelector::None"
    );
    let retry_report: Value = serde_json::from_str(&retry_report_text)?;
    let retry_test_names = report_test_names(&retry_report)?;
    let retry_has_the_test = retry_test_names
        .iter()
        .any(|name| name.contains(broken_method));
    assert!(
        retry_has_the_test,
        "Expected '{broken_method}' in retry report test_names, got: {retry_test_names:?}"
    );
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_brr_transient_unmanaged_failure_runs_tests_broken_unmanaged_test(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    brr_transient_unmanaged_failure_runs_tests(buck, tmp_path, BROKEN_UNMANAGED_TEST).await
}

/// Transient failure for an unmanaged test: the test method fails on the first
/// run (TPX_PLAYGROUND_FAIL), but passes on the BRR retry.
///
/// The BRR filter must not silently drop the unmanaged suite just because the
/// individual test name "target - <broken_method> (...)" does not match the
/// suite synthetic ("- main"/"- unmanaged"). The retry should run the full
/// suite and report a passing result.
async fn brr_transient_unmanaged_failure_runs_tests(
    buck: &Buck,
    tmp_path: &Path,
    target_and_method: (&str, &str),
) -> anyhow::Result<()> {
    let (target, broken_method) = target_and_method;
    let mode = get_mode_from_platform(Mode::Dev);
    let report_file = tmp_path.join("report.json");
    let retry_report_file = tmp_path.join("retry_report.json");
    let report_file_arg = report_file.to_str().context("A non-UTF-8 path")?;
    let retry_report_file_arg = retry_report_file.to_str().context("A non-UTF-8 path")?;

    // Step 1: Run with TPX_PLAYGROUND_FAIL=1 to get a failure.
    buck.test(&[
        target,
        mode,
        "--",
        "--env",
        "TPX_PLAYGROUND_FAIL=1",
        "--save-failures-for-retry-in-file",
        report_file_arg,
    ])
    .await
    .expect_failure();

    // Step 2: Verify the report has the individual test name.
    assert!(report_file.exists(), "Failure report was not written");
    let report: Value = serde_json::from_str(&std::fs::read_to_string(&report_file)?)?;
    let test_names = report_test_names(&report)?;
    let has_the_test = test_names.iter().any(|name| name.contains(broken_method));
    assert!(
        has_the_test,
        "Expected '{broken_method}' in test_names, got: {test_names:?}"
    );

    // Step 3: Feed the report back WITHOUT TPX_PLAYGROUND_FAIL — the test
    // passes this time. The BRR filter must let the unmanaged suite through
    // so that a result is produced.
    let stderr = buck
        .test(&[
            target,
            mode,
            "--",
            "--base-rev-retry-with-input-file",
            report_file_arg,
            "--save-failures-for-retry-in-file",
            retry_report_file_arg,
        ])
        .await
        .unwrap_or_else(BuckError::into_output)
        .stderr;

    // Step 4: The retry must have produced a result — not "NO TESTS RAN".
    // The suite should not be silently dropped by the filter just because
    // the BRR file had an individual test name that does not match the suite
    // synthetic.
    let stderr = remove_ansi_escape_sequences(&stderr);
    assert!(
        !stderr.contains("NO TESTS RAN"),
        "BRR retry with a transient unmanaged failure should produce results, \
         not silently skip the unmanaged suite. This happens because the BRR \
         filter has 'target - {broken_method} (...)' but the suite synthetic is \
         '- main'/'- unmanaged' for TestCaseSelector::None."
    );
    assert!(stderr.contains("Pass"));
    Ok(())
}
