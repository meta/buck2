/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// FIXME(JakobDegen): These tests should be isolated and moved into `tests/isolated/audit`

use buck2_e2e::prelude::*;

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_audit_classpath(buck: &Buck) {
    buck.audit(&[
        "classpath",
        "fbsource//fbandroid/buck2/tests/good/classpath:top",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(r"Using `audit classpath` is no longer supported. Use the `\[classpath\]` or `\[classpath_targets\]` sub-targets instead.");
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_audit_classpath_binary(buck: &Buck) {
    buck.audit(&[
        "classpath",
        "fbsource//fbandroid/buck2/tests/good/classpath:apk",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(r"Using `audit classpath` is no longer supported. Use the `\[classpath\]` or `\[classpath_targets\]` sub-targets instead.");
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_audit_classpath_json(buck: &Buck) {
    let top = "fbsource//fbandroid/buck2/tests/good/classpath:top";
    let direct_dep = "fbsource//fbandroid/buck2/tests/good/classpath:direct_dep";

    buck.audit(&["classpath", top, direct_dep, "--json"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"Using `audit classpath` is no longer supported. Use the `\[classpath\]` or `\[classpath_targets\]` sub-targets instead.");
}
