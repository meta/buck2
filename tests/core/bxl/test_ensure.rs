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

use buck2_e2e::golden::sanitize_build_report;
use buck2_e2e::golden::sanitize_hashes;
use buck2_e2e::json::Json;
use buck2_e2e::prelude::*;
use serde_json::Value;

const BUCK_OUT_ROOT_REL_PATH: &str = "buck-out/v2/art/root";

fn test_bxl_artifact_path_cmd_args_helper(
    buck: &Buck,
    part_to_validate: &str,
    full_path: &Value,
    is_abs: bool,
) {
    let full_path = full_path.as_str().expect("a path");
    assert!(full_path.contains(BUCK_OUT_ROOT_REL_PATH));
    assert!(full_path.contains(part_to_validate));
    if is_abs {
        assert!(
            full_path.contains(
                buck.cwd()
                    .join(BUCK_OUT_ROOT_REL_PATH)
                    .to_string_lossy()
                    .as_ref()
            )
        );
        assert!(!Path::new(full_path).exists());
    } else {
        assert!(!full_path.contains(buck.cwd().to_string_lossy().as_ref()));
        assert!(!buck.cwd().join(full_path).exists());
    }
}

#[buck_test]
async fn test_bxl_ensure_no_materialization(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&[
            "//no_materialization_bxl_build/remote_text.bxl:ensure",
            "--materializations=none",
        ])
        .await?;

    let [output] = result.stdout.lines().collect::<Vec<_>>()[..] else {
        panic!("Expected one line, got:\n{}", result.stdout);
    };
    assert!(!buck.cwd().join(output).exists());

    let result = buck
        .bxl(&["//no_materialization_bxl_build/remote_text.bxl:ensure"])
        .await?;

    let [output] = result.stdout.lines().collect::<Vec<_>>()[..] else {
        panic!("Expected one line, got:\n{}", result.stdout);
    };
    assert!(buck.cwd().join(output).exists());
    Ok(())
}

