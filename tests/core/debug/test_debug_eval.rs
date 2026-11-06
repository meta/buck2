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
async fn test_debug_eval_good(buck: &Buck) -> anyhow::Result<()> {
    buck.debug(&["eval", "./good.bzl", "./good.bxl"]).await?;
    Ok(())
}

#[buck_test]
async fn test_debug_eval_bad_bzl(buck: &Buck) {
    buck.debug(&["eval", "./bad.bzl"])
        .await
        .expect_failure()
        .assert_stderr_matches("fail: bad bzl");
}

#[buck_test]
async fn test_debug_eval_bad_bxl(buck: &Buck) {
    buck.debug(&["eval", "./bad.bxl"])
        .await
        .expect_failure()
        .assert_stderr_matches("fail: bad bxl");
}
