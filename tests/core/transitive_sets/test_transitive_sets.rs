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
async fn test_transitive_sets(buck: &Buck) -> anyhow::Result<()> {
    let rule = "//:bar";
    let report = buck.build(&[rule]).await?;
    let out = std::fs::read_to_string(report.build_report()?.output_for_target(rule)?)?;
    let out: Vec<&str> = out.trim().split('\n').map(str::trim).collect();
    assert_eq!(out, ["bar", "foo", "foo2", "foo1"]);
    Ok(())
}

#[buck_test]
async fn test_transitive_set_deduplication(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:test_duplication"]).await?;
    Ok(())
}

#[buck_test]
async fn test_project_as_args_with_optional_args(buck: &Buck) {
    let rule = "//:combined_optional_args";

    // TODO(ianc) Allow project_as_args to skip nodes that return None
    buck.build(&[rule]).await.expect_failure().assert_stderr_matches(
        r"error: Expected `Artifact \| CellPath \| CellRoot \| Label \| OutputArtifact \| ProjectRoot \| ResolvedStringWithMacros \| TaggedCommandLine \| TargetLabel \| TransitiveSetArgsProjection \| WriteJsonCliArgs \| cmd_args \| str \| RunInfo`, but got `NoneType \(repr: None\)`",
    );
}

#[buck_test]
async fn test_project_as_json_with_optional_args(buck: &Buck) -> anyhow::Result<()> {
    let rule = "//:combined_optional_json_args";

    let report = buck.build(&[rule]).await?;
    let out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        report.build_report()?.output_for_target(rule)?,
    )?)?;

    // TODO(ianc) remove the None
    assert_eq!(
        out,
        serde_json::json!(["combined_optional_json_args", "optional_json_args1", null])
    );
    Ok(())
}
