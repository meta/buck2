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
async fn test_conflicting_fbcode_coverage_flags_fail(buck: &Buck) {
    buck.test(&[
        "--config",
        "fbcode.coverage=true",
        "--config",
        "fbcode.coverage_selective=true",
        "fbcode//testing_frameworks/code_coverage/playground:test",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"fbcode.coverage and fbcode.coverage_selective are both true. Pick one.",
    );
}

#[buck_test(inplace = true)]
async fn test_fbcode_coverage_selective_require_filters(buck: &Buck) {
    buck.test(&[
        "--config",
        "fbcode.coverage_selective=true",
        "fbcode//testing_frameworks/code_coverage/playground:test",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(r"fbcode.coverage_selective=true with no filters");
}
