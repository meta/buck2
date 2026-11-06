/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeSet;
use std::path::Path;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use serde_json::Value;

fn parse_json_diffs(stdout: &str) -> anyhow::Result<Vec<Value>> {
    // first three lines are the header
    stdout
        .lines()
        .skip(3)
        .filter(|line| !line.trim().is_empty())
        .map(|line| Ok(serde_json::from_str(line)?))
        .collect()
}

/// The path of the last command's event log.
async fn last_log(buck: &Buck) -> anyhow::Result<String> {
    Ok(buck.log(&["last"]).await?.stdout.trim().to_owned())
}

#[buck_test]
async fn test_no_action_divergence_command(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:simple", "-c", "test.buck2_output=foo"])
        .await?;
    let path1 = last_log(buck).await?;
    buck.build(&["//:simple", "-c", "test.buck2_output=foo"])
        .await?;
    let path2 = last_log(buck).await?;
    let out = buck
        .log(&[
            "diff",
            "action-divergence",
            "--path1",
            &path1,
            "--path2",
            &path2,
        ])
        .await?;

    assert!(out.stdout.contains("No divergent actions found."));
    Ok(())
}

#[buck_test]
async fn test_action_divergence_command(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:non_det", "-c", "test.buck2_output=foo"])
        .await?;
    buck.build(&["//:non_det", "-c", "test.buck2_output=bar"])
        .await?;
    let out = buck
        .log(&[
            "diff",
            "action-divergence",
            "--recent1",
            "0",
            "--recent2",
            "1",
        ])
        .await?;

    assert!(out.stdout.contains(
        "Present in both builds with differing output digests\nprelude//:non_det (<unspecified>) (write foo.txt)"
    ));
    Ok(())
}

#[buck_test]
async fn test_no_config_diff_command(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:simple", "-c", "test.buck2_output=foo"])
        .await?;
    let path1 = last_log(buck).await?;
    buck.build(&["//:simple", "-c", "test.buck2_output=foo"])
        .await?;
    let path2 = last_log(buck).await?;
    let out = buck
        .log(&[
            "diff",
            "external-configs",
            "--path1",
            &path1,
            "--path2",
            &path2,
            "--format=json",
        ])
        .await?;
    let diffs = parse_json_diffs(&out.stdout)?;
    assert_eq!(diffs.len(), 0);
    Ok(())
}

#[buck_test]
async fn test_diff_order_config_diff_command(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:simple",
        "-c",
        "test.buck2_output=out",
        "-c",
        "test.key_a=1",
        "-c",
        "test.key_b=2",
    ])
    .await?;
    let path1 = last_log(buck).await?;
    buck.build(&[
        "//:simple",
        "-c",
        "test.buck2_output=out",
        "-c",
        "test.key_b=2",
        "-c",
        "test.key_a=1",
    ])
    .await?;
    let path2 = last_log(buck).await?;
    let out = buck
        .log(&[
            "diff",
            "external-configs",
            "--path1",
            &path1,
            "--path2",
            &path2,
            "--format=json",
        ])
        .await?;
    let diffs = parse_json_diffs(&out.stdout)?;
    assert!(diffs.len() == 1 && diffs[0].get("FullDiff").is_some());

    let changes = diffs[0]["FullDiff"]["changes"].as_array().unwrap();
    let keys = ["test.key_a=1", "test.key_b=2"];
    let order = |tags: [&str; 2]| -> Vec<&str> {
        changes
            .iter()
            .filter(|c| c["tag"].as_str().is_some_and(|tag| tags.contains(&tag)))
            .filter_map(|c| c["value"].as_str())
            .filter(|value| keys.contains(value))
            .collect()
    };
    let original_order = order(["equal", "delete"]);
    let new_order = order(["equal", "insert"]);
    assert_eq!(original_order, ["test.key_a=1", "test.key_b=2"]);
    assert_eq!(new_order, ["test.key_b=2", "test.key_a=1"]);
    Ok(())
}

