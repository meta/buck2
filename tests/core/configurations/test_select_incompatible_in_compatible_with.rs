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
async fn test_select_incompatible_in_ordinary_attribute_is_incompatible(buck: &Buck) {
    // Requested explicitly, an incompatible target is an error that names the cause.
    buck.build(&["//:attr_select_incompatible"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"is incompatible: select resolved to select_incompatible\(\): not for a",
        );
}

#[buck_test]
async fn test_select_incompatible_in_target_compatible_with_is_skipped(
    buck: &Buck,
) -> anyhow::Result<()> {
    // In `target_compatible_with` the `select_incompatible()` branch makes the target
    // incompatible, like in any other attribute: a package pattern skips it.
    let result = buck.build(&["//:"]).await?;
    assert!(result.stderr.contains("Skipped 3 incompatible targets:"));
    assert!(result.stderr.contains("root//:tcw_select_incompatible ("));
    buck.build(&["//:tcw_select_incompatible"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"select resolved to select_incompatible\(\): not for a");
    Ok(())
}
