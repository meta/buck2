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
use buck2_e2e::utils::replace_hash;
use indexmap::IndexMap;

#[buck_test]
async fn test_bxl_dynamic_action(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//:dynamic.bxl:dynamic_test"]).await?;
    let outputs = result.stdout.trim();
    assert_eq!(std::fs::read_to_string(outputs)?, "content");
    Ok(())
}

#[buck_test]
async fn test_bxl_dynamic_with_bxl_ctx(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//:dynamic.bxl:dynamic_test_with_bxl_ctx"])
        .await?;

    let outputs: IndexMap<String, String> = serde_json::from_str(&result.stdout)?;
    let golden_result = outputs
        .into_iter()
        .map(|(k, v)| Ok((k, replace_hash(&std::fs::read_to_string(v)?))))
        .collect::<anyhow::Result<IndexMap<_, _>>>()?;

    golden(
        &serde_json::to_string_pretty(&golden_result)?,
        "happy_path_dynamic_ctx.golden.json",
    );
    Ok(())
}

#[buck_test]
async fn test_bxl_dynamic_cannot_access_exec_deps(buck: &Buck) {
    buck.bxl(&["//:dynamic.bxl:dynamic_test_execution_resolution"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Anon targets and dynamic actions cannot access `bxl.Actions.exec_deps`",
        );
}

#[buck_test]
async fn test_bxl_dynamic_incompatible_targets(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//:dynamic.bxl:dynamic_test_incompatible_targets"])
        .await?;

    assert!(result.stderr.contains("Skipped 1 incompatible targets"));
    Ok(())
}
