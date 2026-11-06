/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Write;
use std::path::Path;

use buck2_e2e::prelude::*;
use serde_json::Value;
use serde_json::json;

#[buck_test]
async fn test_log_show_invocation_record(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let mode_file = tmp_path.join("mode");
    std::fs::write(&mode_file, "-c\naa.bb=cc\n-c\ndd.ee=ff\n")?;
    let mode_arg = format!("@{}", mode_file.display());

    // Any simple would do.
    buck.uquery(&[&mode_arg, "//:EEE"]).await?;

    let result = buck.log(&["show"]).await?;
    let invocation: Value = serde_json::from_str(result.stdout.lines().next().unwrap())?;
    let command_line_args = invocation["command_line_args"].as_array().unwrap();
    let expanded_command_line_args = invocation["expanded_command_line_args"].as_array().unwrap();
    assert!(command_line_args.contains(&json!(mode_arg)));
    assert!(!expanded_command_line_args.contains(&json!(mode_arg)));
    assert!(expanded_command_line_args.contains(&json!("aa.bb=cc")));
    assert!(!command_line_args.contains(&json!("aa.bb=cc")));
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_log_size_logging(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.cquery(&["//:EEE"]).await?;

    let out = buck.log(&["last"]).await?;
    let path = out.stdout.trim();
    let log_size_in_disk = std::fs::read(path)?.len();

    let logged_size = &res.invocation_record()?["compressed_event_log_size_bytes"];

    assert_eq!(*logged_size, json!(log_size_in_disk));
    Ok(())
}

#[buck_test]
async fn test_last_log(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:EEE"]).await?;
    let out = buck.log(&["last"]).await?;
    let path = out.stdout.trim();
    assert!(Path::new(path).exists());
    assert!(path.contains("/log/") || path.contains("\\log\\"));
    let out2 = buck.log(&["path"]).await?;
    assert_eq!(path, out2.stdout.trim());
    Ok(())
}

#[buck_test]
async fn test_last_log_all(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:EEE"]).await?;
    let out = buck.log(&["last", "--all"]).await?;
    let paths: Vec<&str> = out.stdout.lines().collect();
    assert!(!paths.is_empty());
    for path in paths {
        assert!(Path::new(path).exists());
        assert!(path.contains("/log/") || path.contains("\\log\\"));
    }
    Ok(())
}

#[buck_test]
async fn test_log_command_with_trace_id(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let build_file_path = tmp_path.join("b");
    buck.uquery(&[
        "//:",
        &format!("--write-build-id={}", build_file_path.display()),
    ])
    .await?;
    let build_id = std::fs::read_to_string(&build_file_path)?.trim().to_owned();
    let trace_id_arg = format!("--trace-id={build_id}");
    buck.log(&["show", &trace_id_arg]).await?;
    let log = buck.log(&["show", &trace_id_arg]).await?.stdout;
    let log: Vec<&str> = log.trim().lines().collect();
    // Check it looks like log.
    assert!(!log.is_empty());
    for line in log {
        serde_json::from_str::<Value>(line)?;
    }
    Ok(())
}

#[buck_test]
async fn test_what_buck(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let mode_path = tmp_path.join("mode");
    std::fs::write(&mode_path, "-c\nxx.yy=zz\n")?;

    buck.uquery(&["//:", &format!("@{}", mode_path.display())])
        .await?;

    let out = buck.log(&["what-cmd"]).await?;
    assert!(out.stdout.contains("uquery //: "));
    if !cfg!(windows) {
        // Path is quoted on Windows.
        assert!(
            out.stdout
                .contains(&format!("uquery //: @{}", mode_path.display()))
        );
    }

    let out = buck.log(&["what-cmd", "--expand"]).await?;
    assert!(out.stdout.contains("uquery //: -c"));
    Ok(())
}

async fn test_log_summary_downloads(
    buck: &Buck,
    tmp_path: &Path,
    snapshots: &[(u64, u64)],
    expected_re: u64,
    expected_http: u64,
) -> anyhow::Result<()> {
    buck.uquery(&["//:EEE"]).await?;
    let log = buck
        .log(&["show"])
        .await?
        .stdout
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<Value>, _>>()?;
    let mut snapshot_event = log[1..]
        .iter()
        .find(|event| {
            event["Event"]["data"]["Instant"]["data"]
                .get("Snapshot")
                .is_some()
        })
        .unwrap()
        .clone();
    let log_path = tmp_path.join("summary.json-lines");
    let mut output = String::new();
    writeln!(output, "{}", serde_json::to_string(&log[0])?)?;
    for (index, (re_bytes, http_bytes)) in snapshots.iter().enumerate() {
        snapshot_event["Event"]["timestamp"] = json!([index + 1, 0]);
        let snapshot = &mut snapshot_event["Event"]["data"]["Instant"]["data"]["Snapshot"];
        snapshot["re_download_bytes"] = json!(re_bytes);
        snapshot["http_download_bytes"] = json!(http_bytes);
        writeln!(output, "{}", serde_json::to_string(&snapshot_event)?)?;
    }
    std::fs::write(&log_path, output)?;

    let result = buck.log(&["summary", log_path.to_str().unwrap()]).await?;

    assert_eq!(
        result
            .stdout
            .lines()
            .filter(|line| line.contains("downloaded:"))
            .collect::<Vec<_>>(),
        [
            format!("- Total downloaded: {}B", expected_re + expected_http),
            format!("  - RE downloaded: {expected_re}B"),
            format!("  - HTTP downloaded: {expected_http}B"),
        ]
    );
    Ok(())
}

#[buck_test]
async fn test_log_summary_downloads_nonzero_baseline(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    test_log_summary_downloads(
        buck,
        tmp_path,
        &[(100_000, 200_000), (100_100, 200_200), (100_300, 200_500)],
        300,
        500,
    )
    .await
}

#[buck_test]
async fn test_log_summary_downloads_zero_baseline(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    test_log_summary_downloads(buck, tmp_path, &[(0, 0), (300, 500)], 300, 500).await
}

#[buck_test]
async fn test_log_summary_downloads_no_downloads(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    test_log_summary_downloads(
        buck,
        tmp_path,
        &[(100_000, 200_000), (100_000, 200_000)],
        0,
        0,
    )
    .await
}

#[buck_test]
async fn test_log_summary_downloads_no_snapshots(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    test_log_summary_downloads(buck, tmp_path, &[], 0, 0).await
}

#[buck_test]
async fn test_log_summary_downloads_one_snapshot(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    test_log_summary_downloads(buck, tmp_path, &[(100_000, 200_000)], 0, 0).await
}
