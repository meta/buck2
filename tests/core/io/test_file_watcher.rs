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

#[buck_test(
    setup_eden = false,
    extra_buck_config = [("buck2", "file_watcher", "edenfs")],
    env = [("BUCK2_HARD_ERROR", "false")],
)]
async fn test_watchman_fallback(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.targets(&["root//:"]).await?;
    // fallback to watchman
    assert!(
        res.stderr.contains("Watchman fresh instance"),
        "{}",
        res.stderr
    );
    Ok(())
}

#[buck_test(
    setup_eden = false,
    extra_buck_config = [("buck2", "file_watcher", "edenfs")],
)]
async fn test_eden_fail(buck: &Buck) {
    let res = buck.targets(&["root//:"]).await.expect_failure();
    assert!(
        res.stderr.contains("Couldn't initiate connection to Eden"),
        "{}",
        res.stderr
    );
}
