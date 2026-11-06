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
async fn test_ok(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["//:ok"]).await?;
    Ok(())
}

#[buck_test]
async fn test_fail(buck: &Buck) {
    buck.test(&["//:fail"])
        .await
        .expect_failure()
        .assert_stderr_matches("Fail: root//:fail - unmanaged");
}

#[buck_test]
async fn test_tests_attribute(buck: &Buck) {
    buck.test(&["//:noop_references_fail"])
        .await
        .expect_failure()
        .assert_stderr_matches("Fail: root//:fail - unmanaged");
}

#[buck_test]
async fn test_tests_attribute_transitive(buck: &Buck) {
    buck.test(&["//:noop_transitively_references_fail"])
        .await
        .expect_failure()
        .assert_stderr_matches("Fail: root//:fail - unmanaged");
}

#[buck_test]
async fn test_tests_attribute_cycle(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["//:noop_cycle1"]).await?;
    Ok(())
}

#[buck_test]
async fn test_tests_attribute_self_transition(buck: &Buck) {
    buck.test(&["//:noop_self_transition_references_fail"])
        .await
        .expect_failure()
        .assert_stderr_matches("Fail: root//:fail - unmanaged");
}
