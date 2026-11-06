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
use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Context;
use buck2_e2e::BuckError;
use buck2_e2e::BuckOutput;
use buck2_e2e::ansi::remove_ansi_escape_sequences;
use buck2_e2e::mode::Mode;
use buck2_e2e::mode::get_mode_from_platform;
use buck2_e2e::prelude::*;
use regex::Regex;
use serde_json::Value;
use serde_json::json;

/// Read a JSONL BRR report file and merge all lines into a single object.
///
/// TPX writes one JSON object per line (one per acked failure). This helper
/// parses every line and collects all test_names into a single combined object
/// so callers can inspect the aggregate report.
fn read_brr_report(path: &Path) -> anyhow::Result<Value> {
    let lines = std::fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).with_context(|| format!("Parsing `{line}`")))
        .collect::<anyhow::Result<Vec<Value>>>()?;
    assert!(
        !lines.is_empty(),
        "BRR report file is empty: {}",
        path.display()
    );
    let mut merged = lines[0]
        .as_object()
        .with_context(|| format!("A BRR report line that is not an object: {}", lines[0]))?
        .clone();
    let mut all_test_names: Vec<Value> = Vec::new();
    for line in &lines {
        if let Some(test_names) = line.get("test_names") {
            all_test_names.extend(
                test_names
                    .as_array()
                    .with_context(|| format!("`test_names` is not a list: {line}"))?
                    .iter()
                    .cloned(),
            );
        }
    }
    merged.insert("test_names".to_owned(), Value::Array(all_test_names));
    Ok(Value::Object(merged))
}

/// The `test_names` of a merged BRR report.
fn report_test_names(report: &Value) -> anyhow::Result<Vec<&str>> {
    report
        .get("test_names")
        .and_then(Value::as_array)
        .with_context(|| format!("No `test_names` list in {report}"))?
        .iter()
        .map(|name| {
            name.as_str()
                .with_context(|| format!("A test name that is not a string: {name}"))
        })
        .collect()
}

// status_detailed codes in the tpx event log. See test_result::TestResultStatus.
const TEST_NOT_EXISTS_STATUS: i64 = 404;
const FAILED_STATUS: i64 = 501;
const FATAL_STATUS: i64 = 502;

