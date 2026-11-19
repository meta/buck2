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
use buck2_e2e::golden::sanitize_build_report;
use buck2_e2e::golden::sanitize_build_report_error;
use buck2_e2e::json::Json;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::read_streaming_report;
use buck2_e2e::utils::replay_streaming_report;
use buck2_e2e::utils::streaming_status_history;
use serde_json::Value;

/// Runs `buck2 test` on `command` with a build report, failing or not as `should_fail` says, and
/// returns the report.
async fn test_with_build_report(
    buck: &Buck,
    tmp_path: &Path,
    command: &[&str],
    should_fail: bool,
) -> anyhow::Result<Value> {
    let report = tmp_path.join("build-report.json");
    let report_arg = report.to_string_lossy();
    if should_fail {
        let mut args = vec![
            "--build-report",
            &report_arg,
            "--build-report-options",
            "fill-out-failures",
        ];
        args.extend(command);
        buck.test(&args).await.expect_failure();
    } else {
        let mut args = vec!["--build-report", &report_arg];
        args.extend(command);
        buck.test(&args).await?;
    }

    Ok(serde_json::from_str(&std::fs::read_to_string(&report)?)?)
}

async fn build_report_test(
    buck: &Buck,
    tmp_path: &Path,
    name: &str,
    command: &[&str],
    should_fail: bool,
) -> anyhow::Result<()> {
    let mut report = test_with_build_report(buck, tmp_path, command, should_fail).await?;

    sanitize_build_report(&mut report);

    golden(
        &sanitize_build_report_error(&Json::from(report.clone()).dumps(Some(2), true)),
        &format!("{GOLDEN_DIRECTORY}{name}.golden.json"),
    );
    Ok(())
}

async fn modifiers_match_test(
    buck: &Buck,
    tmp_path: &Path,
    command: &[&str],
    expected_configurations: usize,
    should_fail: bool,
) -> anyhow::Result<()> {
    let report = test_with_build_report(buck, tmp_path, command, should_fail).await?;

    let mut num_configurations = 0;
    for (target_pattern, entry) in report["results"].as_object().into_iter().flatten() {
        // grabs the modifiers from the pattern as a list of strings
        let modifiers: Vec<&str> = target_pattern
            .split('?')
            .nth(1)
            .unwrap_or_else(|| panic!("`{target_pattern}` has no modifiers"))
            .split('+')
            .collect();

        // for configuration id associated with the target_pattern
        // i.e. cfg:<empty>#0899b0510451fef9
        // check if the associated modifiers exist in the configuration
        for configuration_id in entry["configured"]
            .as_object()
            .into_iter()
            .flat_map(|configured| configured.keys())
        {
            num_configurations += 1;
            let configurations = buck
                .audit_configurations(&[configuration_id.as_str()])
                .await?;

            for modifier in &modifiers {
                assert!(configurations.stdout.contains(modifier));
            }
        }
    }

    assert_eq!(num_configurations, expected_configurations);
    Ok(())
}

#[buck_test]
async fn test_build_report_package(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_package",
        &["//:?root//:macos"],
        true,
    )
    .await
}

#[buck_test]
async fn test_build_report_ambiguous(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_ambiguous",
        &["//subdir?root//:macos"],
        false,
    )
    .await
}

#[buck_test]
async fn test_single_modifier_single_pattern(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    modifiers_match_test(buck, tmp_path, &["//:ok?root//:macos"], 1, false).await
}

#[buck_test]
async fn test_build_report_format_all_ok(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_format_all_ok",
        &[
            "//:ok?root//:macos",
            "//:ok?root//:linux",
            "//:ok?root//:macos+root//:arm",
        ],
        false,
    )
    .await
}

#[buck_test]
async fn test_build_report_format_all_fail(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_format_all_fail",
        &[
            "//:fail?root//:macos",
            "//:fail?root//:linux",
            "//:fail?root//:macos+root//:arm",
        ],
        true,
    )
    .await
}

#[buck_test]
async fn test_build_report_fail_build(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_fail_build",
        &["//:fail_build?root//:linux", "//:fail_build?root//:macos"],
        true,
    )
    .await
}

#[buck_test]
async fn test_build_report_mixed(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_mixed",
        &[
            "//:ok?root//:macos",
            "//:fail?root//:linux",
            "//:fail_build?root//:arm",
        ],
        true,
    )
    .await
}

#[buck_test]
async fn test_build_report_recursive(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_recursive",
        &["//...?root//:macos"],
        true,
    )
    .await
}

#[buck_test]
async fn test_build_report_deduplicate(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_deduplicate",
        &[
            "//:?root//:macos",
            "//:ok?root//:macos",
            "//:ok?root//:linux",
        ],
        true,
    )
    .await
}

#[buck_test]
async fn test_build_report_modifiers_that_result_in_same_configured(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_build_report_modifiers_that_result_in_same_configured",
        &[
            "//:ok?root//:linux+root//:arm",
            "//:ok?root//:arm+root//:linux",
        ],
        false,
    )
    .await
}

#[buck_test]
async fn test_multiple_modifiers_single_pattern(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    modifiers_match_test(buck, tmp_path, &["//:ok?root//:macos+root//:arm"], 1, false).await
}

