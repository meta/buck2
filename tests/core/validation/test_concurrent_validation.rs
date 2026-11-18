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
use buck2_e2e::utils::random_string;

#[buck_test]
async fn test_validation_concurrent(buck: &Buck) {
    // There are 2 actions — slow build action and fast validation action.
    // Check that validation doesn't wait for a slow DefaultInfo artifact to be built and fails the build first.
    buck.build(&[
        ":plate",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Validation for `.+` failed");
}
