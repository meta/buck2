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
async fn test_not_selector_attr(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:selector_concat_dict.bxl:not_selector_attr"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_selector_dict_attr(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:selector_concat_dict.bxl:selector_dict_attr"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_selector_concat_attr(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:selector_concat_dict.bxl:selector_concat_attr"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_selector_dict_write_json(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .bxl(&["//:selector_concat_dict.bxl:selector_dict_write_json"])
        .await?;
    let file_path = res.stdout.trim();
    let content = std::fs::read_to_string(file_path)?;
    let expected_content = json!({
        "__type": "selector",
        "entries": {
            "DEFAULT": ["--foo", "--bar"],
            "root//constraints:macos": ["--foo-macos", "--bar-macos"],
            "root//constraints:x86": ["--foo-x86", "--bar-x86"],
        },
    });
    assert_eq!(serde_json::from_str::<Value>(&content)?, expected_content);
    Ok(())
}

#[buck_test]
async fn test_selector_concat_write_json(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .bxl(&["//:selector_concat_dict.bxl:selector_concat_write_json"])
        .await?;
    let file_path = res.stdout.trim();
    let content = std::fs::read_to_string(file_path)?;
    let expected_content = json!({
        "__type": "concat",
        "items": [
            ["--flag", "--baz"],
            {
                "__type": "selector",
                "entries": {
                    "DEFAULT": ["--foo", "--bar"],
                    "root//constraints:macos": ["--foo-macos", "--bar-macos"],
                    "root//constraints:x86": ["--foo-x86", "--bar-x86"],
                },
            },
        ],
    });
    assert_eq!(serde_json::from_str::<Value>(&content)?, expected_content);
    Ok(())
}
