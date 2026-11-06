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
async fn test_cquery_ctx(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/check_type_names_and_symbols.bxl:cquery_ctx"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_target_set(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/check_type_names_and_symbols.bxl:target_set"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_target_set_syntax(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/check_type_names_and_symbols.bxl:target_set_syntax"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_target_set_unsupported_node_type(buck: &Buck) {
    let result = buck
        .bxl(&["//bxl/check_type_names_and_symbols.bxl:target_set_unsupported_node_type"])
        .await
        .expect_failure();
    assert!(result.stderr.contains("`bxl.TargetSet` expects one of"));
    assert!(result.stderr.contains("got `int`"));
}
