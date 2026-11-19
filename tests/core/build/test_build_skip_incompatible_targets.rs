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

use buck2_e2e::prelude::*;
use buck2_e2e::utils::read_streaming_report;
use buck2_e2e::utils::streaming_status_history;
use serde_json::json;

#[buck_test]
async fn test_build_skip_incompatible(buck: &Buck) -> anyhow::Result<()> {
    let target_a = "root//:compatible-with-A";
    let target_b = "root//:compatible-with-B";
    let platform_a = "root//:platA";

    buck.build(&[
        target_a,
        target_b,
        &format!("--target-platforms={platform_a}"),
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(&format!(r"{target_b}\s*is incompatible with {platform_a}"));

    let result = buck
        .build(&[
            target_a,
            target_b,
            &format!("--target-platforms={platform_a}"),
            "--skip-incompatible-targets",
        ])
        .await?;
    assert!(result.stderr.contains("Skipped 1 incompatible target"));
    assert!(result.stderr.contains(target_b));

    let report = result.build_report()?;
    assert_eq!(report.results().len(), 2);
    assert_eq!(
        report.results()[target_a]["configured"]
            .as_object()
            .map(serde_json::Map::len),
        Some(1)
    );
    assert_eq!(
        report.results()[target_b]["configured"]
            .as_object()
            .map(serde_json::Map::len),
        Some(0)
    );
    Ok(())
}

/// A target whose only configuration is skipped still finishes, with no configurations.
#[buck_test]
async fn test_streaming_build_report_skip_incompatible(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let target_a = "root//:compatible-with-A";
    let target_b = "root//:compatible-with-B";
    let streaming_report = tmp_path.join("streaming-build-report.jsonl");

    buck.build(&[
        target_a,
        target_b,
        "--target-platforms=root//:platA",
        "--skip-incompatible-targets",
        "--streaming-build-report",
        streaming_report.to_str().unwrap(),
    ])
    .await?;

    let reports = read_streaming_report(&streaming_report)?;
    assert_eq!(
        streaming_status_history(&reports),
        BTreeMap::from([
            (
                target_a.to_owned(),
                vec![json!("IN_PROGRESS"), json!("SUCCESS")]
            ),
            (
                target_b.to_owned(),
                vec![json!("IN_PROGRESS"), json!("SUCCESS")]
            ),
        ])
    );
    assert!(
        reports
            .iter()
            .filter(|report| report["results"]
                .as_object()
                .is_some_and(|results| results.contains_key(target_b)))
            .all(|report| report["results"][target_b]["configured"] == json!({}))
    );
    Ok(())
}
