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
async fn test_define_anon_bxl(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//anon_bxl.bxl:define_anon"]).await?;
    Ok(())
}

#[buck_test]
async fn test_define_wrong_type_anon_bxl(buck: &Buck) {
    buck.bxl(&["//wrong_type_anon_bxl.bxl:wrong_type"])
        .await
        .expect_failure()
        .assert_stderr_matches("Callable of type .* does not match the expected signature");
}

#[buck_test]
async fn test_eval_anon_bxl(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//anon_bxl.bxl:eval_anon_bxl"]).await?;
    Ok(())
}

#[buck_test]
async fn test_check_anon_ouput_artifact(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//anon_bxl.bxl:check_anon_ouput_artifact"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_pass_string_to_arg_attr(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//anon_bxl.bxl:eval_of_anon_with_arg_bxl"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_content_based_output(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//anon_bxl.bxl:eval_of_anon_with_content_based_output_impl"])
        .await?;

    let output_path = std::fs::canonicalize(buck.cwd().join(result.stdout.trim()))?;
    assert_eq!(std::fs::read_to_string(output_path)?, "hello world");
    Ok(())
}