/// The events of a JSONL event log, without its blank lines and lines that are not JSON. A
/// missing log has none.
fn event_log_events(path: &Path) -> anyhow::Result<Vec<Value>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(std::fs::read_to_string(path)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// True if any event in the JSONL event log is a TEST_NOT_EXISTS phantom.
fn event_log_has_test_not_exists(path: &Path) -> anyhow::Result<bool> {
    Ok(event_log_events(path)?.iter().any(|event| {
        event.get("status_detailed").and_then(Value::as_i64) == Some(TEST_NOT_EXISTS_STATUS)
    }))
}

/// Map of test_name -> status_detailed for every result row in the event log.
fn event_log_results(path: &Path) -> anyhow::Result<BTreeMap<String, Value>> {
    let mut results = BTreeMap::new();
    for event in event_log_events(path)? {
        let Some(status) = event.get("status_detailed").filter(|s| !s.is_null()) else {
            continue;
        };
        let Some(name) = event.get("test_name") else {
            continue;
        };
        let name = name
            .as_str()
            .with_context(|| format!("A `test_name` that is not a string: {event}"))?;
        results.insert(name.to_owned(), status.clone());
    }
    Ok(results)
}

/// Names of tests that produced a real result row (have a status_detailed)
/// in the JSONL event log. Excludes non-result events like the run_id line.
fn event_log_result_names(path: &Path) -> anyhow::Result<BTreeSet<String>> {
    Ok(event_log_results(path)?.into_keys().collect())
}

/// Print the TestX testrun URL parsed from a buck.test output, labelled by
/// run type (e.g. "target run" / "base-rev retry"). Each contract does two
/// buck.test calls, so this surfaces both testruns for inspection.
fn print_testrun_url(label: &str, output: &str) {
    let testrun = Regex::new(r"testinfra/testrun/\d+").unwrap();
    let url = match testrun.find(output) {
        Some(m) => format!("https://www.internalfb.com/intern/{}", m.as_str()),
        None => "<no testrun>".to_owned(),
    };
    eprintln!("[BRR-contract] {label}: {url}");
}

/// The run id in the test run URL a `buck2 test` printed.
fn run_id(stderr: &str) -> Option<String> {
    let testrun = Regex::new(r"testinfra/testrun/(\d+)").unwrap();
    testrun.captures(stderr).map(|c| c[1].to_owned())
}

/// The stderr of a `buck2 test` that may fail.
fn stderr_of(result: Result<BuckOutput, BuckError>) -> String {
    result.unwrap_or_else(BuckError::into_output).stderr
}

/// A BRR input file for `test_names` of the suite `target`, after the test run `run_id`.
fn write_brr_file(
    path: &Path,
    target: &str,
    test_names: &[&str],
    run_id: &str,
) -> anyhow::Result<()> {
    let brr_data = json!({
        "suite_name": target,
        "fully_qualified_target_name": target,
        "test_names": test_names,
        "run_id": run_id,
        "test_type": 5,
        "test_config": {
            "config": "",
            "host": "linux",
            "mode": "@fbcode//mode/dev",
        },
    });
    std::fs::write(path, brr_data.to_string())?;
    Ok(())
}

fn path_arg(path: &Path) -> anyhow::Result<&str> {
    path.to_str().context("A non-UTF-8 path")
}

const PYTHON_TEST_TARGET: &str = "fbcode//buck2/tests/targets/rules/python/test:test";

// Bundle-mode target with 3 test cases (test_a, test_b, test_c). Used for
// the SRR `--exact` tests below because the SRR path needs to demonstrate
// both single-test selection and suite-level expansion against a target
// with multiple discoverable tests.
const PYTHON_MULTI_TESTS_TARGET: &str = "fbcode//buck2/tests/targets/rules/python/test:multi_tests";

// Full match-name for `multi_tests.TestCase.test_a` in the format tpx's
// `--exact` filter compares against: `<suite> - <display_name>`, where
// `<display_name>` is `<method> (<full.module.path.Class>)` per the legacy
// python unittest listing parser (tpx-buck/src/listing/python.rs).
fn test_a_match_name() -> String {
    format!(
        "{PYTHON_MULTI_TESTS_TARGET} - test_a \
         (buck2.tests.targets.rules.python.test.multi_tests.TestCase)"
    )
}

fn main_match_name() -> String {
    format!("{PYTHON_MULTI_TESTS_TARGET} - main")
}

// Auto-generated pyrefly type-checking target. It is UNMANAGED (non-listable):
// it reports per-file results and never goes through test-case listing — the same
// shape as the `:llama4x-static-type-check` target in the original Bug 7 report.
const TYPE_CHECK_UNMANAGED_TARGET: &str =
    "fbcode//testinfra/playground/python/pytest:pytest_test-library-type-checking";

// Landable unmanaged sh_test whose failure is driven on demand by the
// BRR_SIM_FAIL env var (passes in prod CI; the e2e flips it via `--env`). Used by
// the two BRR contracts below to simulate a *failing* unmanaged test.
const BRR_SIM_UNMANAGED_TARGET: &str = "fbcode//testinfra/playground/sh:brr_sim_unmanaged";

// Same on-demand-failing unmanaged sh_test as BRR_SIM_UNMANAGED_TARGET, but
// carrying the `tpx:skip-filtering-unmanaged-test-suite` label (cogwheel /
// audit_dependencies style). Used to prove the *labeled* case below.
const BRR_SIM_UNMANAGED_SKIP_FILTERING_TARGET: &str =
    "fbcode//testinfra/playground/sh:brr_sim_unmanaged_skip_filtering";

const BROKEN_RUN_AS_BUNDLE_TARGET: &str =
    "fbcode//testinfra/playground/python/broken_run_as_bundle:broken_run_as_bundle_test";

const BROKEN_LISTING_TARGET: &str =
    "fbcode//testinfra/playground/python/broken_listing:broken_listing_test";

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_srr_exact_with_main_expands_to_full_suite(buck: &Buck) -> anyhow::Result<()> {
    // GIVEN: a bundle-mode python target with 3 discoverable test cases
    // (test_a, test_b, test_c) and the patched tpx whose `--exact` filter
    // converts synthetic `- main` / `- unmanaged` suffixes into prefix
    // regexes `^<suite> - ` (suite-level expansion).
    let mode = get_mode_from_platform(Mode::Dev);

    // WHEN: SRR invokes tpx with two `--exact` names — a real existing
    // test case plus the synthetic suite-level `- main`. This is the SRR
    // shape produced when TARGET had both a specific test failure and a
    // suite-level fatal.
    let stderr = stderr_of(
        buck.test(&[
            PYTHON_MULTI_TESTS_TARGET,
            mode,
            "--",
            "--exact",
            &test_a_match_name(),
            &main_match_name(),
        ])
        .await,
    );

    // THEN: the synthetic `- main` entry expands to `^<target> - ` and
    // matches all 3 discovered cases (test_a/b/c). tpx runs them as a
    // bundle so the bundle worker reports too — Pass 4 total (3 children +
    // main worker). SRR now uploads real AV data for the suite-level
    // failure instead of silently running just the specific test.
    let stderr = remove_ansi_escape_sequences(&stderr);
    assert!(
        stderr.contains("Pass 4"),
        "Synthetic '- main' should expand to the whole suite, \
         running all 3 children + bundle worker = Pass 4, stderr was:\n{stderr}"
    );
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_srr_exact_single_test_stays_narrow(buck: &Buck) -> anyhow::Result<()> {
    // GIVEN: a bundle-mode python target with 3 discoverable test cases
    // (test_a, test_b, test_c) and any tpx version — this test's
    // assertion holds in both pre-fix and post-fix states (regression
    // guard ensuring the fix only widens for synthetic suite-level names).
    let mode = get_mode_from_platform(Mode::Dev);

    // WHEN: SRR invokes tpx with `--exact` on a single real existing test
    // case name. No synthetic `- main`/`- unmanaged` suffix is present.
    let stderr = stderr_of(
        buck.test(&[
            PYTHON_MULTI_TESTS_TARGET,
            mode,
            "--",
            "--exact",
            &test_a_match_name(),
        ])
        .await,
    );

    // THEN: exactly the one named test runs. The suite-level expansion
    // must NOT accidentally widen the filter when no synthetic entry is
    // present, or `--exact` would silently grow the set of tests SRR
    // executes for any caller. Bundle mode emits a synthetic `- main`
    // aggregate alongside the executed case, so the observed count is
    // `Pass 2` (test_a + bundle `- main`) both pre-fix and post-fix.
    let stderr = remove_ansi_escape_sequences(&stderr);
    assert!(
        stderr.contains("Pass 2"),
        "Regression guard: SRR --exact with a single real test name \
         should match only that one test, stderr was:\n{stderr}"
    );
    Ok(())
}

/// When a BRR input file contains a suite-level '- main' entry (indicating
/// a run_as_bundle test failed in the original run), the retry should run all
/// tests from that suite rather than filtering them all out.
#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_brr_suite_level_main_runs_all_tests(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);

    // Step 1: Run the test normally to get a valid run_id from the test infra
    let first_run = buck.test(&[PYTHON_TEST_TARGET, mode]).await?;
    let run_id = run_id(&first_run.stderr).context("Could not extract run_id from test output")?;

    // Step 2: Create BRR file with suite-level "- main" entry, simulating a
    // run_as_bundle failure in the original run
    let brr_file = tmp_path.join("brr_input.json");
    write_brr_file(
        &brr_file,
        PYTHON_TEST_TARGET,
        &[&format!("{PYTHON_TEST_TARGET} - main")],
        &run_id,
    )?;

    // Step 3: Run with BRR retry — the fix converts "- main" to a prefix
    // matcher so individual test cases from the suite are included.
    // BRR mode may exit non-zero (code 32) even when tests pass, so we
    // take stderr from whichever result we get.
    let stderr = stderr_of(
        buck.test(&[
            PYTHON_TEST_TARGET,
            mode,
            "--",
            "--base-rev-retry-with-input-file",
            path_arg(&brr_file)?,
        ])
        .await,
    );

    // Step 4: Assert tests actually ran (before the fix, this was "NO TESTS RAN")
    let stderr = remove_ansi_escape_sequences(&stderr);
    assert!(
        !stderr.contains("NO TESTS RAN"),
        "BRR retry with suite-level '- main' entry should run tests, not skip them"
    );
    assert!(stderr.contains("Pass"));
    Ok(())
}

/// Simulate, in a single BRR run, an unmanaged suite that has BOTH a real
/// pre-existing entry AND a synthetic "new" entry (a case name the suite will
/// never list). With the fix, the suite is cleared at discovery
/// (FbTestSuite::is_unmanaged), so NEITHER entry yields a phantom
/// TEST_NOT_EXISTS (404). Before the fix the synthetic per-case entry survived
/// the not-listed tracker and produced a phantom 404.
///
/// This also pins the known GAP: a genuinely-new unmanaged test cannot be
/// reported as "new". New-test detection relies on listing, which unmanaged
/// suites never emit, so the synthetic entry produces NO result at all (no 404,
/// no skip) — it is simply dropped. Land-blocking is unaffected (a real new
/// unmanaged failure still propagates as FAILED from the target run); only the
/// new-test classification/reporting is missing. Restoring it (via execution-diff
/// detection on the base rerun) is tracked as a follow-up.
#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_brr_unmanaged_existing_and_synthetic_new_no_phantom(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);

    // Step 1: Run the unmanaged type-check target normally to get a valid run_id.
    let first_run = buck.test(&[TYPE_CHECK_UNMANAGED_TARGET, mode]).await?;
    let run_id = run_id(&first_run.stderr).context("Could not extract run_id from test output")?;

    // Step 2: BRR file with a real existing unmanaged entry plus a synthetic new
    // per-case entry the suite will never list.
    let existing_name = format!("{TYPE_CHECK_UNMANAGED_TARGET} - unmanaged");
    let synthetic_new_name = format!("{TYPE_CHECK_UNMANAGED_TARGET} - brr_synthetic_new_test.py");
    let brr_file = tmp_path.join("brr_input.json");
    write_brr_file(
        &brr_file,
        TYPE_CHECK_UNMANAGED_TARGET,
        &[&existing_name, &synthetic_new_name],
        &run_id,
    )?;

    // Step 3: Base-revision retry with new-test detection enabled, capturing the
    // tpx event log. BRR mode may exit non-zero, so inspect the log regardless.
    let event_log = tmp_path.join("events.ndjson");
    let _ignored = buck
        .test(&[
            TYPE_CHECK_UNMANAGED_TARGET,
            mode,
            "--",
            "--base-rev-retry-with-input-file",
            path_arg(&brr_file)?,
            "--experiment",
            "report-new-test-not-exists-status-for-brr",
            "--event-log-file",
            path_arg(&event_log)?,
        ])
        .await;

    assert!(event_log.exists(), "Event log file was not written");

    // No phantom TEST_NOT_EXISTS for either entry (the fix clears unmanaged suites
    // at discovery). Pre-fix, the synthetic per-case entry produced a 404.
    assert!(
        !event_log_has_test_not_exists(&event_log)?,
        "Unmanaged suite must not produce a phantom TEST_NOT_EXISTS, even with a \
         synthetic new entry; event log was:\n{}",
        std::fs::read_to_string(&event_log)?
    );

    // The existing unmanaged suite still RUNS on the base rerun (clearing the
    // tracker does not suppress execution): its `- unmanaged` result is present.
    // The synthetic new entry produces NO result row of any kind (no 404, no
    // skip) — it is simply dropped. So only the synthetic one fails to run.
    let result_names = event_log_result_names(&event_log)?;
    assert!(
        result_names.contains(&existing_name),
        "Existing unmanaged test should still run on the BRR base rerun, but no \
         result was emitted for it. Results: {result_names:?}"
    );
    assert!(
        !result_names.contains(&synthetic_new_name),
        "Synthetic new unmanaged entry must not run / be reported (dropped at \
         discovery). Results: {result_names:?}"
    );
    Ok(())
}

