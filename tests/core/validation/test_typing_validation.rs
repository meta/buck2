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
async fn test_typing_validation_fails_on_type_errors(buck: &Buck) {
    buck.build(&[":bad_types_validated"])
        .await
        .expect_failure()
        .assert_stderr_matches("Validation for .+ failed.*is not assignable to parameter");
}

#[buck_test]
async fn test_typing_validation_passes_on_clean_types(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[":good_types_validated"]).await?;
    Ok(())
}

#[buck_test]
async fn test_typing_validation_off_ignores_errors(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[":bad_types_no_validation"]).await?;
    Ok(())
}

#[buck_test]
async fn test_typing_disabled_skips_validation(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[":typing_disabled"]).await?;
    Ok(())
}
