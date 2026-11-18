/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use buck2_e2e::prelude::*;

#[buck_test]
async fn test_chrome_trace(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // Just check it at least runs.
    buck.build(&["//..."]).await?;
    buck.debug(&[
        "chrome-trace",
        "--trace-path",
        &tmp_path.join("trace.json").to_string_lossy(),
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_chrome_trace_no_repo(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // Check that it runs from a path that is not in the repo.
    buck.build(&["//..."]).await?;
    let log_path = buck.log(&["last"]).await?.stdout.trim().to_owned();
    // The filesystem root, relative to the project.
    let root: PathBuf = buck
        .cwd()
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .map(|_| "..")
        .collect();
    buck.debug(&[
        "chrome-trace",
        "--trace-path",
        &tmp_path.join("trace.json").to_string_lossy(),
        "--path",
        &log_path,
    ])
    .rel_cwd(root)
    .await?;
    Ok(())
}
