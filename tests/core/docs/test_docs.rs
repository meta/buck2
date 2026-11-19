/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Tests to ensure that the `buck docs` command works as expected

use buck2_e2e::json::Json;
use buck2_e2e::prelude::*;

#[buck_test]
async fn test_docs_returns(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.docs(&["starlark"]).await?;
    let decoded: serde_json::Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(decoded, serde_json::json!([]));
    Ok(())
}

#[buck_test]
async fn test_prelude_docs(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.docs(&["starlark", "prelude//:prelude.bzl"]).await?;
    golden(
        &Json::loads(&result.stdout)?.dumps(Some(2), false),
        "prelude_docs.golden.json",
    );
    Ok(())
}

#[buck_test]
async fn test_docs_fail_with_invalid_patterns(buck: &Buck) {
    buck.docs(&["starlark", "not_an_import_path"])
        .await
        .expect_failure()
        .assert_stderr_matches("Import path must have suffix .*: `root//not_an_import_path`");
    buck.docs(&["starlark", "//cell"])
        .await
        .expect_failure()
        .assert_stderr_matches("Import path must have suffix .*: `root//cell`");
}
