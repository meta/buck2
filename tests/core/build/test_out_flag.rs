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

/// An existing file to pass to `--out`.
fn existing_file(tmp_path: &Path) -> anyhow::Result<String> {
    let path = tmp_path.join("out");
    std::fs::write(&path, "")?;
    Ok(path.to_str().unwrap().to_owned())
}

#[buck_test]
async fn test_out_single_default_output(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let output = tmp_path.join("output");
    buck.build(&["//:a", "--out", output.to_str().unwrap()])
        .await?;
    assert_eq!(std::fs::read_to_string(&output)?, "a\n");
    Ok(())
}

#[buck_test]
async fn test_out_overwrite(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let output = tmp_path.join("output");
    buck.build(&["//:a", "--out", output.to_str().unwrap()])
        .await?;
    buck.build(&["//:a", "--out", output.to_str().unwrap()])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_out_parent_not_exist(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let output = tmp_path.join("notexist").join("output");
    buck.build(&["//:a", "--out", output.to_str().unwrap()])
        .await?;
    assert_eq!(std::fs::read_to_string(&output)?, "a\n");
    Ok(())
}

#[buck_test]
async fn test_out_single_default_output_to_dir(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    buck.build(&["//:a", "--out", tmp_path.to_str().unwrap()])
        .await?;
    assert_eq!(std::fs::read_to_string(tmp_path.join("a.txt"))?, "a\n");
    Ok(())
}

#[buck_test]
async fn test_out_no_outputs(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let out = existing_file(tmp_path)?;
    buck.build(&["//:none", "--out", &out])
        .await
        .expect_failure()
        .assert_stderr_matches("produced zero default outputs");
    Ok(())
}

#[buck_test]
async fn test_out_multiple_outputs(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let out = existing_file(tmp_path)?;
    buck.build(&["//:ab", "--out", &out])
        .await
        .expect_failure()
        .assert_stderr_matches("produced 2 outputs");
    Ok(())
}

#[buck_test]
async fn test_out_multiple_targets(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let out = existing_file(tmp_path)?;
    buck.build(&["//:a", "//:b", "--out", &out])
        .await
        .expect_failure()
        .assert_stderr_matches("command built multiple top-level targets");
    Ok(())
}

#[buck_test]
async fn test_out_directory(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    buck.build(&["//:dir", "--out", tmp_path.to_str().unwrap()])
        .await?;
    assert!(tmp_path.join("b.txt").exists());
    assert!(tmp_path.join("nested_dir").join("a.txt").exists());
    Ok(())
}

#[buck_test]
async fn test_out_stdout_multiple(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//:a", "//:b", "--out", "-"]).await?;

    // The e2e test runner adds a `--build-report` flag in order to be able
    // to parse out failures. In normal usage of `--out -` there wouldn't be this
    // extra line of JSON on the stdout, we'd _just_ get the requested outputs.
    let parts: Vec<&str> = result.stdout.split('\n').collect();
    let [a, b, build_report, trailing] = parts.as_slice() else {
        anyhow::bail!("Expected two outputs and a build report, got {parts:?}");
    };
    assert!((*a, *b) == ("a", "b") || (*a, *b) == ("b", "a"));
    assert!(build_report.starts_with('{'));
    assert_eq!(*trailing, "");
    Ok(())
}

#[buck_test]
async fn test_out_stdout_none(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["--out", "-"]).await?;
    Ok(())
}

#[buck_test]
async fn test_out_stdout_directory(buck: &Buck) {
    buck.build(&["//:dir", "--out", "-"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "produces a default output that is a directory, and cannot be sent to stdout",
        );
}