#[buck_test]
async fn test_single_modifier_multiple_patterns(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    modifiers_match_test(
        buck,
        tmp_path,
        &["//:ok?root//:macos", "//:fail?root//:macos"],
        2,
        true,
    )
    .await
}

#[buck_test]
async fn test_multiple_modifiers_multiple_patterns(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    modifiers_match_test(
        buck,
        tmp_path,
        &["//:ok?root//:macos+root//:arm", "//:fail?root//:linux"],
        2,
        true,
    )
    .await
}

#[buck_test]
async fn test_recursive_modifiers(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    modifiers_match_test(buck, tmp_path, &["//...?root//:arm"], 4, true).await
}

#[buck_test]
async fn test_package_modifiers(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    modifiers_match_test(buck, tmp_path, &["//:?root//:arm"], 3, true).await
}

#[buck_test]
async fn test_ambiguous_modifiers(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    modifiers_match_test(buck, tmp_path, &["//subdir?root//:arm"], 1, false).await
}

#[buck_test]
async fn test_modifiers_complex(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    modifiers_match_test(
        buck,
        tmp_path,
        &[
            "//...?root//:macos+root//:arm",
            "//:?root//:linux",
            "//:ok?root//:linux",
        ],
        7,
        true,
    )
    .await
}

/// The one configuration a build report has for `target`.
fn single_configuration<'a>(report: &'a Value, target: &str) -> &'a str {
    let configured = report["results"][target]["configured"]
        .as_object()
        .unwrap_or_else(|| panic!("no configured results for `{target}`"));
    let mut configurations = configured.keys();
    match (configurations.next(), configurations.next()) {
        (Some(configuration), None) => configuration,
        _ => panic!(
            "expected one configuration for `{target}`, got {:?}",
            configured.keys().collect::<Vec<_>>()
        ),
    }
}

#[buck_test]
async fn test_order_of_modifiers(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let target_with_modifiers = "root//:ok?root//:linux+root//:macos";
    let report = tmp_path.join("build-report.json");

    buck.test(&[
        "--build-report",
        &report.to_string_lossy(),
        target_with_modifiers,
    ])
    .await?;

    let report: Value = serde_json::from_str(&std::fs::read_to_string(&report)?)?;

    let configuration = single_configuration(&report, target_with_modifiers);

    let cfg = buck.audit_configurations(&[configuration]).await?;

    assert!(cfg.stdout.contains("root//:macos"));
    assert!(!cfg.stdout.contains("root//:linux"));
    Ok(())
}

#[buck_test]
async fn test_modifiers_that_end_up_with_same_configuration(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mac_first = "root//:ok?root//:macos+root//:arm";
    let arm_first = "root//:ok?root//:arm+root//:macos";
    let report = tmp_path.join("build-report.json");

    buck.test(&[
        "--build-report",
        &report.to_string_lossy(),
        mac_first,
        arm_first,
    ])
    .await?;

    let report: Value = serde_json::from_str(&std::fs::read_to_string(&report)?)?;

    assert_eq!(
        report["results"][mac_first]["configured"],
        report["results"][arm_first]["configured"]
    );

    let configuration = single_configuration(&report, mac_first);

    let cfg = buck.audit_configurations(&[configuration]).await?;

    assert!(cfg.stdout.contains("root//:macos"));
    assert!(cfg.stdout.contains("root//:arm"));
    Ok(())
}

/// Each `?modifier` entry, and its configuration, finishes with its own label.
#[buck_test]
async fn test_streaming_build_report_target_completion_with_modifiers(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");

    buck.test(&[
        "//:ok?root//:macos",
        "//:ok?root//:linux",
        "--streaming-build-report",
        &streaming_report.to_string_lossy(),
    ])
    .await?;

    let reports = read_streaming_report(&streaming_report)?;
    assert_eq!(
        streaming_status_history(&reports),
        BTreeMap::from([
            (
                "root//:ok?root//:macos".to_owned(),
                vec![Value::from("IN_PROGRESS"), Value::from("SUCCESS")]
            ),
            (
                "root//:ok?root//:linux".to_owned(),
                vec![Value::from("IN_PROGRESS"), Value::from("SUCCESS")]
            ),
        ])
    );
    assert_eq!(
        replay_streaming_report(&reports)
            .into_iter()
            .map(|(key, entry)| {
                let statuses: Vec<Value> = entry["configured"]
                    .as_object()
                    .into_iter()
                    .flat_map(|configured| configured.values())
                    .map(|configured| configured["success"].clone())
                    .collect();
                (key, statuses)
            })
            .collect::<BTreeMap<_, _>>(),
        BTreeMap::from([
            (
                "root//:ok?root//:macos".to_owned(),
                vec![Value::from("SUCCESS")]
            ),
            (
                "root//:ok?root//:linux".to_owned(),
                vec![Value::from("SUCCESS")]
            ),
        ])
    );
    Ok(())
}

#[buck_test]
async fn test_fails_with_global_modifiers(buck: &Buck) {
    buck.test(&["--modifier", "root//:macos", "//:ok?root//:linux"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
        );
}