/// CONTRACT — pre-existing failing unmanaged test.
///
/// An unmanaged test that failed on the target run and *fails again* on the
/// base-revision retry is a pre-existing failure (it exists on base). We drive
/// the failure on demand with `--env BRR_SIM_FAIL=1` so the target is still safe
/// to run in production (it passes by default).
///
/// The retry runs the real, discovered unmanaged suite, so the fix clears it at
/// discovery: there is NO phantom TEST_NOT_EXISTS, and the `- unmanaged` result
/// is actually produced (it ran and failed on base) — i.e. pre-existing.
#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_brr_unmanaged_preexisting_failure(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);
    let fail_env = ["--env", "BRR_SIM_FAIL=1"];

    // Step 1: target run — the unmanaged test FAILS on demand. Capture its run_id.
    let stderr = stderr_of(
        buck.test(&[&[BRR_SIM_UNMANAGED_TARGET, mode, "--"], &fail_env[..]].concat())
            .await,
    );
    let run_id = run_id(&stderr)
        .with_context(|| format!("Could not extract run_id from test output:\n{stderr}"))?;
    print_testrun_url("Contract 1 (pre-existing) target run", &stderr);

    // Step 2: BRR file naming the suite-level `- unmanaged` entry that failed.
    let existing_name = format!("{BRR_SIM_UNMANAGED_TARGET} - unmanaged");
    let brr_file = tmp_path.join("brr_input.json");
    write_brr_file(
        &brr_file,
        BRR_SIM_UNMANAGED_TARGET,
        &[&existing_name],
        &run_id,
    )?;

    // Step 3: base-revision retry — still failing on base (BRR_SIM_FAIL=1).
    let event_log = tmp_path.join("events.ndjson");
    let retry_stderr = stderr_of(
        buck.test(
            &[
                &[BRR_SIM_UNMANAGED_TARGET, mode, "--"],
                &fail_env[..],
                &[
                    "--base-rev-retry-with-input-file",
                    path_arg(&brr_file)?,
                    "--experiment",
                    "report-new-test-not-exists-status-for-brr",
                    "--event-log-file",
                    path_arg(&event_log)?,
                ],
            ]
            .concat(),
        )
        .await,
    );
    print_testrun_url("Contract 1 (pre-existing) base-rev retry", &retry_stderr);

    assert!(event_log.exists(), "Event log file was not written");
    let log = std::fs::read_to_string(&event_log)?;
    // Pre-existing: no phantom 404, and the suite actually ran on base.
    assert!(
        !event_log_has_test_not_exists(&event_log)?,
        "Pre-existing unmanaged failure must NOT be flagged TEST_NOT_EXISTS; \
         event log was:\n{log}"
    );
    let results = event_log_results(&event_log)?;
    let Some(status) = results.get(&existing_name) else {
        panic!(
            "Pre-existing unmanaged test should run on the base rerun and produce a \
             result. Results: {results:?}\nlog:\n{log}"
        );
    };
    // And it must be recorded as a real failure (it fails on base) — not passed,
    // skipped, or a phantom. Observed: status_detailed 501 (FAILED).
    assert!(
        [json!(FAILED_STATUS), json!(FATAL_STATUS)].contains(status),
        "Pre-existing unmanaged result must be FAILED/FATAL on the base rerun, \
         got status_detailed={status} \
         (501=FAILED, 502=FATAL). log:\n{log}"
    );
    Ok(())
}

