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

// The grace period comes from a file the workspace's `.buckconfig` includes, as CI delivers it.
#[buck_test(skip_for_os = ["windows"])]
async fn test_timed_out_test_reports_on_sigterm(buck: &Buck) {
    buck.test(&[":reports_on_sigterm", "--", "--timeout", "2"])
        .await
        .expect_failure()
        .assert_stderr_matches("reported on SIGTERM");
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_no_grace_period_kills_without_sigterm(buck: &Buck) {
    let output = buck
        .test(&[
            "-c",
            "buck2.test_timeout_grace_period_s=0",
            ":reports_on_sigterm",
            "--",
            "--timeout",
            "2",
        ])
        .await
        .expect_failure();
    assert!(
        !output.stderr.contains("reported on SIGTERM"),
        "{}",
        output.stderr
    );
}
