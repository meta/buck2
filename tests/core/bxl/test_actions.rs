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
use serde_json::Value;
use serde_json::json;

#[buck_test]
async fn test_bxl_actions(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//artifact_test/artifacts.bxl:artifact_test"])
        .await?;

    // FIXME(JakobDegen): The first assert doesn't test anything the second doesn't cover
    assert!(
        result
            .stdout
            .contains("<source artifact artifact_test/TARGETS.fixture>")
    );
    assert!(
        result
            .stdout
            .contains("[<source artifact artifact_test/TARGETS.fixture>]")
    );
    Ok(())
}

#[buck_test]
async fn test_bxl_create_build_actions(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&[
            "//actions_test:actions.bxl:build_actions_test",
            "--",
            "--content",
            "my_content",
        ])
        .await?;
    assert_eq!(
        std::fs::read_to_string(buck.cwd().join(result.stdout.trim()))?,
        "my_content"
    );
    Ok(())
}

#[buck_test]
async fn test_bxl_create_build_actions_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&[
            "//actions_test:actions.bxl:build_actions_test",
            "--",
            "--content",
            "my_content",
            "--has_content_based_path",
            "true",
        ])
        .await?;

    assert_eq!(
        std::fs::read_to_string(buck.cwd().join(result.stdout.trim()))?,
        "my_content"
    );
    Ok(())
}

#[buck_test]
async fn test_resolve(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//resolve_test:resolve.bxl:resolve_test"])
        .await?;

    assert_eq!(result.stdout, "a-string\n");
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_bxl_declared_artifact_path(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//actions_test/declared_artifact_path.bxl:declared_artifact_path_test"])
        .await?;

    let output: Vec<&str> = result.stdout.lines().collect();
    // first line is result of get_path_without_materialization, second line is output of ctx.output.ensure
    assert_eq!(output[0], output[1]);
    Ok(())
}

#[buck_test]
async fn test_bxl_build_and_write(buck: &Buck) -> anyhow::Result<()> {
    // Performs a failed build and a successful action.
    let res = buck
        .bxl(&[
            "//actions_test:actions.bxl:build_and_write",
            "--",
            "--target",
            "actions_test:fail",
        ])
        .await?;

    assert_eq!(res.exit_code(), Some(0));
    assert!(res.stderr.contains("BXL SUCCEEDED"));
    Ok(())
}

#[buck_test]
async fn test_bxl_not_show_bxl_succeeded(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .command(&[
            "-v=status",
            "bxl",
            "//actions_test:actions.bxl:build_and_write",
            "--",
            "--target",
            "actions_test:fail",
        ])
        .await?;

    assert_eq!(res.exit_code(), Some(0));
    assert!(!res.stderr.contains("BXL SUCCEEDED"));
    Ok(())
}

#[buck_test]
async fn test_write_json_cell_path(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .bxl(&["//actions_test:actions.bxl:write_json_cell_path"])
        .await?;
    let output_path = res.stdout.trim();
    let content = std::fs::read_to_string(output_path)?;
    let data: Value = serde_json::from_str(&content)?;
    let expcted = json!({"root//resolve_test:buildable": ["root//resolve_test/foo.txt"]});
    assert_eq!(data, expcted);
    Ok(())
}

#[buck_test]
async fn test_ensure_unbound_artifact(buck: &Buck) {
    buck.bxl(&["//actions_test:ensure_unbound_artifact.bxl:ensure_unbound_artifact_test"])
        .await
        .expect_failure()
        .assert_stderr_matches("Artifact must be bound by now");
}
