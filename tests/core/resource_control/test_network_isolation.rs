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

/// Verify that network_access='none' blocks network in a new network namespace.
#[buck_test(skip_for_os = ["windows", "darwin"], disable_daemon_cgroup = false)]
async fn test_network_isolated(buck: &Buck) {
    // The test command tries to bind to localhost. With network isolation the
    // loopback interface is DOWN, so the bind fails and the test exits non-zero.
    buck.test(&[
        "root//:network_isolated",
        "--local-only",
        "--no-remote-cache",
    ])
    .await
    .expect_failure();
}

/// Control: verify network works when network_access is not set.
#[buck_test(skip_for_os = ["windows", "darwin"], disable_daemon_cgroup = false)]
async fn test_network_accessible(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&[
        "root//:network_accessible",
        "--local-only",
        "--no-remote-cache",
    ])
    .await?;
    Ok(())
}

/// Isolation applies even when daemon cgroups are off.
#[buck_test(skip_for_os = ["windows", "darwin"])]
async fn test_network_isolated_without_cgroups(buck: &Buck) {
    // disable_daemon_cgroup defaults to True — no daemon-level cgroups.
    buck.test(&[
        "root//:network_isolated",
        "--local-only",
        "--no-remote-cache",
    ])
    .await
    .expect_failure();
}

#[buck_test]
async fn test_nop(_buck: &Buck) {
    // Pytest gets upset if we have no windows or mac tests in this file
}
