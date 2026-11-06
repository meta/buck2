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

async fn check_io_provider(buck: &Buck, name: &str) -> anyhow::Result<()> {
    buck.server(&[]).await?;
    let out = buck.status(&[]).await?;
    let status: serde_json::Value = serde_json::from_str(out.stdout.trim())?;
    let io_provider = &status["io_provider"];
    assert_eq!(io_provider, name);
    Ok(())
}

#[buck_test(
    setup_eden = false,
    extra_buck_config = [("buck2", "allow_eden_io", "false")],
)]
async fn test_no_eden(buck: &Buck) -> anyhow::Result<()> {
    check_io_provider(buck, "fs").await
}

#[buck_test(
    setup_eden = false,
    extra_buck_config = [("buck2", "allow_eden_io", "true")],
)]
async fn test_allow_eden_io_ignored_on_fs_io(buck: &Buck) -> anyhow::Result<()> {
    check_io_provider(buck, "fs").await
}

#[buck_test(
    setup_eden = true,
    extra_buck_config = [("buck2", "allow_eden_io", "false")],
)]
async fn test_allow_eden_io_respected(buck: &Buck) -> anyhow::Result<()> {
    check_io_provider(buck, "fs").await
}

#[buck_test(
    setup_eden = true,
    extra_buck_config = [("buck2", "allow_eden_io", "true")],
)]
async fn test_eden_io(buck: &Buck) -> anyhow::Result<()> {
    check_io_provider(buck, "eden").await
}