/// CONTRACT — pre-existing failing unmanaged test that carries the
/// `tpx:skip-filtering-unmanaged-test-suite` label (the cogwheel / audit case).
///
/// This is the regression guard for removing the BRR-path
/// `should_skip_filtering_unmanaged_test_suite` flag (was D107275705). That flag
/// used to suppress phantom TEST_NOT_EXISTS rows for labeled unmanaged suites at
/// emission time. It is gone; suppression now comes solely from the general
/// discovery-time clear keyed on FbTestSuite::is_unmanaged, which is label- and
/// test-type-agnostic. So a labeled cogwheel-style suite that fails on base must
/// still produce NO phantom TEST_NOT_EXISTS, and its `- unmanaged` result must
/// still run and be FAILED (pre-existing).
#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_brr_unmanaged_cogwheel_label_preexisting_failure_no_phantom(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);
    let fail_env = ["--env", "BRR_SIM_FAIL=1"];
    let target = BRR_SIM_UNMANAGED_SKIP_FILTERING_TARGET;

    // Step 1: target run — the labeled unmanaged test FAILS on demand. Capture run_id.
    let stderr = stderr_of(
        buck.test(&[&[target, mode, "--"], &fail_env[..]].concat())
            .await,
    );
    let run_id = run_id(&stderr)
        .with_context(|| format!("Could not extract run_id from test output:\n{stderr}"))?;
    print_testrun_url("Cogwheel-label contract target run", &stderr);

    // Step 2: BRR file naming the suite-level `- unmanaged` entry that failed.
    let existing_name = format!("{target} - unmanaged");
    let brr_file = tmp_path.join("brr_input.json");
    write_brr_file(&brr_file, target, &[&existing_name], &run_id)?;

    // Step 3: base-revision retry — still failing on base (BRR_SIM_FAIL=1).
    let event_log = tmp_path.join("events.ndjson");
    let retry_stderr = stderr_of(
        buck.test(
            &[
                &[target, mode, "--"],
                &fail_env[..],
                &[
                    "--base-rev-retry-with-input-file",
                    path_arg(&brr_file)?,
                    "--experiment",
                    "report-new-test-not-exists-status-for-brr",
                    "--event-log-file",
                    path_arg(&event_log)?,
                ],
            ]
            .concat(),
        )
        .await,
    );
    print_testrun_url("Cogwheel-label contract base-rev retry", &retry_stderr);

    assert!(event_log.exists(), "Event log file was not written");
    let log = std::fs::read_to_string(&event_log)?;
    // The whole point: a LABELED unmanaged suite must NOT yield a phantom 404,
    // with the should_skip_filtering BRR suppression removed.
    assert!(
        !event_log_has_test_not_exists(&event_log)?,
        "Labeled (skip-filtering) unmanaged failure must NOT be flagged \
         TEST_NOT_EXISTS; event log was:\n{log}"
    );
    let results = event_log_results(&event_log)?;
    let Some(status) = results.get(&existing_name) else {
        panic!(
            "Labeled unmanaged test should still run on the base rerun and produce a \
             result. Results: {results:?}\nlog:\n{log}"
        );
    };
    assert!(
        [json!(FAILED_STATUS), json!(FATAL_STATUS)].contains(status),
        "Labeled unmanaged result must be FAILED/FATAL on the base rerun, \
         got status_detailed={status} \
         (501=FAILED, 502=FATAL). log:\n{log}"
    );
    Ok(())
}

