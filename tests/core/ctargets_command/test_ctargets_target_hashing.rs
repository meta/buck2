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
use std::sync::LazyLock;

use buck2_e2e::golden::golden_replace_cfg_hash;
use buck2_e2e::prelude::*;
use regex::Regex;
use serde_json::Value;

static HASH_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[0-9a-f]{32}|[0-9a-f]{64})$").unwrap());

/// The `buck.target_hash` of a target in a `--json` output.
fn hash_of(target: &Value) -> &Value {
    target
        .get("buck.target_hash")
        .unwrap_or_else(|| panic!("no `buck.target_hash` in {target}"))
}

async fn target_hash(buck: &Buck, target: &str, args: &[&str]) -> anyhow::Result<String> {
    let result = buck
        .ctargets(&[&[target, "--show-target-hash", "--json"], args].concat())
        .await?;
    let output: Vec<Value> = serde_json::from_str(&result.stdout)?;
    assert_eq!(output.len(), 1, "{output:?}");
    let target_hash = output[0]["buck.target_hash"].as_str().unwrap().to_owned();
    assert!(HASH_REGEX.is_match(&target_hash), "{target_hash}");
    Ok(target_hash)
}

async fn test_hash_output_golden(
    buck: &Buck,
    output_format: &str,
    hash_function: &str,
    recursive: bool,
) -> anyhow::Result<()> {
    let mut args = if output_format == "json" {
        vec!["--json"]
    } else {
        vec![]
    };
    if recursive {
        args.push("--target-hash-recursive");
    }
    let hash_function_arg = format!("--target-hash-function={hash_function}");
    let result = buck
        .ctargets(
            &[
                &[
                    "root//:parent?root//:linux",
                    "--show-target-hash",
                    &hash_function_arg,
                ],
                args.as_slice(),
            ]
            .concat(),
        )
        .await?;
    golden_replace_cfg_hash(
        &Regex::new(r"\b(?:[0-9a-f]{32}|[0-9a-f]{64})\b")
            .unwrap()
            .replace_all(&result.stdout, "<TARGET_HASH>"),
        &format!("golden/hash.{output_format}.golden"),
    );
    Ok(())
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_output_golden_False_fast_json(buck: &Buck) -> anyhow::Result<()> {
    test_hash_output_golden(buck, "json", "fast", false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_output_golden_False_fast_text(buck: &Buck) -> anyhow::Result<()> {
    test_hash_output_golden(buck, "text", "fast", false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_output_golden_False_strong_json(buck: &Buck) -> anyhow::Result<()> {
    test_hash_output_golden(buck, "json", "strong", false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_output_golden_False_strong_text(buck: &Buck) -> anyhow::Result<()> {
    test_hash_output_golden(buck, "text", "strong", false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_output_golden_True_fast_json(buck: &Buck) -> anyhow::Result<()> {
    test_hash_output_golden(buck, "json", "fast", true).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_output_golden_True_fast_text(buck: &Buck) -> anyhow::Result<()> {
    test_hash_output_golden(buck, "text", "fast", true).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_output_golden_True_strong_json(buck: &Buck) -> anyhow::Result<()> {
    test_hash_output_golden(buck, "json", "strong", true).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_output_golden_True_strong_text(buck: &Buck) -> anyhow::Result<()> {
    test_hash_output_golden(buck, "text", "strong", true).await
}

#[buck_test]
async fn test_hash_ignores_configuration_but_tracks_configured_attrs(
    buck: &Buck,
) -> anyhow::Result<()> {
    let same_result = buck
        .ctargets(&[
            "root//:same?root//:linux",
            "root//:same?root//:macos",
            "--show-target-hash",
            "--json",
        ])
        .await?;
    let same_outputs: Vec<Value> = serde_json::from_str(&same_result.stdout)?;
    assert_eq!(same_outputs.len(), 2, "{same_outputs:?}");

    let selected_result = buck
        .ctargets(&[
            "root//:selected?root//:linux",
            "root//:selected?root//:macos",
            "--show-target-hash",
            "--json",
        ])
        .await?;
    let selected_outputs: Vec<Value> = serde_json::from_str(&selected_result.stdout)?;
    assert_eq!(selected_outputs.len(), 2, "{selected_outputs:?}");

    assert_eq!(hash_of(&same_outputs[0]), hash_of(&same_outputs[1]));
    assert_ne!(hash_of(&selected_outputs[0]), hash_of(&selected_outputs[1]));
    Ok(())
}

async fn test_hash_deduplicates_targets_after_transition(
    buck: &Buck,
    output_format: &str,
) -> anyhow::Result<()> {
    let format_arg = format!("--{output_format}");
    let mut args = vec![
        "root//:transitioned?root//:linux",
        "root//:transitioned?root//:macos",
        "--show-target-hash",
    ];
    if output_format != "text" {
        args.push(&format_arg);
    }
    let result = buck.ctargets(&args).await?;
    if output_format == "text" {
        let lines: Vec<&str> = result.stdout.lines().collect();
        let [line] = lines.as_slice() else {
            panic!("expected one line: {lines:?}");
        };
        assert!(line.starts_with("root//:transitioned ("));
        assert!(HASH_REGEX.is_match(line.rsplit_once(' ').unwrap().1));
        return Ok(());
    }

    let mut targets: Value = serde_json::from_str(&result.stdout)?;
    if output_format == "json-report" {
        targets = targets["compatible_targets"].take();
    }
    let targets = targets.as_array().unwrap();
    let [target] = targets.as_slice() else {
        panic!("expected one target: {targets:?}");
    };
    assert_eq!(target["buck.target"], "root//:transitioned");
    assert_eq!(target["buck.type"], "root//defs.bzl:transitioned");
    assert!(HASH_REGEX.is_match(target["buck.target_hash"].as_str().unwrap()));
    Ok(())
}

#[buck_test]
async fn test_hash_deduplicates_targets_after_transition_text(buck: &Buck) -> anyhow::Result<()> {
    test_hash_deduplicates_targets_after_transition(buck, "text").await
}

#[buck_test]
async fn test_hash_deduplicates_targets_after_transition_json(buck: &Buck) -> anyhow::Result<()> {
    test_hash_deduplicates_targets_after_transition(buck, "json").await
}

#[buck_test]
async fn test_hash_deduplicates_targets_after_transition_json_report(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_hash_deduplicates_targets_after_transition(buck, "json-report").await
}

async fn test_hash_ignores_forward_dependency_nodes(
    buck: &Buck,
    hash_function: &str,
    recursive: bool,
) -> anyhow::Result<()> {
    let target = "root//:transition_parent";
    let with_forward_dep = format!("{target}?root//:linux");
    let hash_function_arg = format!("--target-hash-function={hash_function}");
    let mut args = vec![hash_function_arg.as_str()];
    if recursive {
        args.push("--target-hash-recursive");
    }

    let baseline = target_hash(buck, target, &args).await?;
    assert_eq!(baseline, target_hash(buck, &with_forward_dep, &args).await?);

    let changed_args = [args.as_slice(), &["-c", "test.transitioned_value=after"]].concat();
    let changed = target_hash(buck, target, &changed_args).await?;
    assert_eq!(baseline != changed, recursive);
    assert_eq!(
        changed,
        target_hash(buck, &with_forward_dep, &changed_args).await?
    );

    let forced_args = [
        args.as_slice(),
        &["--require-hash-change-deps", "root//:transitioned"],
    ]
    .concat();
    let forced = target_hash(buck, target, &forced_args).await?;
    assert_ne!(baseline, forced);
    assert_eq!(
        forced,
        target_hash(buck, &with_forward_dep, &forced_args).await?
    );
    Ok(())
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_ignores_forward_dependency_nodes_False_fast(buck: &Buck) -> anyhow::Result<()> {
    test_hash_ignores_forward_dependency_nodes(buck, "fast", false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_ignores_forward_dependency_nodes_False_strong(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_hash_ignores_forward_dependency_nodes(buck, "strong", false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_ignores_forward_dependency_nodes_True_fast(buck: &Buck) -> anyhow::Result<()> {
    test_hash_ignores_forward_dependency_nodes(buck, "fast", true).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_ignores_forward_dependency_nodes_True_strong(buck: &Buck) -> anyhow::Result<()> {
    test_hash_ignores_forward_dependency_nodes(buck, "strong", true).await
}

#[buck_test]
async fn test_hash_is_available_in_all_output_formats(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:same?root//:linux";

    let plain_json: Value =
        serde_json::from_str(&buck.ctargets(&[target, "--json"]).await?.stdout)?;
    assert!(
        plain_json.as_array().unwrap()[0]
            .get("buck.target_hash")
            .is_none()
    );

    let text = buck.ctargets(&[target, "--show-target-hash"]).await?.stdout;
    assert!(
        Regex::new(r" [0-9a-f]{32}\n$").unwrap().is_match(&text),
        "{text}"
    );

    let report: Value = serde_json::from_str(
        &buck
            .ctargets(&[target, "--show-target-hash", "--json-report"])
            .await?
            .stdout,
    )?;
    assert!(
        HASH_REGEX.is_match(
            report["compatible_targets"][0]["buck.target_hash"]
                .as_str()
                .unwrap()
        )
    );
    Ok(())
}

async fn test_hash_tracks_dependency_attributes_only(
    buck: &Buck,
    recursive: bool,
) -> anyhow::Result<()> {
    let args: &[&str] = if recursive {
        &["--target-hash-recursive"]
    } else {
        &[]
    };
    let baseline = target_hash(buck, "root//:parent?root//:linux", args).await?;

    let targets_file = buck.cwd().join("TARGETS.fixture");
    std::fs::write(
        &targets_file,
        std::fs::read_to_string(&targets_file)?.replace("unrelated-before", "unrelated-after"),
    )?;
    buck.kill().await?;
    let unrelated_changed = target_hash(buck, "root//:parent?root//:linux", args).await?;
    assert_eq!(baseline, unrelated_changed);

    std::fs::write(
        &targets_file,
        std::fs::read_to_string(&targets_file)?.replace("dep-before", "dep-after"),
    )?;
    buck.kill().await?;
    let dependency_changed = target_hash(buck, "root//:parent?root//:linux", args).await?;
    assert_eq!(baseline != dependency_changed, recursive);
    Ok(())
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_tracks_dependency_attributes_only_False(buck: &Buck) -> anyhow::Result<()> {
    test_hash_tracks_dependency_attributes_only(buck, false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_tracks_dependency_attributes_only_True(buck: &Buck) -> anyhow::Result<()> {
    test_hash_tracks_dependency_attributes_only(buck, true).await
}

#[buck_test]
async fn test_hash_does_not_read_source_contents(buck: &Buck) -> anyhow::Result<()> {
    let baseline = target_hash(buck, "root//:with_src?root//:linux", &[]).await?;

    std::fs::write(buck.cwd().join("source.txt"), "changed contents\n")?;
    buck.kill().await?;

    assert_eq!(
        baseline,
        target_hash(buck, "root//:with_src?root//:linux", &[]).await?
    );
    assert_ne!(
        baseline,
        target_hash(
            buck,
            "root//:with_src?root//:linux",
            &["--require-hash-change-deps", "root//:with_src"],
        )
        .await?
    );
    Ok(())
}

/// The target hashes of a `--json` output, by target.
fn hashes_by_target(nodes: &[Value]) -> BTreeMap<String, Value> {
    nodes
        .iter()
        .map(|node| {
            (
                node["buck.target"].as_str().unwrap().to_owned(),
                hash_of(node).clone(),
            )
        })
        .collect()
}

async fn test_require_hash_change_deps(
    buck: &Buck,
    hash_function: &str,
    recursive: bool,
) -> anyhow::Result<()> {
    let targets = [
        "dep",
        "parent",
        "grandparent",
        "great_grandparent",
        "with_src",
        "unrelated",
    ];
    let hash_function_arg = format!("--target-hash-function={hash_function}");
    let patterns: Vec<String> = targets
        .iter()
        .map(|target| format!("root//:{target}?root//:linux"))
        .collect();
    let mut args = vec![hash_function_arg.as_str(), "--show-target-hash", "--json"];
    if recursive {
        args.push("--target-hash-recursive");
    }
    args.extend(patterns.iter().map(String::as_str));
    let baseline: Vec<Value> = serde_json::from_str(&buck.ctargets(&args).await?.stdout)?;
    let changed: Vec<Value> = serde_json::from_str(
        &buck
            .ctargets(
                &[
                    args.as_slice(),
                    &["--require-hash-change-deps", "root//:dep", ":with_src"],
                ]
                .concat(),
            )
            .await?
            .stdout,
    )?;
    let mut expected = BTreeSet::from(["root//:dep", "root//:parent", "root//:with_src"]);
    if recursive {
        expected.extend(["root//:grandparent", "root//:great_grandparent"]);
    }
    let baseline_hashes = hashes_by_target(&baseline);
    let changed_hashes = hashes_by_target(&changed);
    let all_targets: BTreeSet<String> = targets
        .iter()
        .map(|target| format!("root//:{target}"))
        .collect();
    assert!(baseline_hashes.keys().eq(changed_hashes.keys()));
    assert!(baseline_hashes.keys().eq(all_targets.iter()));
    assert_eq!(
        baseline_hashes
            .iter()
            .filter(|(target, hash)| changed_hashes[*target] != **hash)
            .map(|(target, _)| target.as_str())
            .collect::<BTreeSet<_>>(),
        expected
    );
    Ok(())
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_require_hash_change_deps_False_fast(buck: &Buck) -> anyhow::Result<()> {
    test_require_hash_change_deps(buck, "fast", false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_require_hash_change_deps_False_strong(buck: &Buck) -> anyhow::Result<()> {
    test_require_hash_change_deps(buck, "strong", false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_require_hash_change_deps_True_fast(buck: &Buck) -> anyhow::Result<()> {
    test_require_hash_change_deps(buck, "fast", true).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_require_hash_change_deps_True_strong(buck: &Buck) -> anyhow::Result<()> {
    test_require_hash_change_deps(buck, "strong", true).await
}

async fn test_require_hash_change_deps_ignores_order_and_unrelated_targets(
    buck: &Buck,
    hash_function: &str,
    recursive: bool,
) -> anyhow::Result<()> {
    let target = "root//:parent?root//:linux";
    let hash_function_arg = format!("--target-hash-function={hash_function}");
    let mut args = vec![hash_function_arg.as_str()];
    if recursive {
        args.push("--target-hash-recursive");
    }
    let baseline = target_hash(buck, target, &args).await?;
    args.push("--require-hash-change-deps");
    assert_eq!(
        baseline,
        target_hash(
            buck,
            target,
            &[args.as_slice(), &["root//:unrelated"]].concat()
        )
        .await?
    );
    let changed = target_hash(
        buck,
        target,
        &[args.as_slice(), &["root//:dep", "root//:unrelated"]].concat(),
    )
    .await?;
    assert_ne!(baseline, changed);
    assert_eq!(
        changed,
        target_hash(
            buck,
            target,
            &[
                args.as_slice(),
                &[
                    "root//:unrelated",
                    ":dep",
                    "--require-hash-change-deps",
                    "root//:dep",
                ],
            ]
            .concat(),
        )
        .await?
    );
    Ok(())
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_require_hash_change_deps_ignores_order_and_unrelated_targets_False_fast(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_require_hash_change_deps_ignores_order_and_unrelated_targets(buck, "fast", false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_require_hash_change_deps_ignores_order_and_unrelated_targets_False_strong(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_require_hash_change_deps_ignores_order_and_unrelated_targets(buck, "strong", false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_require_hash_change_deps_ignores_order_and_unrelated_targets_True_fast(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_require_hash_change_deps_ignores_order_and_unrelated_targets(buck, "fast", true).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_require_hash_change_deps_ignores_order_and_unrelated_targets_True_strong(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_require_hash_change_deps_ignores_order_and_unrelated_targets(buck, "strong", true).await
}

async fn test_require_hash_change_deps_matches_all_configurations(
    buck: &Buck,
    recursive: bool,
) -> anyhow::Result<()> {
    let mut args = if recursive {
        vec!["--target-hash-recursive"]
    } else {
        vec![]
    };
    let baseline = target_hash(buck, "root//:same?root//:linux", &args).await?;
    args.extend(["--require-hash-change-deps", "root//:same"]);
    let changed = target_hash(buck, "root//:same?root//:linux", &args).await?;
    assert_ne!(baseline, changed);
    assert_eq!(
        changed,
        target_hash(buck, "root//:same?root//:macos", &args).await?
    );
    Ok(())
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_require_hash_change_deps_matches_all_configurations_False(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_require_hash_change_deps_matches_all_configurations(buck, false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_require_hash_change_deps_matches_all_configurations_True(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_require_hash_change_deps_matches_all_configurations(buck, true).await
}

#[buck_test]
async fn test_require_hash_change_deps_requires_hash_output(buck: &Buck) {
    buck.ctargets(&["root//:same", "--require-hash-change-deps", "root//:same"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"required arguments were not provided:[\s\S]*--show-target-hash");
}

#[buck_test]
async fn test_require_hash_change_deps_requires_target_labels(buck: &Buck) {
    buck.ctargets(&[
        "root//:same",
        "--show-target-hash",
        "--require-hash-change-deps",
        "root//...",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Required a target literal, but got a non-literal pattern");
}

async fn test_hash_function_selects_fast_or_strong(
    buck: &Buck,
    recursive: bool,
) -> anyhow::Result<()> {
    let target = "root//:parent?root//:linux";
    let args: &[&str] = if recursive {
        &["--target-hash-recursive"]
    } else {
        &[]
    };
    let fast_args = [&["--target-hash-function=fast"], args].concat();
    let strong_args = [&["--target-hash-function=strong"], args].concat();
    let fast = target_hash(buck, target, &fast_args).await?;
    let strong = target_hash(buck, target, &strong_args).await?;

    assert_eq!(fast.len(), 32);
    assert_eq!(strong.len(), 64);
    assert_eq!(fast, target_hash(buck, target, &fast_args).await?);
    assert_eq!(strong, target_hash(buck, target, &strong_args).await?);
    Ok(())
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_function_selects_fast_or_strong_False(buck: &Buck) -> anyhow::Result<()> {
    test_hash_function_selects_fast_or_strong(buck, false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_hash_function_selects_fast_or_strong_True(buck: &Buck) -> anyhow::Result<()> {
    test_hash_function_selects_fast_or_strong(buck, true).await
}

async fn test_hash_preserves_dependency_order(
    buck: &Buck,
    target: &str,
    hash_function: &str,
) -> anyhow::Result<()> {
    let label = format!("root//:{target}");
    let hash_arg = format!("--target-hash-function={hash_function}");
    let hash_arg = hash_arg.as_str();
    let swapped_args = ["-c", "test.swap_platforms=true"];
    let recursive_arg = "--target-hash-recursive";

    assert_eq!(
        target_hash(buck, &label, &[hash_arg]).await?,
        target_hash(
            buck,
            &label,
            &[&[hash_arg], swapped_args.as_slice()].concat()
        )
        .await?
    );
    assert_ne!(
        target_hash(buck, &label, &[hash_arg, recursive_arg]).await?,
        target_hash(
            buck,
            &label,
            &[&[hash_arg, recursive_arg], swapped_args.as_slice()].concat()
        )
        .await?
    );
    Ok(())
}

#[buck_test]
async fn test_hash_preserves_dependency_order_fast_attribute_order(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_hash_preserves_dependency_order(buck, "attribute_order", "fast").await
}

#[buck_test]
async fn test_hash_preserves_dependency_order_fast_list_order(buck: &Buck) -> anyhow::Result<()> {
    test_hash_preserves_dependency_order(buck, "list_order", "fast").await
}

#[buck_test]
async fn test_hash_preserves_dependency_order_fast_split_order(buck: &Buck) -> anyhow::Result<()> {
    test_hash_preserves_dependency_order(buck, "split_order", "fast").await
}

#[buck_test]
async fn test_hash_preserves_dependency_order_strong_attribute_order(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_hash_preserves_dependency_order(buck, "attribute_order", "strong").await
}

#[buck_test]
async fn test_hash_preserves_dependency_order_strong_list_order(buck: &Buck) -> anyhow::Result<()> {
    test_hash_preserves_dependency_order(buck, "list_order", "strong").await
}

#[buck_test]
async fn test_hash_preserves_dependency_order_strong_split_order(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_hash_preserves_dependency_order(buck, "split_order", "strong").await
}

#[buck_test]
async fn test_hash_ignores_configuration_of_equal_dependencies(buck: &Buck) -> anyhow::Result<()> {
    assert_eq!(
        target_hash(buck, "root//:equal_deps", &["--target-hash-recursive"]).await?,
        target_hash(
            buck,
            "root//:equal_deps",
            &["--target-hash-recursive", "-c", "test.swap_platforms=true"],
        )
        .await?
    );
    Ok(())
}

#[buck_test]
async fn test_hash_tracks_swapped_dependency_contents(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:attribute_order";
    let recursive_arg = "--target-hash-recursive";
    let baseline = target_hash(buck, target, &[recursive_arg]).await?;
    let local_baseline = target_hash(buck, target, &[]).await?;

    let targets_file = buck.cwd().join("TARGETS.fixture");
    std::fs::write(
        &targets_file,
        std::fs::read_to_string(&targets_file)?
            .replace(r#"":linux": "linux""#, r#"":linux": "macos""#)
            .replace(r#"":macos": "macos""#, r#"":macos": "linux""#),
    )?;
    buck.kill().await?;

    assert_eq!(local_baseline, target_hash(buck, target, &[]).await?);
    assert_ne!(baseline, target_hash(buck, target, &[recursive_arg]).await?);
    Ok(())
}
