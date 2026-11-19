/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use buck2_e2e::BuckCommand;
use buck2_e2e::prelude::*;

/// The `buck2 <cmd> <args>` command.
fn command<'a>(buck: &'a Buck, cmd: &str, args: &[&str]) -> BuckCommand<'a> {
    match cmd {
        "build" => buck.build(args),
        "targets" => buck.targets(args),
        "cquery" => buck.cquery(args),
        "bxl" => buck.bxl(args),
        "uquery" => buck.uquery(args),
        _ => unreachable!("no `{cmd}` command"),
    }
}

async fn test_write_uuid(buck: &Buck, tmp_path: &Path, cmd: &str) -> anyhow::Result<()> {
    let file = tmp_path.join("build_id");
    std::fs::File::create(&file)?;
    command(
        buck,
        cmd,
        &["--write-build-id", file.to_str().unwrap(), "a"],
    )
    .await
    .expect_failure();

    assert!(!std::fs::read(&file)?.is_empty());
    Ok(())
}

#[buck_test]
async fn test_write_uuid_build(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    test_write_uuid(buck, tmp_path, "build").await
}

#[buck_test]
async fn test_write_uuid_targets(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    test_write_uuid(buck, tmp_path, "targets").await
}

#[buck_test]
async fn test_write_uuid_cquery(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    test_write_uuid(buck, tmp_path, "cquery").await
}

#[buck_test]
async fn test_write_uuid_bxl(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    test_write_uuid(buck, tmp_path, "bxl").await
}

#[buck_test]
async fn test_write_uuid_uquery(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    test_write_uuid(buck, tmp_path, "uquery").await
}

async fn test_ban_cell_override(buck: &Buck, cmd: &str) {
    command(buck, cmd, &["--config", "repositories.foo=bar", "a"])
        .await
        .expect_failure();
    command(buck, cmd, &["--config", "cells.foo=bar", "a"])
        .await
        .expect_failure();
}

#[buck_test]
async fn test_ban_cell_override_build(buck: &Buck) {
    test_ban_cell_override(buck, "build").await
}

#[buck_test]
async fn test_ban_cell_override_targets(buck: &Buck) {
    test_ban_cell_override(buck, "targets").await
}

#[buck_test]
async fn test_ban_cell_override_cquery(buck: &Buck) {
    test_ban_cell_override(buck, "cquery").await
}

#[buck_test]
async fn test_ban_cell_override_bxl(buck: &Buck) {
    test_ban_cell_override(buck, "bxl").await
}

#[buck_test]
async fn test_ban_cell_override_uquery(buck: &Buck) {
    test_ban_cell_override(buck, "uquery").await
}
