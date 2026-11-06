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
async fn test_bxl_audit_output(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//audit.bxl:audit_output_action_exists"])
        .await?;

    buck.bxl(&["//audit.bxl:audit_output_config_not_match"])
        .await?;

    buck.bxl(&["//audit.bxl:audit_output_invalid_path"])
        .await
        .expect_failure()
        .assert_stderr_matches("Malformed buck-out path");
    Ok(())
}

#[buck_test]
async fn test_bxl_audit_content_based_output(buck: &Buck) -> anyhow::Result<()> {
    let label = "root//:with_content_based_output";
    let result = buck.build(&[label, "--show-output"]).await?;
    let path = result.build_report()?.output_for_target(label)?;

    // resolve the symlink that we get as the output from buck to find the underlying content-based path.
    let path = std::fs::canonicalize(buck.cwd().join(path))?;
    // make it a relative path again (to the canonical project directory: on Windows `canonicalize`
    // gives a verbatim `\\?\` path)
    let path = path.strip_prefix(std::fs::canonicalize(buck.cwd())?)?;

    buck.bxl(&[
        "//audit.bxl:audit_content_based_output_action_exists",
        "--",
        "--label",
        label,
        "--path",
        &path.to_string_lossy().replace('\\', "/"),
    ])
    .await?;
    Ok(())
}
