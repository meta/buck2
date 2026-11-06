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

use buck2_e2e::prelude::*;
use buck2_e2e::utils::random_string;
use buck2_e2e::utils::read_what_ran;
use serde_json::Value;

/// The `test.run` entries of what the last command ran.
async fn test_runs(buck: &Buck, args: &[&str]) -> anyhow::Result<Vec<Value>> {
    Ok(read_what_ran(buck, args)
        .await?
        .into_iter()
        .filter(|entry| entry["reason"] == "test.run")
        .collect())
}

/// The files named `name` under `dir`, at any depth.
fn find_files(dir: &Path, name: &str, found: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            find_files(&entry.path(), name, found)?;
        } else if entry.file_name() == name {
            found.push(entry.path());
        }
    }
    Ok(())
}

// Running a test more than once reuses its key in the critical path graph, which is the
// `critical_path_duplicate_key` soft error.
#[buck_test(allow_soft_errors = true)]
async fn test_stable_action_digest_with_deterministic_paths(buck: &Buck) -> anyhow::Result<()> {
    let args = [
        "-c",
        "test.local_enabled=false",
        "-c",
        "test.remote_enabled=true",
        "//:test",
    ];

    buck.test(&args).await?;
    let first_digests: Vec<Value> = test_runs(buck, &[])
        .await?
        .iter()
        .map(|entry| {
            entry
                .pointer("/reproducer/details/digest")
                .unwrap_or_else(|| panic!("No digest in {entry}"))
                .clone()
        })
        .collect();
    assert_eq!(first_digests.len(), 1, "Expected one test.run entry");

    buck.test(&args).await?;
    let second_digests: Vec<Value> = test_runs(buck, &[])
        .await?
        .iter()
        .map(|entry| {
            entry
                .pointer("/reproducer/details/digest")
                .unwrap_or_else(|| panic!("No digest in {entry}"))
                .clone()
        })
        .collect();
    assert_eq!(second_digests.len(), 1, "Expected one test.run entry");

    assert_eq!(
        first_digests[0], second_digests[0],
        "Test action digests differ between runs: {} vs {}",
        first_digests[0], second_digests[0]
    );
    Ok(())
}

// Running a test more than once reuses its key in the critical path graph, which is the
// `critical_path_duplicate_key` soft error.
#[buck_test(allow_soft_errors = true)]
async fn test_stress_runs_have_different_action_digests(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&[
        "-c",
        "test.local_enabled=false",
        "-c",
        "test.remote_enabled=true",
        "//:test",
        "--",
        "--stress-runs",
        "2",
    ])
    .await?;
    let test_runs = test_runs(buck, &[]).await?;
    assert_eq!(
        test_runs.len(),
        2,
        "Expected exactly 2 test.run entries for stress runs, got {}",
        test_runs.len()
    );

    let digests: Vec<&Value> = test_runs
        .iter()
        .map(|entry| {
            entry
                .pointer("/reproducer/details/digest")
                .unwrap_or_else(|| panic!("No digest in {entry}"))
        })
        .collect();
    assert_ne!(
        digests[0], digests[1],
        "Stress run action digests should differ but were both: {}",
        digests[0]
    );
    Ok(())
}

#[buck_test]
async fn test_remote_test_execution_cached(buck: &Buck) -> anyhow::Result<()> {
    let args = [
        "-c",
        "test.local_enabled=false",
        "-c",
        "test.remote_enabled=true",
        "//:cacheable_test",
    ];

    buck.test(&args).await?;

    buck.test(&args).await?;
    let second_test_runs: Vec<Value> = test_runs(buck, &["--emit-cache-queries"])
        .await?
        .into_iter()
        .filter(|entry| entry["reproducer"]["executor"] == "Cache")
        .collect();
    assert_eq!(
        second_test_runs.len(),
        1,
        "Expected exactly one cached test.run entry, got {}",
        second_test_runs.len()
    );
    Ok(())
}

