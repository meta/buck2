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
use buck2_e2e::approx::approx;
use buck2_e2e::json::Json;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::replace_digest;
use buck2_e2e::utils::replace_hash;
use serde_json::Value;

/// Replace timing-dependent fields with a placeholder to avoid flaky tests.
fn sanitize_timing_fields(obj: &mut Value) {
    match obj {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if key == "compute_time_ms" {
                    *value = Value::from("<COMPUTE_TIME_MS>");
                } else {
                    sanitize_timing_fields(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(sanitize_timing_fields),
        _ => {}
    }
}

async fn build_report_test(
    buck: &Buck,
    tmp_path: &Path,
    name: &str,
    command: &[&str],
) -> anyhow::Result<()> {
    let report = tmp_path.join("build-report.json");
    let mut args = vec!["--build-report", report.to_str().unwrap()];
    args.extend(command);
    buck.build(&args).await?;

    let mut report: Value = serde_json::from_str(&std::fs::read_to_string(&report)?)?;
    let fields = report
        .as_object_mut()
        .context("The build report is not an object")?;
    fields.remove("trace_id").context("No `trace_id`")?;
    fields.remove("project_root").context("No `project_root`")?;
    sanitize_timing_fields(&mut report);

    golden(
        &replace_digest(&replace_hash(
            &Json::from(report.clone()).dumps(Some(2), true),
        )),
        &format!("fixtures/{name}.golden.json"),
    );
    Ok(())
}

async fn get_sketch_cardinalities_from_report(
    buck: &Buck,
    target: &str,
) -> anyhow::Result<(Value, Value)> {
    let report_path = buck.cwd().join("build-report.json");
    buck.build(&[
        target,
        "--build-report",
        report_path.to_str().unwrap(),
        "-c",
        "buck2.log_artifact_count_sketch=true",
        "-c",
        "buck2.log_artifact_size_sketch=true",
        "-c",
        "buck2.log_sketch_cardinalities=true",
    ])
    .await?;

    let report: Value = serde_json::from_str(&std::fs::read_to_string(&report_path)?)?;
    let entry = report["results"][target]["configured"]
        .as_object()
        .and_then(|configured| configured.values().next())
        .with_context(|| format!("No configuration of `{target}` in the build report"))?;
    Ok((
        entry["artifact_count_sketch_cardinality"].clone(),
        entry["artifact_size_sketch_cardinality"].clone(),
    ))
}

#[buck_test]
async fn test_artifact_count_sketch_format(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_artifact_count_sketch_format",
        &["//:simple", "-c", "buck2.log_artifact_count_sketch=true"],
    )
    .await
}

#[buck_test]
async fn test_artifact_size_sketch_format(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_artifact_size_sketch_format",
        &["//:simple", "-c", "buck2.log_artifact_size_sketch=true"],
    )
    .await
}

#[buck_test]
async fn test_both_artifact_sketches_format(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    build_report_test(
        buck,
        tmp_path,
        "test_both_artifact_sketches_format",
        &[
            "//:simple",
            "//:with_dep",
            "-c",
            "buck2.log_artifact_count_sketch=true",
            "-c",
            "buck2.log_artifact_size_sketch=true",
        ],
    )
    .await
}

#[buck_test]
async fn test_estimated_single_target(buck: &Buck) -> anyhow::Result<()> {
    let (count, size) = get_sketch_cardinalities_from_report(buck, "root//:simple").await?;
    // simple_write: 1 immediate provider output (out.txt). Distinct paths = 1.
    assert!(!count.is_null());
    assert_eq!(count, approx(1.0).abs(0.5));
    // Size sketch estimates total bytes: "content" = 7 bytes
    assert!(!size.is_null());
    assert_eq!(size, approx(7.0).abs(1.5));
    Ok(())
}

