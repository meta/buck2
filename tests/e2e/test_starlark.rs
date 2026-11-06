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

#[buck_test(inplace = true)]
async fn test_lint_buck2(buck: &Buck) -> anyhow::Result<()> {
    // The test data is wrong on purpose
    buck.starlark(&[
        "lint",
        "buck2",
        "-X",
        "buck2/tests/core",
        "-X",
        "buck2/tests/e2e",
    ])
    .await?;
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_typecheck_prelude_lightweight(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "buck2/prelude/prelude.bzl"])
        .await?;
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_typecheck_prelude_compiler(buck: &Buck) -> anyhow::Result<()> {
    buck.uquery(&["fbcode//buck2:buck2", "--unstable-typecheck"])
        .await?;
    Ok(())
}