// Running a test more than once reuses its key in the critical path graph, which is the
// `critical_path_duplicate_key` soft error.
#[buck_test(allow_soft_errors = true)]
async fn test_remote_test_execution_not_cached_for_stress_runs(buck: &Buck) -> anyhow::Result<()> {
    let args = [
        "-c",
        "test.local_enabled=false",
        "-c",
        "test.remote_enabled=true",
        "//:cacheable_test",
        "--",
        "--stress-runs",
        "2",
    ];

    buck.test(&args).await?;

    buck.test(&args).await?;
    let test_runs = test_runs(buck, &[]).await?;
    assert_eq!(
        test_runs.len(),
        2,
        "Expected exactly 2 test.run entries for stress runs, got {}",
        test_runs.len()
    );

    // Stress runs disable caching — even on the second invocation, both runs
    // should execute remotely rather than hitting the cache.
    for entry in &test_runs {
        let executor = entry["reproducer"]["executor"].as_str().unwrap_or_default();
        assert_eq!(
            executor, "Re",
            "Expected Re executor for stress runs, got: {executor}"
        );
    }
    Ok(())
}

#[buck_test]
async fn test_local_test_execution_not_cached(buck: &Buck) -> anyhow::Result<()> {
    let seed = format!("test.seed={}", random_string());
    let args = [
        "-c",
        "test.local_enabled=true",
        "-c",
        "test.remote_enabled=false",
        "-c",
        &seed,
        "//:cacheable_test",
    ];

    buck.test(&args).await?;

    buck.test(&args).await?;
    let second_test_runs = test_runs(buck, &[]).await?;
    assert_eq!(
        second_test_runs.len(),
        1,
        "Expected exactly one test.run entry, got {}",
        second_test_runs.len()
    );
    assert_eq!(
        second_test_runs[0]["reproducer"]["executor"], "Local",
        "Expected test to run locally, not be cached!"
    );
    Ok(())
}

// Running a test more than once reuses its key in the critical path graph, which is the
// `critical_path_duplicate_key` soft error.
#[buck_test(allow_soft_errors = true)]
async fn test_remote_test_execution_not_cached_with_no_remote_cache(
    buck: &Buck,
) -> anyhow::Result<()> {
    let args = [
        "-c",
        "test.local_enabled=false",
        "-c",
        "test.remote_enabled=true",
        "--no-remote-cache",
        "//:cacheable_test",
    ];

    buck.test(&args).await?;

    buck.test(&args).await?;
    let second_test_runs = test_runs(buck, &[]).await?;
    assert_eq!(
        second_test_runs.len(),
        1,
        "Expected exactly one test.run entry, got {}",
        second_test_runs.len()
    );
    assert_eq!(
        second_test_runs[0]["reproducer"]["executor"], "Re",
        "Expected test to run remotely, not be cached!"
    );
    Ok(())
}

#[buck_test]
async fn test_remote_test_execution_not_cached_with_disable_flag(
    buck: &Buck,
) -> anyhow::Result<()> {
    let args = [
        "-c",
        "test.local_enabled=false",
        "-c",
        "test.remote_enabled=true",
        "//:cacheable_test",
        "--",
        "--disable-test-execution-caching",
    ];

    buck.test(&args).await?;

    buck.test(&args).await?;
    let second_test_runs = test_runs(buck, &[]).await?;
    assert_eq!(
        second_test_runs.len(),
        1,
        "Expected exactly one test.run entry, got {}",
        second_test_runs.len()
    );
    assert_eq!(
        second_test_runs[0]["reproducer"]["executor"], "Re",
        "Expected test to run remotely, not be cached!"
    );
    Ok(())
}

#[buck_test]
async fn test_outputs_of_a_local_run_are_on_disk(buck: &Buck) -> anyhow::Result<()> {
    // The test runner reads a test's outputs out of process, so after a local run Buck asks
    // the materializer for them by value; the only copy of what the test wrote is the one on
    // disk.
    buck.test(&[
        "-c",
        "test.local_enabled=true",
        "-c",
        "test.remote_enabled=false",
        "//:writes_output",
    ])
    .await?;
    let mut paths = Vec::new();
    find_files(
        &buck.cwd().join("buck-out").join("v2").join("test"),
        "written_by_test",
        &mut paths,
    )?;
    let mut written = paths
        .iter()
        .map(std::fs::read_to_string)
        .collect::<std::io::Result<Vec<_>>>()?;
    written.sort();
    assert_eq!(written, ["list", "run"]);
    Ok(())
}
