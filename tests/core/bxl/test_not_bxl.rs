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
async fn test_not_bxl(buck: &Buck) {
    buck.bxl(&["//not_bxl.bxl:not_bxl"])
        .await
        .expect_failure()
        .assert_stderr_matches("Expected value of type `bxl` but got `function");
}

#[buck_test]
async fn test_not_allowed_now(buck: &Buck) {
    buck.build(&[":"])
        .await
        .expect_failure()
        .assert_stderr_matches("This function can only be called from Bxl");
}