/// CONTRACT — new unmanaged test that does not exist on the base revision.
///
/// The BRR file names a `- unmanaged` entry for a suite that is NOT discovered on
/// the retry (a target that doesn't exist on base) — modelling a brand-new
/// unmanaged test. Nothing runs for it: it produces no result row.
///
/// (Whether TPX emits TEST_NOT_EXISTS for a *non-discovered* suite — which would
/// be the legitimate "does not exist on this revision" signal — is asserted from
/// the observed behavior below.)
#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_brr_unmanaged_new_does_not_exist(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);

    // Step 1: a normal run of the real target just to get a valid run_id.
    let first_run = buck.test(&[BRR_SIM_UNMANAGED_TARGET, mode]).await?;
    let run_id = run_id(&first_run.stderr).context("Could not extract run_id from test output")?;
    print_testrun_url("Contract 2 (new) target run", &first_run.stderr);

    // Step 2: BRR file naming a `- unmanaged` entry for a target that does not
    // exist (never discovered on the retry) — i.e. a new unmanaged test.
    let nonexistent_name =
        "fbcode//testinfra/playground/sh:brr_sim_unmanaged_new_does_not_exist - unmanaged";
    let brr_file = tmp_path.join("brr_input.json");
    write_brr_file(
        &brr_file,
        BRR_SIM_UNMANAGED_TARGET,
        &[nonexistent_name],
        &run_id,
    )?;

    // Step 3: base-revision retry with new-test detection enabled.
    let event_log = tmp_path.join("events.ndjson");
    let retry_stderr = stderr_of(
        buck.test(&[
            BRR_SIM_UNMANAGED_TARGET,
            mode,
            "--",
            "--base-rev-retry-with-input-file",
            path_arg(&brr_file)?,
            "--experiment",
            "report-new-test-not-exists-status-for-brr",
            "--event-log-file",
            path_arg(&event_log)?,
        ])
        .await,
    );
    print_testrun_url("Contract 2 (new) base-rev retry", &retry_stderr);

    assert!(event_log.exists(), "Event log file was not written");
    let log = std::fs::read_to_string(&event_log)?;
    // Observed: the non-existent `- unmanaged` entry is silently dropped — nothing
    // runs for it and NO phantom TEST_NOT_EXISTS is emitted (the event log holds
    // only the run_id row).
    assert!(
        !event_log_has_test_not_exists(&event_log)?,
        "A non-existent unmanaged entry must not produce a phantom \
         TEST_NOT_EXISTS; event log was:\n{log}"
    );
    let result_names = event_log_result_names(&event_log)?;
    assert!(
        !result_names.contains(nonexistent_name),
        "A new unmanaged entry that does not exist on base must not run. \
         Results: {result_names:?}\nlog:\n{log}"
    );
    Ok(())
}

