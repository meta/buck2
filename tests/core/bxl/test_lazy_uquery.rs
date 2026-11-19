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
async fn test_testsof(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:lazy_uquery.bxl:tests_of"]).await?;
    Ok(())
}

#[buck_test]
async fn test_testsof_fail(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:lazy_uquery.bxl:tests_of_fail"]).await?;
    Ok(())
}

#[buck_test]
async fn test_catch_eval_conversion_error(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:lazy_uquery.bxl:eval_conversion_error"])
        .await?;
    Ok(())
}
