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

use buck2_e2e::prelude::*;

/// Checks that the file is not executable by its owner. Windows doesn't have the permission.
#[cfg(unix)]
fn assert_not_executable(output: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(output)?.permissions().mode();
    assert_eq!(mode & 0o100, 0, "`{}` is executable", output.display());
    Ok(())
}

#[cfg(not(unix))]
fn assert_not_executable(_output: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[buck_test]
async fn test_build_nested_subtargets(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//:nested[sub][nested_sub]"]).await?;
    let build_report = result.build_report()?;

    let outputs = build_report.outputs_for_target("//:nested", "sub|nested_sub")?;
    let [output] = outputs.as_slice() else {
        anyhow::bail!("Expected one output for `//:nested`, got {outputs:?}");
    };

    assert_eq!(std::fs::read_to_string(output)?.trim_end(), "foo_content");
    assert_not_executable(output)?;
    Ok(())
}

#[buck_test]
async fn test_build_nested_subtargets_errors(buck: &Buck) {
    buck.build(&["//:nested[bad]"])
        .await
        .expect_failure()
        .assert_stderr_matches("Available subtargets are.*sub");

    buck.build(&["//:nested[sub][bad]"])
        .await
        .expect_failure()
        .assert_stderr_matches("Available subtargets are.*nested_sub");
}
