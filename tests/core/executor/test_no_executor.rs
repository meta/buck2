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
async fn test_no_executor(buck: &Buck) -> anyhow::Result<()> {
    buck.cquery(&["root//:run_action"]).await?;

    buck.build(&["root//:run_action"])
        .await
        .expect_failure()
        .assert_stderr_matches(".*incompatible with the executor config.*");
    Ok(())
}