#[buck_test]
async fn test_config_diff_command_command_line(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    buck.build(&[
        "//:simple",
        "-c",
        "test.buck2_output=changed_old",
        "-c",
        "test.first=x",
        "-c",
        "test.first=overwrite_x",
    ])
    .await?;
    let path1 = last_log(buck).await?;
    let config_file = tmp_path.join("config");
    std::fs::write(&config_file, "[test]\nsecond = x\n")?;
    buck.build(&[
        "//:simple",
        "-c",
        "test.buck2_output=changed_new",
        "--config-file",
        config_file.to_str().unwrap(),
    ])
    .await?;
    let path2 = last_log(buck).await?;
    let out = buck
        .log(&[
            "diff",
            "external-configs",
            "--path1",
            &path1,
            "--path2",
            &path2,
            "--format=json",
        ])
        .await?;
    let summary_diffs: Vec<Value> = parse_json_diffs(&out.stdout)?
        .into_iter()
        .filter(|d| d.get("FullDiff").is_none())
        .collect();
    assert_eq!(summary_diffs.len(), 3);

    assert!(
        summary_diffs[0]["Changed"]["key"] == "test.buck2_output"
            && summary_diffs[0]["Changed"]["old_value"] == "changed_old"
            && summary_diffs[0]["Changed"]["new_value"] == "changed_new"
    );

    assert!(
        summary_diffs[1]["FirstOnly"]["key"] == "test.first"
            && summary_diffs[1]["FirstOnly"]["value"] == "overwrite_x"
    );
    assert!(
        summary_diffs[2]["SecondOnly"]["key"] == "test.second"
            && summary_diffs[2]["SecondOnly"]["value"] == "x"
    );
    Ok(())
}

#[buck_test]
async fn test_config_diff_command_project_relative(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:simple",
        "-c",
        "test.buck2_output=out",
        "@root//mode/my_mode_a",
        "@root//mode/my_mode_c",
    ])
    .await?;
    let path1 = last_log(buck).await?;
    buck.build(&[
        "//:simple",
        "-c",
        "test.buck2_output=out",
        "@root//mode/my_mode_b",
        "@root//mode/my_mode_c",
        "@root//mode/my_mode_b",
    ])
    .await?;
    let path2 = last_log(buck).await?;
    let out = buck
        .log(&[
            "diff",
            "external-configs",
            "--path1",
            &path1,
            "--path2",
            &path2,
            "--format=json",
        ])
        .await?;
    let summary_diffs: Vec<Value> = parse_json_diffs(&out.stdout)?
        .into_iter()
        .filter(|d| d.get("FullDiff").is_none())
        .collect();
    assert_eq!(summary_diffs.len(), 2);

    let keys = |kind: &str| -> BTreeSet<&str> {
        summary_diffs
            .iter()
            .filter_map(|d| d.get(kind))
            .map(|d| d["key"].as_str().unwrap())
            .collect()
    };
    // We only store the path of the modefile
    assert_eq!(keys("FirstOnly"), BTreeSet::from(["my_mode_a.bcfg"]));
    assert_eq!(keys("SecondOnly"), BTreeSet::from(["my_mode_b.bcfg"]));
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_config_diff_tracker_modfile_change(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:simple",
        "-c",
        "test.buck2_output=out",
        "@root//mode/my_mode_a",
    ])
    .await?;
    let res = buck
        .build(&[
            "//:simple",
            "-c",
            "test.buck2_output=out",
            "@root//mode/my_mode_b",
        ])
        .await?;
    let cell_config_diffs = filter_events(
        buck,
        &["Event", "data", "Instant", "data", "CellHasNewConfigs"],
    )
    .await?;
    assert!(cell_config_diffs.len() == 1 && cell_config_diffs[0]["cell"] == "prelude");

    assert_eq!(res.invocation_record()?["new_configs_used"], true);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_config_diff_tracker_no_change(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:simple",
        "-c",
        "test.buck2_output=out",
        "@root//mode/my_mode_a",
    ])
    .await?;
    let res = buck
        .build(&[
            "//:simple",
            "-c",
            "test.buck2_output=out",
            "@root//mode/my_mode_a",
        ])
        .await?;
    let cell_config_diffs = filter_events(
        buck,
        &["Event", "data", "Instant", "data", "CellHasNewConfigs"],
    )
    .await?;
    assert_eq!(cell_config_diffs.len(), 0);
    assert_eq!(res.invocation_record()?["new_configs_used"], false);
    Ok(())
}
