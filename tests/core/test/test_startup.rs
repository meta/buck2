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

#[buck_test(env = [("BUCK2_TEST_TPX_USE_TCP", "true")])]
async fn test_tcp_startup_fail(buck: &mut Buck) {
    // Python is a binary that will just fail when we give it our executor args
    // but works on any platform. It's a bit dumb but it'll do
    buck.set_env("BUCK2_TPX", "fbpython");
    buck.test(&["..."])
        .await
        .expect_failure()
        .assert_stderr_matches("Executor exited before connecting");
}
