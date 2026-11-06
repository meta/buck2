/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Test that when we render paths relative to the repo root, we prefix them with a
//! `./` to ensure the OS executes the cwd-relative path and doesn't do a $PATH
//! lookup for them.

use buck2_e2e::prelude::*;

#[buck_test]
async fn test_build_root_executable_local(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[":top", "--local-only"]).await?;
    Ok(())
}

#[buck_test]
async fn test_build_root_executable_remote(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[":top", "--remote-only"]).await?;
    Ok(())
}
