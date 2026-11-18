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

#[buck_test]
async fn test_bxl_analysis(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//analysis.bxl:providers_test"]).await?;

    let lines: Vec<&str> = result.stdout.lines().collect();
    assert!(lines[0].contains("provides_foo_foo"));
    assert!(lines[1].contains("provides_foo_foo"));

    let result = buck.bxl(&["//analysis.bxl:dependency_test"]).await?;

    assert_eq!(
        result.stdout.lines().collect::<Vec<_>>(),
        ["Dependency", "root//:stub (<unspecified>)"]
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_bxl_analysis_missing_subtarget(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .bxl(&["//analysis.bxl:missing_subtarget_test"])
        .await
        .expect_failure();
    res.assert_stderr_matches("requested sub target named `missing_subtarget` .* is not available");

    let record = res.invocation_record()?;
    let errors = record["errors"].as_array().expect("`errors` is a list");

    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["category"], "USER");
    Ok(())
}

#[buck_test]
async fn test_bxl_analysis_unconfigured_target_error(buck: &Buck) {
    buck.bxl(&["//analysis.bxl:unconfigured_target_error_test"])
        .await
        .expect_failure()
        .assert_stderr_matches("Type of parameter `labels` doesn't match");
}

#[buck_test]
async fn test_bxl_analysis_subtarget_order(buck: &Buck) -> anyhow::Result<()> {
    // More labels than one prepare batch, so results are reassembled across batches.
    let result = buck
        .bxl(&["//analysis.bxl:ordered_subtargets_test"])
        .await?;
    assert_eq!(
        result.stdout.lines().collect::<Vec<_>>(),
        (0..=129)
            .rev()
            .map(|i| format!("child_{i}"))
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[buck_test]
async fn test_bxl_analysis_skip_incompatible(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&[
            "//analysis.bxl:incompatible_skip_test",
            "--",
            "--target",
            "root//:incompatible_target",
        ])
        .await?;
    assert_eq!(
        result.stdout.lines().collect::<Vec<_>>(),
        ["None", "0", "provides_foo_foo"]
    );
    assert!(result.stderr.contains("incompatible_target"));
    Ok(())
}

#[buck_test]
async fn test_bxl_analysis_reject_incompatible(buck: &Buck) {
    for entry in ["incompatible_error_test", "incompatible_list_error_test"] {
        buck.bxl(&[
            &format!("//analysis.bxl:{entry}"),
            "--",
            "--target",
            "root//:incompatible_target",
        ])
        .await
        .expect_failure()
        .assert_stderr_matches("incompatible_target.*is incompatible");
    }
}
