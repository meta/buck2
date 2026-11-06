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

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_eval(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:lazy_cquery.bxl:eval_query_test"]).await?;

    assert!(result.stdout.contains("TARGETS.fixture"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_eval_query_catch_error(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl:lazy_cquery.bxl:eval_query_catch_error_test"])
        .await?;
    Ok(())
}