/// End-to-end BRR roundtrip: a run_as_bundle test that fails produces a
/// report with '- main', and feeding that report back via
/// --base-rev-retry-with-input-file reproduces the failure (the retry
/// report also contains '- main').
#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_brr_roundtrip_run_as_bundle_failure(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);
    let report_file = tmp_path.join("report.json");
    let retry_report_file = tmp_path.join("retry_report.json");

    // Step 1: Run the broken run_as_bundle target and save the failure report.
    // The test will fail (fatal error), so we expect a failure.
    buck.test(&[
        BROKEN_RUN_AS_BUNDLE_TARGET,
        mode,
        "--",
        "--env",
        "TPX_PLAYGROUND_FATAL=1",
        "--save-failures-for-retry-in-file",
        path_arg(&report_file)?,
    ])
    .await
    .expect_failure();

    // Step 2: Verify the report was written and contains a "- main" entry.
    assert!(report_file.exists(), "Failure report was not written");
    let report = read_brr_report(&report_file)?;
    let test_names = report_test_names(&report)?;
    let has_main = test_names.iter().any(|name| name.ends_with("- main"));
    assert!(
        has_main,
        "Expected a '- main' entry in test_names, got: {test_names:?}"
    );

    // Step 3: Feed the report back as a BRR retry input, saving the retry
    // output to a second report file so we can inspect it directly.
    let _ignored = buck
        .test(&[
            BROKEN_RUN_AS_BUNDLE_TARGET,
            mode,
            "--",
            "--env",
            "TPX_PLAYGROUND_FATAL=1",
            "--base-rev-retry-with-input-file",
            path_arg(&report_file)?,
            "--save-failures-for-retry-in-file",
            path_arg(&retry_report_file)?,
        ])
        .await;

    // Step 4: The retry must reproduce the failure.  Verify by
    // checking the retry report file rather than parsing stderr.
    assert!(
        retry_report_file.exists(),
        "Retry failure report was not written"
    );
    let retry_report = read_brr_report(&retry_report_file)?;
    let retry_test_names = report_test_names(&retry_report)?;
    let retry_has_main = retry_test_names.iter().any(|name| name.ends_with("- main"));
    assert!(
        retry_has_main,
        "Expected a '- main' entry in retry report test_names, got: {retry_test_names:?}"
    );
    eprintln!("{retry_report}");
    Ok(())
}

/// End-to-end BRR roundtrip: a test whose listing fails (os._exit(1) at
/// import time with supports_static_listing=False) produces a report with
/// only '- listing' (not '- main'), and feeding that report back via
/// --base-rev-retry-with-input-file reproduces the listing failure (the
/// retry report also contains '- listing').
#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_brr_roundtrip_listing_failure(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);
    let report_file = tmp_path.join("report.json");
    let retry_report_file = tmp_path.join("retry_report.json");

    // Step 1: Run the broken listing target and save the failure report.
    // With TPX_PLAYGROUND_FATAL=1, the test crashes at import time which
    // kills dynamic listing (supports_static_listing=False).
    buck.test(&[
        BROKEN_LISTING_TARGET,
        mode,
        "--",
        "--env",
        "TPX_PLAYGROUND_FATAL=1",
        "--save-failures-for-retry-in-file",
        path_arg(&report_file)?,
    ])
    .await
    .expect_failure();

    // Step 2: Verify the report was written and contains a "- listing" entry.
    // Listing failures only produce "- listing" in the BRR file (not "- main"),
    // since "- listing" is the authoritative signal for listing health.
    assert!(report_file.exists(), "Failure report was not written");
    let report = read_brr_report(&report_file)?;
    let test_names = report_test_names(&report)?;
    let has_listing = test_names.iter().any(|name| name.ends_with("- listing"));
    assert!(
        has_listing,
        "Expected a '- listing' entry in test_names, got: {test_names:?}"
    );
    let has_main = test_names.iter().any(|name| name.ends_with("- main"));
    assert!(
        !has_main,
        "Expected no '- main' entry in test_names (listing failures only produce '- listing'), got: {test_names:?}"
    );

    // Step 3: Feed the report back as a BRR retry input, saving the retry
    // output to a second report file so we can inspect it directly.
    let _ignored = buck
        .test(&[
            BROKEN_LISTING_TARGET,
            mode,
            "--",
            "--env",
            "TPX_PLAYGROUND_FATAL=1",
            "--base-rev-retry-with-input-file",
            path_arg(&report_file)?,
            "--save-failures-for-retry-in-file",
            path_arg(&retry_report_file)?,
        ])
        .await;

    // Step 4: The retry must reproduce the listing failure.  Verify by
    // checking the retry report file rather than parsing stderr.
    assert!(
        retry_report_file.exists(),
        "Retry failure report was not written"
    );
    let retry_report = read_brr_report(&retry_report_file)?;
    let retry_test_names = report_test_names(&retry_report)?;
    let retry_has_listing = retry_test_names
        .iter()
        .any(|name| name.ends_with("- listing"));
    assert!(
        retry_has_listing,
        "Expected a '- listing' entry in retry report test_names, got: {retry_test_names:?}"
    );
    eprintln!("{retry_report}");
    Ok(())
}

