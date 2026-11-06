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

use buck2_e2e::golden::GOLDEN_DIRECTORY;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::read_streaming_report;
use buck2_e2e::utils::replay_streaming_report;
use buck2_e2e::utils::streaming_status_history;
use serde_json::Value;
use serde_json::json;

async fn run_all_output_flags(buck: &Buck, argv: &[&str]) -> anyhow::Result<String> {
    let flags = [
        "--show-output",
        "--show-full-output",
        "--show-simple-output",
        "--show-full-simple-output",
        "--show-json-output",
        "--show-full-json-output",
    ];

    let mut results = Vec::new();
    for flag in flags {
        let mut args = vec![flag];
        args.extend(argv);
        let result = buck.build_without_report(&args).await?;
        results.push(format!("{flag}\n{}", result.stdout));
    }

    let output = results.join("\n\n");
    let output = output.replace("\\\\", "\\"); // Windows path separators in json
    let output = output.replace(&*buck.cwd().to_string_lossy(), "/abs/project/root");
    let output = output.replace('\\', "/"); // Windows path separators not in json

    Ok(output)
}

/// The configuration of the one configured target `target` has in the build report `output`.
fn single_configuration(output: &Value, target: &str) -> String {
    let configurations: Vec<&String> = output["results"][target]["configured"]
        .as_object()
        .unwrap_or_else(|| panic!("No configured `{target}` in {output}"))
        .keys()
        .collect();
    let [configuration] = configurations.as_slice() else {
        panic!("Expected one configuration of `{target}`, got {configurations:?}");
    };
    (*configuration).clone()
}

#[buck_test]
async fn test_build_with_single_modifier(buck: &Buck) -> anyhow::Result<()> {
    let target_with_modifiers = "root//:dummy?root//:macos";

    let result = buck.build(&[target_with_modifiers]).await?;

    let output: Value = serde_json::from_str(&result.stdout)?;

    let configuration = single_configuration(&output, target_with_modifiers);

    let cfg = buck.audit_configurations(&[&configuration]).await?;

    assert!(cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_build_with_multiple_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let target_with_modifiers = "root//:dummy?root//:macos+root//:arm";
    let result = buck.build(&[target_with_modifiers]).await?;

    let output: Value = serde_json::from_str(&result.stdout)?;

    let configuration = single_configuration(&output, target_with_modifiers);

    let cfg = buck.audit_configurations(&[&configuration]).await?;

    assert!(cfg.stdout.contains("root//:macos"));
    assert!(cfg.stdout.contains("root//:arm"));
    Ok(())
}

#[buck_test]
async fn test_build_order_of_modifiers(buck: &Buck) -> anyhow::Result<()> {
    // if passing in modifiers of the same constraint setting,
    // the last one should be the one that applies
    let target_with_modifiers = "root//:dummy?root//:linux+root//:macos";
    let result = buck.build(&[target_with_modifiers]).await?;

    let output: Value = serde_json::from_str(&result.stdout)?;

    let configuration = single_configuration(&output, target_with_modifiers);

    let cfg = buck.audit_configurations(&[&configuration]).await?;

    assert!(cfg.stdout.contains("root//:macos"));
    assert!(!cfg.stdout.contains("root//:linux"));
    Ok(())
}

#[buck_test]
async fn test_build_with_different_targets_and_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let mac_target = "root//:dummy?root//:macos";
    let linux_target = "root//:dummy2?root//:linux";

    let result = buck.build(&[mac_target, linux_target]).await?;

    let output: Value = serde_json::from_str(&result.stdout)?;

    let configuration = single_configuration(&output, mac_target);
    let cfg = buck.audit_configurations(&[&configuration]).await?;
    assert!(cfg.stdout.contains("root//:macos"));

    let configuration = single_configuration(&output, linux_target);
    let cfg = buck.audit_configurations(&[&configuration]).await?;
    assert!(cfg.stdout.contains("root//:linux"));
    Ok(())
}

#[buck_test]
async fn test_build_with_same_target_different_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let mac_target = "root//:dummy?root//:macos";
    let linux_target = "root//:dummy?root//:linux";

    let result = buck.build(&[mac_target, linux_target]).await?;

    let output: Value = serde_json::from_str(&result.stdout)?;

    let configuration = single_configuration(&output, mac_target);
    let cfg = buck.audit_configurations(&[&configuration]).await?;
    assert!(cfg.stdout.contains("root//:macos"));

    let configuration = single_configuration(&output, linux_target);
    let cfg = buck.audit_configurations(&[&configuration]).await?;
    assert!(cfg.stdout.contains("root//:linux"));
    Ok(())
}

