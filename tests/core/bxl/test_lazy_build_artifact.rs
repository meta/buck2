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
async fn test_build_artifact(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .bxl(&["//:lazy_build_artifact.bxl:build_artifact"])
        .await?;
    assert!(res.stdout.contains("foo.txt"));
    assert!(res.stdout.contains("bar.txt"));
    Ok(())
}

#[buck_test]
async fn test_build_artifact_catch_error(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .bxl(&["//:lazy_build_artifact.bxl:build_artifact_fail"])
        .await?;
    assert!(res.stdout.contains("foo.txt"));
    Ok(())
}

#[buck_test]
async fn test_cannot_build_dynmiac_action_output(buck: &Buck) {
    buck.bxl(&["//:lazy_build_artifact.bxl:dynamic"])
        .await
        .expect_failure()
        .assert_stderr_matches("does not accept declared artifact");
}

#[buck_test]
async fn test_cannot_bxl_action_output(buck: &Buck) {
    buck.bxl(&["//:lazy_build_artifact.bxl:bxl_action_output"])
        .await
        .expect_failure()
        .assert_stderr_matches("does not accept declared artifact");
}
