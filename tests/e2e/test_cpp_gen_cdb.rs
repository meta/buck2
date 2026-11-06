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

use anyhow::Context;
use buck2_e2e::prelude::*;
use serde_json::Value;

/// The host's name as Python's `platform.system().lower()` gives it.
fn platform_system() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    }
}

// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_no_quotes(buck: &Buck) -> anyhow::Result<()> {
    let filename = buck
        .cwd()
        .parent()
        .context("The cell root has a parent")?
        .join("fbcode/buck2/tests/targets/cpp_gen_cdb/basic/src/main.cpp");
    let result = buck
        .bxl(&[
            "fbcode//tools/build/buck/bxl/cpp_lsp/cpp_gen_cdb.bxl:cpp_gen_cdb",
            "--",
            "--filename",
            &filename.to_string_lossy(),
            "--os",
            platform_system(),
            "--exec_mode",
            "local",
        ])
        .await?;
    let outputs: Value = serde_json::from_str(&result.stdout)?;
    let compdb_path = Path::new(
        outputs
            .get("compilationDatabasePath")
            .and_then(Value::as_str)
            .context("No `compilationDatabasePath` in the output")?,
    )
    .join("..")
    .join("compdb.json");

    let commands: Value = serde_json::from_str(&std::fs::read_to_string(&compdb_path)?)?;

    // check that the define is present without any shell quotes
    let arguments = commands
        .get(0)
        .and_then(|command| command.get("arguments"))
        .and_then(Value::as_array)
        .context("No `arguments` in the first command")?;
    let index = arguments
        .iter()
        .position(|a| a == "-DM_FOO_BAR=1")
        .with_context(|| format!("No `-DM_FOO_BAR=1` in {arguments:?}"))?;
    assert_ne!(index, 0, "`-DM_FOO_BAR=1` is not the compiler");
    Ok(())
}

// TODO(marwhal): Add this back one at least one test in this file passes on Windows
#[buck_test(inplace = true)]
async fn test_noop(_buck: &Buck) {}
