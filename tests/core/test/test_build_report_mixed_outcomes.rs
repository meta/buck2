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

use buck2_e2e::ansi::remove_ansi_escape_sequences;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::is_open_source;
use serde_json::Value;

#[buck_test]
async fn test_passing_test_and_failed_build_action(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let report_path = tmp_path.join("build-report.json");
    let report_arg = report_path.to_string_lossy();
    let mut args = vec![
        "root//:failed_build",
        "root//:passing_test",
        "--build-default-info",
        "--build-report",
        &report_arg,
    ];
    if !is_open_source() {
        args.extend(["--", "--no-record-results"]);
    }
    let output = buck.test(&args).await.expect_failure();
    let stderr = remove_ansi_escape_sequences(&output.stderr);
    assert_eq!(output.exit_code(), Some(3), "{stderr}");
    assert!(stderr.contains("Tests finished: Pass 1."), "{stderr}");

    let report: Value = serde_json::from_str(&tokio::fs::read_to_string(report_path).await?)?;
    assert_eq!(report["test_exit_code"], 0, "the sibling test passed");
    assert_eq!(report["success"], false);
    let failed_build = &report["results"]["root//:failed_build"];
    assert_eq!(failed_build["success"], "FAIL");
    let configured = failed_build["configured"].as_object().unwrap();
    assert!(
        configured.values().any(|target| {
            target["errors"]
                .as_array()
                .is_some_and(|errors| errors.iter().any(|error| error["action_error"].is_object()))
        }),
        "expected a failed action, got {failed_build:#}"
    );
    Ok(())
}
