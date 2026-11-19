/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::prelude::*;
use serde_json::Value;

const TARGETS: [&str; 6] = [
    "target1", "target2", "target3", "target4", "target5", "target6",
];

/// The sorted names of the targets and the errors in a `--json` output.
fn targets_and_errors(stdout: &str) -> anyhow::Result<(Vec<String>, Vec<Value>)> {
    let xs: Vec<Value> = serde_json::from_str(stdout)?;
    let errors: Vec<Value> = xs
        .iter()
        .filter(|x| x.get("buck.error").is_some())
        .cloned()
        .collect();
    let mut targets: Vec<String> = xs
        .iter()
        .filter_map(|x| x.get("name"))
        .map(|name| name.as_str().unwrap().to_owned())
        .collect();
    targets.sort();
    Ok((targets, errors))
}

#[buck_test]
async fn test_keep_going_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.targets(&["//...", "--json", "--keep-going"]).await?;
    let (targets, errors) = targets_and_errors(&result.stdout)?;

    assert_eq!(targets, TARGETS);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["buck.package"], "root//b");
    assert!(
        errors[0]["buck.error"]
            .as_str()
            .unwrap()
            .contains("test_error")
    );
    Ok(())
}

#[buck_test]
async fn test_keep_going(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.targets(&["//...", "--keep-going"]).await?;
    assert!(result.stderr.contains("test_error"));
    Ok(())
}

async fn test_keep_going_streaming(buck: &Buck, no_cache: bool) -> anyhow::Result<()> {
    let mut args = vec!["//...", "--json", "--streaming", "--keep-going"];
    if no_cache {
        args.push("--no-cache");
    }
    let result = buck.targets(&args).await?;
    let (targets, errors) = targets_and_errors(&result.stdout)?;

    assert_eq!(targets, TARGETS);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["buck.package"], "root//b");
    assert!(
        errors[0]["buck.error"]
            .as_str()
            .unwrap()
            .contains("test_error")
    );
    Ok(())
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_keep_going_streaming_False(buck: &Buck) -> anyhow::Result<()> {
    test_keep_going_streaming(buck, false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_keep_going_streaming_True(buck: &Buck) -> anyhow::Result<()> {
    test_keep_going_streaming(buck, true).await
}

#[buck_test]
async fn test_keep_going_streaming_chunked_stdout(buck: &Buck) -> anyhow::Result<()> {
    // Each package's output is written to the client in one go; the daemon has
    // to split it into transport-sized messages. Force tiny chunks and check
    // that the output is byte-identical to the unchunked output.
    let args = [
        "//...",
        "--json-lines",
        "--streaming",
        "--keep-going",
        "--no-cache",
    ];
    let result1 = buck.targets(&args).await?;
    buck.kill().await?;
    let result2 = buck
        .targets(&args)
        .env("BUCK2_DEBUG_RAWOUTPUT_CHUNK_SIZE", "7")
        .await?;
    // Streaming output order is not deterministic across runs.
    let mut lines1: Vec<&str> = result1.stdout.lines().collect();
    let mut lines2: Vec<&str> = result2.stdout.lines().collect();
    lines1.sort();
    lines2.sort();
    assert_eq!(lines1, lines2);
    assert_eq!(result1.stdout.lines().count(), 7); // 6 targets + 1 error
    Ok(())
}

#[buck_test]
async fn test_streaming_keep_going_missing_targets(buck: &Buck) -> anyhow::Result<()> {
    let targets = [
        "//a:target1",
        "//a:target2",
        "//a:bogus_target",
        "//a:worse_target",
        "//a:target5",
        "//d:bogus_package",
    ];
    let mut args = targets.to_vec();
    args.extend(["--json", "--streaming", "--keep-going"]);
    let result = buck.targets(&args).await?;
    let xs: Vec<Value> = serde_json::from_str(&result.stdout)?;
    assert_eq!(xs.len(), 5); // 3 success, 2 errors
    let mut bad_packages = Vec::new();
    let mut good_targets = Vec::new();
    for x in &xs {
        if let Some(error) = x.get("buck.error") {
            bad_packages.push(x["buck.package"].as_str().unwrap());
            if x["buck.package"] == "root//a" {
                let error = error.as_str().unwrap();
                assert!(error.contains("`bogus_target`"));
                assert!(error.contains("`worse_target`"));
            }
        } else {
            good_targets.push(x["name"].as_str().unwrap());
        }
    }
    bad_packages.sort();
    good_targets.sort();
    assert_eq!(bad_packages, ["root//a", "root//d"]);
    assert_eq!(good_targets, ["target1", "target2", "target5"]);
    Ok(())
}

#[buck_test]
async fn test_streaming_keep_going_with_single_failure(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .targets(&[
            "//a:does_not_exist",
            "--json",
            "--streaming",
            "--keep-going",
        ])
        .await?;
    let xs: Vec<Value> = serde_json::from_str(&result.stdout)?;
    assert_eq!(xs.len(), 1);
    assert_eq!(xs[0]["buck.package"], "root//a");
    assert_eq!(
        xs[0]["buck.error"],
        "Unknown targets `does_not_exist` from package `root//a`."
    );
    Ok(())
}

#[buck_test]
async fn test_streaming_keep_going_with_single_failing_target_and_one_other_target_in_different_package(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result = buck
        .targets(&[
            "//a:target1",
            "//c:does_not_exist",
            "-a",
            "type",
            "--streaming",
            "--keep-going",
        ])
        .await?;

    let xs: Vec<Value> = serde_json::from_str(&result.stdout)?;
    assert_eq!(xs.len(), 2);

    let (good_target, bad_target) = if xs[0].get("buck.error").is_some() {
        (&xs[1], &xs[0])
    } else {
        (&xs[0], &xs[1])
    };

    assert_eq!(good_target["buck.type"], "prelude//prelude.bzl:a_target");

    assert_eq!(bad_target["buck.package"], "root//c");
    assert_eq!(
        bad_target["buck.error"],
        "Unknown targets `does_not_exist` from package `root//c`."
    );
    Ok(())
}