#[buck_test]
async fn test_bxl_ensure(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&[
            "//ensure.bxl:ensure_build_result_test",
            "--",
            "--target",
            ":buildable",
        ])
        .await?;

    let outputs: Value = serde_json::from_str(&result.stdout)?;
    let buck_out = outputs
        .as_object()
        .expect("an object")
        .iter()
        .find(|(k, _)| k.starts_with("root//:buildable"))
        .map(|(_, v)| v)
        .expect("an output for `root//:buildable`");
    let [buck_out] = &buck_out.as_array().expect("a list of outputs")[..] else {
        panic!("Expected one output, got {buck_out}");
    };
    let buck_out = buck_out.as_str().expect("a path");
    assert_eq!(std::fs::read_to_string(buck.cwd().join(buck_out))?, "abcd");

    let result = buck.bxl(&["//ensure.bxl:ensure_cmd_line_test"]).await?;

    let mut lines: Vec<&str> = result.stdout.lines().collect();
    lines.sort();
    let read = |line: &str| std::fs::read_to_string(buck.cwd().join(line));
    assert_eq!(read(lines[0])?, "run_info_out");
    assert_eq!(read(lines[1])?, "target_with_tset\n");
    assert_eq!(read(lines[2])?, "tset1\n");
    assert_eq!(read(lines[3])?, "tset2\n");
    assert_eq!(read(lines[4])?, "tset3\n");

    let result = buck
        .bxl(&["//ensure.bxl:ensure_cmd_line_json_output"])
        .await?;

    let mut json_array: Vec<String> = serde_json::from_str(&result.stdout)?;
    json_array.sort();
    assert!(json_array[0].contains("target_with_tset"));
    assert!(json_array[1].contains("tset1"));
    assert!(json_array[2].contains("tset2"));
    assert!(json_array[3].contains("tset3"));
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_bxl_artifact_path(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//artifacts.bxl:artifact_path_test"]).await?;

    let outputs: Value = serde_json::from_str(&result.stdout)?;
    let output = |key: &str| outputs[key].as_str().expect("a string").to_owned();

    assert_eq!(
        output("source_artifact"),
        "<source artifact artifacts/DATA>"
    );
    // The project relative path of the source artifact
    assert_eq!(output("source_artifact_project_rel_path"), "artifacts/DATA");

    // Abs path for the source artifact. The path should exist on the filesystem.
    assert_eq!(
        output("source_artifact_abs_path"),
        buck.cwd().join("artifacts/DATA").to_string_lossy()
    );
    assert!(buck.cwd().join(output("source_artifact_abs_path")).exists());

    assert!(
        output("build_artifact")
            .contains("build artifact foo.txt bound to root//artifacts:with_build_artifact")
    );

    let prefix = format!("{BUCK_OUT_ROOT_REL_PATH}/");

    // The project relative path to the buck-out directory with the output
    assert!(output("build_artifact_project_rel_path").starts_with(&prefix));
    assert!(
        output("build_artifact_project_rel_path")
            .contains("/artifacts/__with_build_artifact__/foo.txt")
    );

    // Abs path for the build artifact. Path should not exist on the filesystem since it's not materialized.
    assert_eq!(
        output("build_artifact_abs_path"),
        buck.cwd()
            .join(output("build_artifact_project_rel_path"))
            .to_string_lossy()
    );

    assert!(!buck.cwd().join(output("build_artifact_abs_path")).exists());
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_bxl_artifact_path_cmd_args(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//artifacts.bxl:cmd_args_artifact_path_test"])
        .await?;

    let outputs: Value = serde_json::from_str(&result.stdout)?;
    test_bxl_artifact_path_cmd_args_helper(
        buck,
        "kind/__target_with_outputs__/run_info_out",
        &outputs["target_with_outputs_rel_paths"][0],
        false,
    );

    test_bxl_artifact_path_cmd_args_helper(
        buck,
        "kind/__target_with_outputs__/run_info_out",
        &outputs["target_with_outputs_abs_paths"][0],
        true,
    );

    assert_eq!(
        outputs["target_with_tset_rel_paths"]
            .as_array()
            .map(Vec::len),
        Some(4)
    );

    test_bxl_artifact_path_cmd_args_helper(
        buck,
        "kind/__target_with_tset__/out.txt",
        &outputs["target_with_tset_rel_paths"][0],
        false,
    );
    test_bxl_artifact_path_cmd_args_helper(
        buck,
        "kind/__tset1__/out.txt",
        &outputs["target_with_tset_rel_paths"][1],
        false,
    );
    test_bxl_artifact_path_cmd_args_helper(
        buck,
        "kind/__tset2__/out.txt",
        &outputs["target_with_tset_rel_paths"][2],
        false,
    );
    test_bxl_artifact_path_cmd_args_helper(
        buck,
        "kind/__tset3__/out.txt",
        &outputs["target_with_tset_rel_paths"][3],
        false,
    );

    assert_eq!(
        outputs["target_with_tset_abs_paths"]
            .as_array()
            .map(Vec::len),
        Some(4)
    );

    test_bxl_artifact_path_cmd_args_helper(
        buck,
        "kind/__target_with_tset__/out.txt",
        &outputs["target_with_tset_abs_paths"][0],
        true,
    );
    test_bxl_artifact_path_cmd_args_helper(
        buck,
        "kind/__tset1__/out.txt",
        &outputs["target_with_tset_abs_paths"][1],
        true,
    );
    test_bxl_artifact_path_cmd_args_helper(
        buck,
        "kind/__tset2__/out.txt",
        &outputs["target_with_tset_abs_paths"][2],
        true,
    );
    test_bxl_artifact_path_cmd_args_helper(
        buck,
        "kind/__tset3__/out.txt",
        &outputs["target_with_tset_abs_paths"][3],
        true,
    );
    Ok(())
}

/// Test that BXL fails when trying to ensure a failed build artifact.
#[buck_test(allow_soft_errors = true, skip_for_os = ["darwin", "windows"])]
async fn test_bxl_ensure_failures(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");

    buck.bxl(&[
        "//ensure.bxl:ensure_failures",
        "--build-report",
        &report.to_string_lossy(),
    ])
    .await
    .expect_failure();

    let mut build_report: Value = serde_json::from_str(&std::fs::read_to_string(&report)?)?;

    sanitize_build_report(&mut build_report);

    golden(
        &sanitize_hashes(&Json::from(build_report.clone()).dumps(Some(2), true)),
        "fixtures/test_bxl_ensure_failures.golden.json",
    );
    Ok(())
}

#[buck_test]
async fn test_bxl_print_json_ensured_in_set(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//artifacts.bxl:print_json_set_test"]).await?;
    let lines: Vec<&str> = result.stdout.trim().lines().collect();
    assert_eq!(lines.len(), 2);
    let first: Value = serde_json::from_str(lines[0])?;
    let second: Value = serde_json::from_str(lines[1])?;
    assert!(
        first[0]
            .as_str()
            .is_some_and(|p| p.ends_with("artifacts/DATA")),
        "{}",
        lines[0]
    );
    assert_eq!(second, first, "{}", lines[1]);
    Ok(())
}

#[buck_test]
async fn test_bxl_ensured_group_paths_keep_input_order(buck: &Buck) -> anyhow::Result<()> {
    // The paths of `cmd_args(a, b)` come out as `a b`, the order the command line uses.
    let result = buck.bxl(&["//artifacts.bxl:group_order_test"]).await?;
    let lines = result
        .stdout
        .trim()
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<Vec<String>>, _>>()?;
    assert_eq!(lines.len(), 2);
    for paths in lines {
        let basenames: Vec<&str> = paths
            .iter()
            // As `os.path.basename`, which splits on either separator on Windows.
            .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p))
            .collect();
        assert_eq!(basenames, ["DATA", "rules.bzl"]);
    }
    Ok(())
}
