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
use buck2_e2e::utils::replace_hash;
use regex::Regex;
use serde_json::Value;
use serde_json::json;

async fn test_ctargets_transition(
    buck: &Buck,
    output_format: &str,
    hash_mode: &str,
) -> anyhow::Result<()> {
    let format_arg = format!("--{output_format}");
    let mut args = vec!["root//:candy", "--target-platforms=root//:p"];
    if output_format != "text" {
        args.push(&format_arg);
    }
    if hash_mode != "none" {
        args.push("--show-target-hash");
    }
    if hash_mode == "recursive" {
        args.push("--target-hash-recursive");
    }
    let result = buck.ctargets(&args).await?;
    let output = replace_hash(&result.stdout);
    let target_hash_regex = Regex::new(r"^[0-9a-f]{32}$").unwrap();
    if output_format == "text" {
        let lines: Vec<&str> = output.lines().collect();
        let [line] = lines.as_slice() else {
            panic!("expected one line: {lines:?}");
        };
        let mut line: &str = line;
        if hash_mode != "none" {
            let target_hash;
            (line, target_hash) = line.rsplit_once(' ').unwrap();
            assert!(target_hash_regex.is_match(target_hash));
        }
        assert_eq!(line, "root//:candy (<clay>#<HASH>)");
        return Ok(());
    }

    let mut targets: Value = serde_json::from_str(&output)?;
    if output_format == "json-report" {
        assert_eq!(targets["incompatible_targets"], json!([]));
        targets = targets["compatible_targets"].take();
    }
    let targets = targets.as_array().unwrap();
    let [target] = targets.as_slice() else {
        panic!("expected one target: {targets:?}");
    };
    assert_eq!(target["buck.target"], "root//:candy");
    assert_eq!(target["buck.type"], "root//defs.bzl:clay_library");
    assert_eq!(target["value"], "before");
    if hash_mode == "none" {
        assert!(target.get("buck.target_hash").is_none());
    } else {
        assert!(target_hash_regex.is_match(target["buck.target_hash"].as_str().unwrap()));
    }
    Ok(())
}

#[buck_test]
async fn test_ctargets_transition_none_text(buck: &Buck) -> anyhow::Result<()> {
    test_ctargets_transition(buck, "text", "none").await
}

#[buck_test]
async fn test_ctargets_transition_none_json(buck: &Buck) -> anyhow::Result<()> {
    test_ctargets_transition(buck, "json", "none").await
}

#[buck_test]
async fn test_ctargets_transition_none_json_report(buck: &Buck) -> anyhow::Result<()> {
    test_ctargets_transition(buck, "json-report", "none").await
}

#[buck_test]
async fn test_ctargets_transition_non_recursive_text(buck: &Buck) -> anyhow::Result<()> {
    test_ctargets_transition(buck, "text", "non_recursive").await
}

#[buck_test]
async fn test_ctargets_transition_non_recursive_json(buck: &Buck) -> anyhow::Result<()> {
    test_ctargets_transition(buck, "json", "non_recursive").await
}

#[buck_test]
async fn test_ctargets_transition_non_recursive_json_report(buck: &Buck) -> anyhow::Result<()> {
    test_ctargets_transition(buck, "json-report", "non_recursive").await
}

#[buck_test]
async fn test_ctargets_transition_recursive_text(buck: &Buck) -> anyhow::Result<()> {
    test_ctargets_transition(buck, "text", "recursive").await
}

#[buck_test]
async fn test_ctargets_transition_recursive_json(buck: &Buck) -> anyhow::Result<()> {
    test_ctargets_transition(buck, "json", "recursive").await
}

#[buck_test]
async fn test_ctargets_transition_recursive_json_report(buck: &Buck) -> anyhow::Result<()> {
    test_ctargets_transition(buck, "json-report", "recursive").await
}

async fn test_ctargets_transition_hash_tracks_actual_attributes(
    buck: &Buck,
    hash_function: &str,
    recursive: bool,
) -> anyhow::Result<()> {
    let hash_function_arg = format!("--target-hash-function={hash_function}");
    let mut args = vec![
        "root//:candy",
        "--target-platforms=root//:p",
        "--json",
        "--show-target-hash",
        &hash_function_arg,
    ];
    if recursive {
        args.push("--target-hash-recursive");
    }
    let before: Vec<Value> = serde_json::from_str(&buck.ctargets(&args).await?.stdout)?;
    let [before] = before.as_slice() else {
        panic!("expected one target: {before:?}");
    };
    let after: Vec<Value> = serde_json::from_str(
        &buck
            .ctargets(&[args.as_slice(), &["-c", "test.value=after"]].concat())
            .await?
            .stdout,
    )?;
    let [after] = after.as_slice() else {
        panic!("expected one target: {after:?}");
    };
    assert_eq!(before["value"], "before");
    assert_eq!(after["value"], "after");
    let target_hash = |target: &Value| {
        target
            .get("buck.target_hash")
            .filter(|hash| hash.is_string())
            .cloned()
            .unwrap_or_else(|| panic!("No `buck.target_hash` in {target}"))
    };
    assert_ne!(target_hash(before), target_hash(after));
    Ok(())
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_ctargets_transition_hash_tracks_actual_attributes_False_fast(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_ctargets_transition_hash_tracks_actual_attributes(buck, "fast", false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_ctargets_transition_hash_tracks_actual_attributes_False_strong(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_ctargets_transition_hash_tracks_actual_attributes(buck, "strong", false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_ctargets_transition_hash_tracks_actual_attributes_True_fast(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_ctargets_transition_hash_tracks_actual_attributes(buck, "fast", true).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_ctargets_transition_hash_tracks_actual_attributes_True_strong(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_ctargets_transition_hash_tracks_actual_attributes(buck, "strong", true).await
}