#[buck_test]
async fn test_build_with_same_target_and_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let target_with_modifier = "root//:dummy?root//:macos";
    let result = buck
        .build(&[target_with_modifier, target_with_modifier])
        .await?;

    let output: Value = serde_json::from_str(&result.stdout)?;

    let configuration = single_configuration(&output, target_with_modifier);

    let cfg = buck.audit_configurations(&[&configuration]).await?;
    assert!(cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_build_with_target_universe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "root//:dummy",
            "--target-universe",
            "root//:universe?root//:linux",
        ])
        .await?;

    let output: Value = serde_json::from_str(&result.stdout)?;

    let configuration = single_configuration(&output, "root//:dummy");

    let cfg = buck.audit_configurations(&[&configuration]).await?;

    assert!(cfg.stdout.contains("root//:linux"));
    Ok(())
}

#[buck_test]
async fn test_build_with_target_universe_multiple_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "root//:dummy",
            "--target-universe",
            "root//:universe?root//:linux+root//:arm",
        ])
        .await?;

    let output: Value = serde_json::from_str(&result.stdout)?;

    let configuration = single_configuration(&output, "root//:dummy");

    let cfg = buck.audit_configurations(&[&configuration]).await?;

    assert!(cfg.stdout.contains("root//:linux"));
    assert!(cfg.stdout.contains("root//:arm"));
    Ok(())
}

#[buck_test]
async fn test_build_with_mutliple_target_universes(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "root//:dummy",
            "--target-universe",
            "root//:universe?root//:linux,root//:dummy?root//:macos+root//:arm",
        ])
        .await?;

    let output: Value = serde_json::from_str(&result.stdout)?;

    let configurations: Vec<&String> = output["results"]["root//:dummy"]["configured"]
        .as_object()
        .unwrap_or_else(|| panic!("No configured `root//:dummy` in {output}"))
        .keys()
        .collect();

    assert_eq!(configurations.len(), 2);

    let mut linux_found = false;
    let mut macos_found = false;
    for configuration in configurations {
        let cfg = buck.audit_configurations(&[configuration]).await?;
        if cfg.stdout.contains("root//:linux") {
            linux_found = true;
        }
        if cfg.stdout.contains("root//:macos") && cfg.stdout.contains("root//:arm") {
            macos_found = true;
        }
    }

    assert!(linux_found);
    assert!(macos_found);
    Ok(())
}

#[buck_test]
async fn test_streaming_build_report_with_multiple_target_universes(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");

    buck.build(&[
        "root//:dummy",
        "--target-universe",
        "root//:universe?root//:linux,root//:dummy?root//:macos+root//:arm",
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
    ])
    .await?;

    let reports = read_streaming_report(&streaming_report)?;
    // One line per configuration's output, one per configured target finishing, and one
    // when the target finishes.
    assert_eq!(reports.len(), 5, "Expected 5 lines, got {}", reports.len());
    let results = replay_streaming_report(&reports);
    assert_eq!(results.keys().collect::<Vec<_>>(), ["root//:dummy"]);
    assert_eq!(
        results["root//:dummy"]["configured"]
            .as_object()
            .map(|configured| configured.len()),
        Some(2)
    );

    assert_eq!(
        streaming_status_history(&reports),
        BTreeMap::from([(
            "root//:dummy".to_owned(),
            vec![json!("IN_PROGRESS"), json!("SUCCESS")]
        )])
    );
    // The target finishes only after both of its configured targets have.
    for report in &reports {
        let entry = report
            .get("results")
            .and_then(|results| results.get("root//:dummy"))
            .unwrap_or_else(|| panic!("No `root//:dummy` result in {report}"));
        if entry["success"] == "SUCCESS" {
            let statuses: Vec<&Value> = entry["configured"]
                .as_object()
                .unwrap_or_else(|| panic!("No `configured` in {entry}"))
                .values()
                .map(|c| &c["success"])
                .collect();
            assert_eq!(statuses, ["SUCCESS", "SUCCESS"], "{entry}");
        }
    }
    Ok(())
}

