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
async fn test_construction_validation_good(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&["//good:"]).await?;
    Ok(())
}

#[buck_test]
async fn test_construction_validation_bad(buck: &Buck) {
    buck.targets(&["//bad:"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"`impl` function signature is incorrect");
}

#[buck_test]
async fn test_construction_validation_bad_param_types(buck: &Buck) {
    buck.targets(&["//bad_param_types:"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"`impl` function signature is incorrect");
}

#[buck_test]
async fn test_construction_validation_bad_param_types_vnew(buck: &Buck) -> anyhow::Result<()> {
    // FIXME(JakobDegen): Evaluate whether we can implement this. The performance
    // concerns are a bit higher here because the code is hotter.
    buck.build(&["//bad_param_types_vnew:"]).await?;
    Ok(())
}

#[buck_test]
async fn test_construction_validation_bad_return_type(buck: &Buck) {
    buck.targets(&["//bad_return_type:"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"`impl` function signature is incorrect");
}
