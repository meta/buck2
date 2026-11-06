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

#[buck_test(inplace = true)]
async fn test_re_resource_exhausted_reported_as_infra_failure(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .test(&[
            "--remote-only",
            "--no-remote-cache",
            "fbcode//buck2/tests/targets/rules/sh_test:test_remote_explicit",
        ])
        .env("BUCK2_TEST_FAIL_RE_RESOURCE_EXHAUSTED", "true")
        .await?;
    assert!(!result.stderr.contains("FATAL"));
    assert!(result.stderr.contains("Infra Failure 1"));
    assert!(result.stderr.to_lowercase().contains("resource exhausted"));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cancel_test_if_re_queue_longer_than_threshold(buck: &Buck) -> anyhow::Result<()> {
    let args = [
        "-c",
        "build.remote_execution_cancel_on_estimated_queue_time_exceeds_s=10",
        "--no-remote-cache",
        "--remote-only",
    ];
    let result = buck
        .test(
            &[
                &args[..],
                &["fbcode//buck2/tests/targets/rules/sh_test:test_remote_explicit_stays_in_queue"],
            ]
            .concat(),
        )
        .env("BUCK2_TEST_RE_QUEUE_ESTIMATE_S", "100")
        .await?;
    assert!(result.stderr.contains(
        "Omitted: fbcode//buck2/tests/targets/rules/sh_test:test_remote_explicit_stays_in_queue - unmanaged"
    ));
    assert!(
        result
            .stderr
            .contains("The test execution stayed in RE queue for more than threshold time.")
    );
    Ok(())
}
