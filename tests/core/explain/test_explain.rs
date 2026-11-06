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

/// Whether `path` exists in the `buck2_logs` Manifold bucket, as the `manifold` CLI (which buck2
/// itself runs to download logs) reports it.
async fn manifold_exists(path: &str) -> anyhow::Result<bool> {
    let out = tokio::process::Command::new("manifold")
        .args([
            "--apikey",
            "buck2_logs-key",
            "exists",
            &format!("buck2_logs/{path}"),
        ])
        .output()
        .await
        .context("Running the `manifold` CLI")?;
    // The CLI exits with 0 for an existing path, 1 for a missing one and others for errors.
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => anyhow::bail!(
            "`manifold exists` failed ({}):\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}

fn output_path(tmp_path: &Path) -> String {
    tmp_path.join("index.html").to_string_lossy().into_owned()
}

#[buck_test]
async fn test_dummy_to_make_this_file_not_empty_on_windows(_buck: &Buck) {}

#[buck_test(skip_for_os = ["windows"])]
async fn test_explain(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let output = output_path(tmp_path);
    buck.build(&["//:simple"]).await?;
    let res = buck.explain(&["--output", &output]).await?;
    assert!(
        res.stderr
            .contains("Using last build invocation `buck2 build //:simple")
    );

    // check we wrote something
    assert!(std::fs::read(&output)?.len() >= 10);
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_explain_alias(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let output = output_path(tmp_path);
    buck.build(&["other_alias"]).await?;
    buck.explain(&["--output", &output]).await?;
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_explain_no_cell(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let output = output_path(tmp_path);
    buck.build(&[":simple"]).await?;
    buck.explain(&["--output", &output]).await?;
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_explain_universe(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let output = output_path(tmp_path);
    // check no universe fails for both
    buck.build(&["//:doesnt_exist"])
        .await
        .expect_failure()
        .assert_stderr_matches("Unknown target `doesnt_exist` from package");
    buck.explain(&["--output", &output])
        .await
        .expect_failure()
        .assert_stderr_matches("Unknown target `doesnt_exist` from package");

    // both don't fail with universe
    buck.build(&["//:doesnt_exist", "-u", ":simple"]).await?;
    buck.explain(&["--output", &output]).await?;
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_explain_only_builds(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let output = output_path(tmp_path);

    buck.uquery(&["//:simple"]).await?;
    buck.explain(&["--output", &output])
        .await
        .expect_failure()
        .assert_stderr_matches("No recent build commands found");

    buck.build(&["//:simple"]).await?;
    buck.explain(&["--output", &output]).await?;
    buck.explain(&["--output", &output]).await?;
    Ok(())
}

#[buck_test(
    skip_for_os = ["windows"],
    // 1 day
    env = [("BUCK2_TEST_MANIFOLD_TTL_S", "84000")],
)]
async fn test_explain_upload(buck: &Buck) -> anyhow::Result<()> {
    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    buck.build(&["//:simple"])
        .env("BUCK_WRAPPER_UUID", uuid)
        .await?;
    buck.explain(&[]).env("BUCK_WRAPPER_UUID", uuid).await?;

    assert!(manifold_exists(&format!("flat/{uuid}-explain.html")).await?);
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_explain_target_platform(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // no config fails
    buck.build(&[":foo"])
        .await
        .expect_failure()
        .assert_stderr_matches("Unknown target `foo`");

    // with config works and so does `explain`
    buck.build(&[":foo", "--config=test.config=foo"]).await?;
    let output = output_path(tmp_path);
    buck.explain(&["--output", &output]).await?;
    Ok(())
}
