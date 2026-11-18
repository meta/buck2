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
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;

async fn check_targets(
    buck: &Buck,
    expected_target_names: &[&str],
    expected_error_messages: &[&str],
) -> anyhow::Result<()> {
    let build_response = filter_events(buck, &["Result", "result", "build_response"]).await?;
    let build_response = &build_response[0];
    let build_targets = build_response["build_targets"]
        .as_array()
        .context("`build_targets` is not a list")?;
    assert_eq!(build_targets.len(), expected_target_names.len());
    for (actual, expected) in build_targets.iter().zip(expected_target_names) {
        assert_eq!(actual["target"], *expected);
    }
    let error_messages = build_response["errors"]
        .as_array()
        .context("`errors` is not a list")?;
    assert_eq!(error_messages.len(), expected_error_messages.len());
    for (actual_msg, expected) in error_messages.iter().zip(expected_error_messages) {
        assert!(
            actual_msg["message"]
                .as_str()
                .is_some_and(|m| m.contains(expected)),
            "`{expected}` not in {actual_msg}"
        );
    }
    Ok(())
}

#[buck_test]
async fn test_build_one_fails(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");
    buck.build(&[
        "--build-report",
        &report.to_string_lossy(),
        "//:fail",
        "//:a_one",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Failed to build 'root//:fail");
    check_targets(
        buck,
        &["root//:a_one", "root//:fail"],
        &["Failed to build 'root//:fail (<unspecified>)'"],
    )
    .await
}
