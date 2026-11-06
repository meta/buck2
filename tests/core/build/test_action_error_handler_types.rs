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
async fn test_action_error_handler_types(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:test_action_error_handler_types.bxl:test_action_error_handler_types"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_output_when_no_error_handler_used(buck: &Buck) {
    let failure = buck
        .build(&["//:does_not_use_error_handler"])
        .await
        .expect_failure();

    assert!(
        !failure
            .stderr
            .contains("Action sub-errors produced by error handlers: <empty>")
    );
}

#[buck_test]
async fn test_error_handler_succeed_on_nonetype(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:error_handler_nonetype"]).await?;
    Ok(())
}

#[buck_test]
async fn test_output_for_error_handler_with_errorformat(buck: &Buck) {
    let failure = buck
        .build(&["//:error_handler_with_errorformat"])
        .await
        .expect_failure();

    assert!(
        failure
            .stderr
            .contains("- [test_failure] main.rs:10 expected `;`, found `}`")
    );
    assert!(!failure.stderr.contains("manually created sub error"));
}