/// Transient listing failure with per-target listing-only BRR: listing fails
/// in the original run (producing '- listing' in the BRR file), but succeeds
/// on the retry. TPX detects the '- listing' entries and suppresses execution
/// for that target via ListOnlyDiscovery::for_targets(), so no tests run even
/// though listing succeeds.
#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_brr_transient_listing_failure_suppresses_execution(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);
    let report_file = tmp_path.join("report.json");

    // Step 1: Run with TPX_PLAYGROUND_FATAL=1 to make listing crash.
    buck.test(&[
        BROKEN_LISTING_TARGET,
        mode,
        "--",
        "--env",
        "TPX_PLAYGROUND_FATAL=1",
        "--save-failures-for-retry-in-file",
        path_arg(&report_file)?,
    ])
    .await
    .expect_failure();

    // Step 2: Verify the report has "- listing".
    assert!(report_file.exists(), "Failure report was not written");
    let report = read_brr_report(&report_file)?;
    let test_names = report_test_names(&report)?;
    let has_listing = test_names.iter().any(|name| name.ends_with("- listing"));
    assert!(
        has_listing,
        "Expected '- listing' in test_names, got: {test_names:?}"
    );

    // Step 3: Feed the report back WITHOUT TPX_PLAYGROUND_FATAL — listing
    // succeeds this time. TPX detects the '- listing' entries and suppresses
    // execution for the target via ListOnlyDiscovery::for_targets().
    let stderr = stderr_of(
        buck.test(&[
            BROKEN_LISTING_TARGET,
            mode,
            "--",
            "--base-rev-retry-with-input-file",
            path_arg(&report_file)?,
        ])
        .await,
    );

    // Step 4: Execution is suppressed for listing-only targets. Listing
    // runs (and succeeds), but no tests execute.
    let stderr = remove_ansi_escape_sequences(&stderr);
    assert!(
        stderr.contains("NO TESTS RAN"),
        "BRR retry with listing-only target should suppress execution, \
         but 'NO TESTS RAN' not found in stderr"
    );
    Ok(())
}

/// End-to-end BRR roundtrip with --list-only: a listing failure produces a
/// report with '- listing', and feeding that report back via
/// --base-rev-retry-with-input-file with --list-only reproduces the listing
/// failure without executing any tests.
#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_brr_listing_only_roundtrip(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);
    let report_file = tmp_path.join("report.json");
    let retry_report_file = tmp_path.join("retry_report.json");

    // Step 1: Run the broken listing target and save the failure report.
    buck.test(&[
        BROKEN_LISTING_TARGET,
        mode,
        "--",
        "--env",
        "TPX_PLAYGROUND_FATAL=1",
        "--save-failures-for-retry-in-file",
        path_arg(&report_file)?,
    ])
    .await
    .expect_failure();

    // Step 2: Verify the report contains a "- listing" entry.
    assert!(report_file.exists(), "Failure report was not written");
    let report = read_brr_report(&report_file)?;
    let test_names = report_test_names(&report)?;
    let has_listing = test_names.iter().any(|name| name.ends_with("- listing"));
    assert!(
        has_listing,
        "Expected a '- listing' entry in test_names, got: {test_names:?}"
    );

    // Step 3: Feed the report back with --list-only (simulating Citadel
    // passing --list-only for a listing-only BRR). Listing still fails
    // because TPX_PLAYGROUND_FATAL is set.
    let _ignored = buck
        .test(&[
            BROKEN_LISTING_TARGET,
            mode,
            "--",
            "--env",
            "TPX_PLAYGROUND_FATAL=1",
            "--list-only",
            "--base-rev-retry-with-input-file",
            path_arg(&report_file)?,
            "--save-failures-for-retry-in-file",
            path_arg(&retry_report_file)?,
        ])
        .await;

    // Step 4: The retry must reproduce the listing failure with --list-only.
    assert!(
        retry_report_file.exists(),
        "Retry failure report was not written"
    );
    let retry_report = read_brr_report(&retry_report_file)?;
    let retry_test_names = report_test_names(&retry_report)?;
    let retry_has_listing = retry_test_names
        .iter()
        .any(|name| name.ends_with("- listing"));
    assert!(
        retry_has_listing,
        "Expected a '- listing' entry in retry report test_names, got: {retry_test_names:?}"
    );
    Ok(())
}

