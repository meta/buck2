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
async fn test_unbound_artifact(buck: &Buck) {
    buck.build(&["root//:action_with_unbound_artifact"])
        .await
        .expect_failure()
        .assert_stderr_matches("error: Artifact must be bound by now");
}

#[buck_test]
async fn test_unbound_artifact_inside_tset(buck: &Buck) {
    buck.build(&["root//:action_with_unbound_artifact"])
        .await
        .expect_failure()
        .assert_stderr_matches("error: Artifact must be bound by now");
}
