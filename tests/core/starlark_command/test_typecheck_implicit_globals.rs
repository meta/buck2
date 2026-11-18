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
async fn test_typecheck_build_file(buck: &Buck) -> anyhow::Result<()> {
    // Rule names in build files come from the implicit prelude import; rule
    // calls are typechecked against their attribute-derived signatures.
    buck.starlark(&["typecheck", "TARGETS.fixture"]).await?;
    let failure = buck
        .starlark(&["typecheck", "bad/TARGETS.fixture"])
        .await
        .expect_failure();
    golden(
        &failure.stdout,
        "fixtures/test_typecheck_build_file_bad.golden.txt",
    );
    Ok(())
}

#[buck_test]
async fn test_typecheck_implicit_prelude_symbols_in_bzl(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "implicit.bzl"]).await?;
    Ok(())
}
