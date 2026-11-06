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

fn has_key(json: &Value, key: &str) -> bool {
    json.as_object().is_some_and(|o| o.contains_key(key))
}

#[buck_test]
async fn test_audit_parse(buck: &Buck) -> anyhow::Result<()> {
    // random config hash
    let config_hash = "3f794b0267173c8e";

    // rule
    // json
    let result = buck
        .audit(&[
            "parse",
            &format!("buck-out/v2/art/root/{config_hash}/path/to/target/__target_name__/output"),
            "--json",
        ])
        .await?;

    let result: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(result["cell_path"], "root//path/to/target");
    assert_eq!(result["target_label"], "root//path/to/target:target_name");
    assert_eq!(result["short_artifact_path"], "output");
    assert_eq!(result["config_hash"], config_hash);
    assert!(!has_key(&result, "content_hash"));
    assert_eq!(
        result["full_artifact_path_no_hash"],
        "root/path/to/target/__target_name__/output"
    );

    // not json
    let result = buck
        .audit(&[
            "parse",
            &format!("buck-out/v2/art/root/{config_hash}/path/to/target/__target_name__/output"),
        ])
        .await?;

    let result: Vec<&str> = result.stdout.lines().collect();
    assert_eq!(result[0], "root//path/to/target");
    assert_eq!(result[1], "root//path/to/target:target_name");
    assert_eq!(result[2], "output");
    assert_eq!(result[3], config_hash);
    assert_eq!(result[4], "root/path/to/target/__target_name__/output");

    // output attribute
    let result = buck
        .audit(&[
            "parse",
            &format!("buck-out/v2/art/root/{config_hash}/path/to/target/__target_name__/output"),
            "--output-attribute",
            "config_hash",
            "--output-attribute",
            "full_artifact_path_no_hash",
        ])
        .await?;

    let result: Vec<&str> = result.stdout.lines().collect();
    assert_eq!(result[0], config_hash);
    assert_eq!(result[1], "root/path/to/target/__target_name__/output");

    // output attribute with json
    let result = buck
        .audit(&[
            "parse",
            &format!("buck-out/v2/art/root/{config_hash}/path/to/target/__target_name__/output"),
            "--json",
            "--output-attribute",
            "config_hash",
            "--output-attribute",
            "full_artifact_path_no_hash",
        ])
        .await?;

    let result: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(result["config_hash"], config_hash);
    assert_eq!(
        result["full_artifact_path_no_hash"],
        "root/path/to/target/__target_name__/output"
    );

    // tmp
    let result = buck
        .audit(&[
            "parse",
            &format!("buck-out/v2/tmp/root/{config_hash}/path/to/target/__target_name__/output"),
            "--json",
        ])
        .await?;

    let result: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(result["cell_path"], "root//path/to/target");
    assert_eq!(result["target_label"], "root//path/to/target:target_name");
    assert_eq!(result["config_hash"], config_hash);
    assert_eq!(
        result["full_artifact_path_no_hash"],
        "root/path/to/target/__target_name__/output"
    );

    // bxl
    let result = buck
        .audit(&[
            "parse",
            &format!(
                "buck-out/v2/art-bxl/root/{config_hash}/path/to/function.bxl/__function_name__/output"
            ),
            "--json",
        ])
        .await?;

    let result: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(
        result["bxl_function_label"],
        "root//path/to/function.bxl:function_name"
    );
    assert_eq!(result["config_hash"], config_hash);
    assert_eq!(
        result["full_artifact_path_no_hash"],
        "root/path/to/function.bxl/__function_name__/output"
    );

    // anon
    let result = buck
        .audit(&[
            "parse",
            &format!(
                "buck-out/v2/art-anon/root/{config_hash}/path/to/target/rule_hash/__target_name__/output"
            ),
            "--json",
        ])
        .await?;

    let result: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(result["cell_path"], "root//path/to/target");
    assert_eq!(result["target_label"], "root//path/to/target:target_name");
    assert_eq!(result["config_hash"], config_hash);
    assert_eq!(result["attr_hash"], "rule_hash");
    assert_eq!(
        result["full_artifact_path_no_hash"],
        "root/path/to/target/rule_hash/__target_name__/output"
    );

    // test
    let result = buck
        .audit(&[
            "parse",
            &format!("buck-out/v2/test/root/{config_hash}/path/to/target/__target_name__/output"),
            "--json",
        ])
        .await?;

    let result: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(result["cell_path"], "root//path/to/target");
    assert_eq!(result["config_hash"], config_hash);
    assert_eq!(
        result["full_artifact_path_no_hash"],
        "root/path/to/target/__target_name__/output"
    );
    Ok(())
}

#[buck_test]
async fn test_audit_parse_content_based(buck: &Buck) -> anyhow::Result<()> {
    // random content hash
    let content_hash = "aaaabbbbccccdddd";

    // json
    let result = buck
        .audit(&[
            "parse",
            &format!("buck-out/v2/art/root/path/to/target/__target_name__/{content_hash}/output"),
            "--json",
        ])
        .await?;

    let result: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(result["cell_path"], "root//path/to/target");
    assert_eq!(result["target_label"], "root//path/to/target:target_name");
    assert_eq!(result["short_artifact_path"], "output");
    assert_eq!(result["content_hash"], content_hash);
    assert!(!has_key(&result, "config_hash"));
    assert_eq!(
        result["full_artifact_path_no_hash"],
        "root/path/to/target/__target_name__/output"
    );

    // not json
    let result = buck
        .audit(&[
            "parse",
            &format!("buck-out/v2/art/root/path/to/target/__target_name__/{content_hash}/output"),
        ])
        .await?;

    let result: Vec<&str> = result.stdout.lines().collect();
    assert_eq!(result[0], "root//path/to/target");
    assert_eq!(result[1], "root//path/to/target:target_name");
    assert_eq!(result[2], "output");
    assert_eq!(result[3], content_hash);
    assert_eq!(result[4], "root/path/to/target/__target_name__/output");
    Ok(())
}