#[buck_test]
async fn test_build_with_package_pattern(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["root//:?root//:macos"]).await?;

    let output: Value = serde_json::from_str(&result.stdout)?;

    let configuration = single_configuration(&output, "root//:dummy?root//:macos");
    let cfg = buck.audit_configurations(&[&configuration]).await?;
    assert!(cfg.stdout.contains("root//:macos"));

    let configuration = single_configuration(&output, "root//:dummy2?root//:macos");
    let cfg = buck.audit_configurations(&[&configuration]).await?;
    assert!(cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_build_with_recursive_pattern(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["root//...?root//:macos"]).await?;

    let output: Value = serde_json::from_str(&result.stdout)?;

    let configuration = single_configuration(&output, "root//:dummy?root//:macos");
    let cfg = buck.audit_configurations(&[&configuration]).await?;
    assert!(cfg.stdout.contains("root//:macos"));

    let configuration = single_configuration(&output, "root//:dummy2?root//:macos");
    let cfg = buck.audit_configurations(&[&configuration]).await?;
    assert!(cfg.stdout.contains("root//:macos"));

    let configuration = single_configuration(
        &output,
        "root//recursive_pattern:recursive_target?root//:macos",
    );
    let cfg = buck.audit_configurations(&[&configuration]).await?;
    assert!(cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_build_fails_with_global_modifiers(buck: &Buck) {
    buck.build(&["--modifier", "root//:macos", "root//:dummy?root//:linux"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
        );

    buck.build(&[
        "--modifier",
        "root//:macos",
        "root//:dummy",
        "--target-universe",
        "root//:dummy?root//:linux",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
    );
}

#[buck_test]
async fn test_build_fails_with_pattern_modifier_and_target_universe_modifier(buck: &Buck) {
    buck.build(&[
        "root//:dummy?root//:macos",
        "--target-universe",
        "root//:dummy?root//:linux",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Cannot use \?modifier syntax in target pattern expression with --target-universe flag",
    );
}

#[buck_test]
async fn test_build_modifiers_output_single_modifier(buck: &Buck) -> anyhow::Result<()> {
    let result = run_all_output_flags(buck, &["root//:dummy?root//:macos"]).await?;

    golden(
        &result,
        &format!("{GOLDEN_DIRECTORY}test_build_modifiers_output_single_modifier.golden.txt"),
    );
    Ok(())
}

#[buck_test]
async fn test_build_modifiers_output_multiple_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = run_all_output_flags(buck, &["root//:dummy?root//:macos+root//:arm"]).await?;

    golden(
        &result,
        &format!("{GOLDEN_DIRECTORY}test_build_modifiers_output_multiple_modifiers.golden.txt"),
    );
    Ok(())
}

#[buck_test]
async fn test_build_modifiers_output_multiple_patterns(buck: &Buck) -> anyhow::Result<()> {
    let result = run_all_output_flags(
        buck,
        &["root//:dummy?root//:macos", "root//:dummy?root//:linux"],
    )
    .await?;

    golden(
        &result,
        &format!("{GOLDEN_DIRECTORY}test_build_modifiers_output_multiple_patterns.golden.txt"),
    );
    Ok(())
}

#[buck_test]
async fn test_build_modifiers_output_multiple_modifiers_multiple_patterns(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result = run_all_output_flags(
        buck,
        &[
            "root//:dummy?root//:macos+root//:arm",
            "root//:dummy?root//:linux",
        ],
    )
    .await?;

    golden(
        &result,
        &format!(
            "{GOLDEN_DIRECTORY}test_build_modifiers_output_multiple_modifiers_multiple_patterns.golden.txt"
        ),
    );
    Ok(())
}

#[buck_test]
async fn test_build_modifiers_output_duplicate_patterns(buck: &Buck) -> anyhow::Result<()> {
    // Note: switching the order of the modifiers will make it so that both patterns are still in the output
    let result = run_all_output_flags(
        buck,
        &[
            "root//:dummy?root//:macos+root//:arm",
            "root//:dummy?root//:macos+root//:arm",
        ],
    )
    .await?;

    golden(
        &result,
        &format!("{GOLDEN_DIRECTORY}test_build_modifiers_output_duplicate_patterns.golden.txt"),
    );
    Ok(())
}

#[buck_test]
async fn test_build_modifiers_output_with_target_universe(buck: &Buck) -> anyhow::Result<()> {
    // Modifiers defined in target universe should not be included in the output
    let result = run_all_output_flags(
        buck,
        &[
            "root//:dummy",
            "--target-universe",
            "root//:dummy?root//:macos+root//:linux",
        ],
    )
    .await?;

    golden(
        &result,
        &format!("{GOLDEN_DIRECTORY}test_build_modifiers_output_with_target_universe.golden.txt"),
    );
    Ok(())
}
