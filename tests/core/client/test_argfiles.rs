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
async fn test_argfile_with_cell(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .audit_config(&["@cell1//argfile", "--cell", "root", "foo.bar"])
        .await?;
    assert!(res.stdout.contains("bar = 1"));
    Ok(())
}

#[buck_test]
async fn test_argfile_from_cwd_cell(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .audit_config(&["@//argfile", "--cell", "root", "foo.bar"])
        .rel_cwd("cell1")
        .await?;
    assert!(res.stdout.contains("bar = 1"));
    Ok(())
}

#[buck_test]
async fn test_executable_argfile(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .audit_config(&[
            "@//exec_argfile.py#iphonesimulator-x86_64",
            "--cell",
            "root",
            "foo.bar",
        ])
        .await?;
    assert!(res.stdout.contains("bar = 1"));
    Ok(())
}

#[buck_test]
async fn test_stdin_argfile(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .audit_config(&["@-", "--cell", "root", "foo.bar"])
        .stdin("--config=foo.bar=1")
        .await?;
    assert!(res.stdout.contains("bar = 1"));
    Ok(())
}