#[buck_test]
async fn test_estimated_with_dep(buck: &Buck) -> anyhow::Result<()> {
    let (count, size) = get_sketch_cardinalities_from_report(buck, "root//:with_dep").await?;
    // copy_dep target has 1 immediate provider output (out.txt). The base dep
    // is intentionally not counted because we only sketch immediate provider
    // outputs, not the action graph.
    // Distinct paths: {with_dep_out} = 1
    assert!(!count.is_null());
    assert_eq!(count, approx(1.0).abs(0.5));
    // Size: with_dep's out.txt is a copy of base's out.txt = "content" (7 bytes)
    assert!(!size.is_null());
    assert_eq!(size, approx(7.0).abs(1.5));
    Ok(())
}

#[buck_test]
async fn test_estimated_content_based_paths(buck: &Buck) -> anyhow::Result<()> {
    let (count, size) = get_sketch_cardinalities_from_report(buck, "root//:content_based").await?;
    // content_based_write: 1 output resolved via resolve_path with the content
    // hash from the ArtifactValue, so the sketched path is the real on-disk
    // content-hashed path. The ArtifactValue has no extra deps for a single-file
    // content-based write.
    // Distinct paths: 1
    assert!(!count.is_null());
    assert_eq!(count, approx(1.0).abs(0.5));
    // Size: "content" = 7 bytes
    assert!(!size.is_null());
    assert_eq!(size, approx(7.0).abs(1.5));
    Ok(())
}

#[buck_test]
async fn test_estimated_projected_artifacts(buck: &Buck) -> anyhow::Result<()> {
    let (count, size) =
        get_sketch_cardinalities_from_report(buck, "root//:projected_target").await?;
    // projected_output rule: default output is out_dir.project("a"). The
    // projected artifact resolves to a single sub-path (out_dir/a).
    // Distinct paths: {out_dir/a} = 1
    assert!(!count.is_null());
    assert_eq!(count, approx(1.0).abs(0.5));
    // Size: out_dir/a contains "a\n" from `echo a` = 2 bytes
    assert!(!size.is_null());
    assert_eq!(size, approx(2.0).abs(1.5));
    Ok(())
}

#[buck_test]
async fn test_estimated_symlink_file(buck: &Buck) -> anyhow::Result<()> {
    let (count, size) = get_sketch_cardinalities_from_report(buck, "root//:symlink_target").await?;
    // symlink_rule: immediate provider output is link.txt (a Symlink leaf entry).
    // The ArtifactValue's `deps` tree carries the symlink target (src.txt) at
    // its project-relative path because `add_symlinked` merged the source value
    // in at action-construction time.
    // Distinct paths: {link.txt, src.txt} = 2
    assert!(!count.is_null());
    assert_eq!(count, approx(2.0).abs(0.5));
    // Size: src.txt contents ("source_content" = 14 bytes) + link.txt symlink
    // target string (a content-based path; ~27 bytes).
    assert!(!size.is_null());
    assert_eq!(size, approx(41.0).abs(5.0));
    Ok(())
}

#[buck_test]
async fn test_estimated_symlinked_dir(buck: &Buck) -> anyhow::Result<()> {
    let (count, size) =
        get_sketch_cardinalities_from_report(buck, "root//:symlinked_dir_target").await?;
    // symlinked_dir_rule: immediate provider output is out_dir, a directory
    // whose entries are symlinks. Walking the entry yields out_dir/file1 and
    // out_dir/file2. The ArtifactValue's `deps` tree carries the underlying
    // source artifacts (src1.txt, src2.txt) because each symlink contributes
    // its source value's deps.
    // Distinct paths: {out_dir/file1, out_dir/file2, src1.txt, src2.txt} = 4
    assert!(!count.is_null());
    assert_eq!(count, approx(4.0).abs(0.5));
    // Size: src1.txt + src2.txt ("content1"/"content2" = 8 bytes each) + two
    // content-based symlink target strings (~32 bytes each).
    assert!(!size.is_null());
    assert_eq!(size, approx(80.0).abs(6.0));
    Ok(())
}
