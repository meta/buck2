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

#[buck_test(skip_for_os = ["windows"])]
async fn test_run_action_timeout_expires(buck: &Buck) {
    buck.build(&["//:slow_with_timeout"])
        .await
        .expect_failure()
        .assert_stderr_matches("timed out after");
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_run_action_timeout_succeeds(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//:fast_with_timeout"]).await?;
    let output = result
        .build_report()?
        .output_for_target("//:fast_with_timeout")?;
    assert_eq!(std::fs::read_to_string(output)?.trim(), "hello");
    Ok(())
}
