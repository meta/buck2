/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::process::Stdio;

use anyhow::Context;
use buck2_e2e::prelude::*;
use serde_json::Value;

#[buck_test(inplace = true)]
async fn test_rust_binary(buck: &Buck) -> anyhow::Result<()> {
    // The test's own `RUST_PROJECT_BIN` is relative to the project root, which `current_dir`
    // below leaves; the fixture's copy is absolute.
    let rust_project_bin = buck
        .env_var("RUST_PROJECT_BIN")
        .context("The test target sets `RUST_PROJECT_BIN`")?;

    let result = tokio::process::Command::new(rust_project_bin)
        .args([
            "develop",
            "--stdout",
            "--pretty",
            "fbcode//buck2/tests/targets/rules/rust/hello_world:welcome",
        ])
        .env("BUCK2_HARD_ERROR", "false")
        .current_dir(buck.cwd())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .await?;

    let json_generated: Value = serde_json::from_slice(&result.stdout)?;

    assert!(json_generated.get("sysroot").is_some());
    assert!(json_generated.get("sysroot_src").is_some());
    assert!(json_generated.get("crates").is_some());
    Ok(())
}