/// Transient listing failure with --list-only: listing fails in the original
/// run, but succeeds on the BRR retry with --list-only. No tests should
/// execute (--list-only suppresses execution even when listing succeeds).
#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_brr_listing_only_transient_failure(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);
    let report_file = tmp_path.join("report.json");

    // Step 1: Run with TPX_PLAYGROUND_FATAL=1 to make listing crash.
    buck.test(&[
        BROKEN_LISTING_TARGET,
        mode,
        "--",
        "--env",
        "TPX_PLAYGROUND_FATAL=1",
        "--save-failures-for-retry-in-file",
        path_arg(&report_file)?,
    ])
    .await
    .expect_failure();

    // Step 2: Verify the report has "- listing".
    assert!(report_file.exists(), "Failure report was not written");
    let report = read_brr_report(&report_file)?;
    let test_names = report_test_names(&report)?;
    let has_listing = test_names.iter().any(|name| name.ends_with("- listing"));
    assert!(
        has_listing,
        "Expected '- listing' in test_names, got: {test_names:?}"
    );

    // Step 3: Feed the report back with --list-only WITHOUT
    // TPX_PLAYGROUND_FATAL — listing succeeds this time, but --list-only
    // should suppress test execution.
    let stderr = stderr_of(
        buck.test(&[
            BROKEN_LISTING_TARGET,
            mode,
            "--",
            "--list-only",
            "--base-rev-retry-with-input-file",
            path_arg(&report_file)?,
        ])
        .await,
    );

    // Step 4: --list-only must suppress execution even when listing succeeds.
    let stderr = remove_ansi_escape_sequences(&stderr);
    assert!(
        stderr.contains("NO TESTS RAN"),
        "BRR retry with --list-only should not execute tests, \
         but 'NO TESTS RAN' not found in stderr"
    );
    Ok(())
}

/// Per-target listing-only BRR: when the BRR input file contains '- listing'
/// entries, TPX automatically suppresses execution for those targets without
/// needing --list-only. This tests the ListOnlyDiscovery::for_targets() path.
#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_brr_per_target_listing_only(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);
    let report_file = tmp_path.join("report.json");

    // Step 1: Run with TPX_PLAYGROUND_FATAL=1 to make listing crash.
    buck.test(&[
        BROKEN_LISTING_TARGET,
        mode,
        "--",
        "--env",
        "TPX_PLAYGROUND_FATAL=1",
        "--save-failures-for-retry-in-file",
        path_arg(&report_file)?,
    ])
    .await
    .expect_failure();

    // Step 2: Verify the report has "- listing".
    assert!(report_file.exists(), "Failure report was not written");
    let report = read_brr_report(&report_file)?;
    let test_names = report_test_names(&report)?;
    let has_listing = test_names.iter().any(|name| name.ends_with("- listing"));
    assert!(
        has_listing,
        "Expected '- listing' in test_names, got: {test_names:?}"
    );

    // Step 3: Feed the report back WITHOUT --list-only and WITHOUT
    // TPX_PLAYGROUND_FATAL — listing succeeds, but TPX should automatically
    // suppress execution because the BRR file has '- listing' entries.
    let stderr = stderr_of(
        buck.test(&[
            BROKEN_LISTING_TARGET,
            mode,
            "--",
            "--base-rev-retry-with-input-file",
            path_arg(&report_file)?,
        ])
        .await,
    );

    // Step 4: TPX must suppress execution for listing-only targets even
    // without --list-only, via ListOnlyDiscovery::for_targets().
    let stderr = remove_ansi_escape_sequences(&stderr);
    assert!(
        stderr.contains("NO TESTS RAN"),
        "BRR retry should suppress execution for listing-only targets, \
         but 'NO TESTS RAN' not found in stderr"
    );
    Ok(())
}

/// --save-successes-for-retry-in-file writes all passing tests to an NDJSON
/// file. Each line contains a single test_name entry. The file format is
/// identical to --save-failures-for-retry-in-file so it can be consumed by
/// --base-rev-retry-with-input-file.
#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_save_successes_for_retry(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let mode = get_mode_from_platform(Mode::Dev);
    let successes_file = tmp_path.join("successes.ndjson");

    // Run a passing test target with --save-successes-for-retry-in-file.
    buck.test(&[
        PYTHON_TEST_TARGET,
        mode,
        "--",
        "--save-successes-for-retry-in-file",
        path_arg(&successes_file)?,
    ])
    .await?;

    // Verify the file was created and is non-empty.
    assert!(successes_file.exists(), "Successes file was not created");
    let content = std::fs::read_to_string(&successes_file)?;
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert!(
        !lines.is_empty(),
        "Successes file is empty — expected passing tests"
    );

    // Verify each line is valid JSON with exactly one test_name.
    for line in lines {
        let entry: Value = serde_json::from_str(line)?;
        let test_names = match entry.get("test_names") {
            Some(test_names) => test_names
                .as_array()
                .with_context(|| format!("`test_names` is not a list: {entry}"))?
                .clone(),
            None => Vec::new(),
        };
        assert_eq!(
            test_names.len(),
            1,
            "Expected exactly 1 test_name per line, got {}: {test_names:?}",
            test_names.len()
        );
        assert!(is_truthy(entry.get("run_id")), "Missing run_id");
        assert!(is_truthy(entry.get("suite_name")), "Missing suite_name");
    }
    Ok(())
}

/// Whether Python would take the value as true: present, and not null, false, zero or empty.
fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}
