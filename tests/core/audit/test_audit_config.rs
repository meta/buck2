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
use serde_json::Map;
use serde_json::Value;

/// The string value of `key` in `audit config --style=json` output.
fn get<'a>(json: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    json.get(key).and_then(Value::as_str)
}

/// Assert that our testing framework works as expected.
#[buck_test(extra_buck_config = [("test", "foo", "bar")])]
async fn test_extra_buck_config(buck: &Buck) -> anyhow::Result<()> {
    let cfg = buck
        .audit_config(&["--style=json"])
        .await?
        .audit_config_json()?;
    assert_eq!(get(&cfg, "test.foo"), Some("bar"));
    Ok(())
}

#[buck_test]
async fn test_audit_config_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.audit_config(&["--style=json"]).await?;
    result.audit_config_json()?;
    Ok(())
}

#[buck_test]
async fn test_audit_config_cell_json(buck: &Buck) -> anyhow::Result<()> {
    let out = buck.audit_config(&["--style", "json"]).await?;
    let out_json = out.audit_config_json()?;
    assert_eq!(get(&out_json, "test.is_root"), Some("yes"));
    assert!(out_json.get("test.is_code").is_none());

    let out = buck
        .audit_config(&["--style", "json", "--cell", "code"])
        .await?;
    let out_json = out.audit_config_json()?;
    assert_eq!(get(&out_json, "test.is_code"), Some("yes"));
    assert!(out_json.get("test.is_root").is_none());

    let out = buck
        .audit_config(&["--style", "json"])
        .rel_cwd("code")
        .await?;
    let out_json = out.audit_config_json()?;
    assert_eq!(get(&out_json, "test.is_code"), Some("yes"));
    assert!(out_json.get("test.is_root").is_none());
    Ok(())
}

#[buck_test(setup_eden = true)]
async fn test_audit_config_all_cells(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .audit_config(&["--all-cells", "--style", "json"])
        .await?;
    let out_json = out.audit_config_json()?;
    eprintln!("{out_json:?}");
    assert_eq!(get(&out_json, "code//bar.a"), Some("2"));
    assert_eq!(get(&out_json, "source//bar.a"), Some("1"));
    assert_eq!(get(&out_json, "root//bar.a"), Some("1"));
    assert!(out_json.get("b//bar.a").is_none());

    let out = buck
        .audit_config(&["--all-cells", "--style", "json", "code//bar.a"])
        .await?;
    let out_json = out.audit_config_json()?;
    assert_eq!(get(&out_json, "code//bar.a"), Some("2"));
    assert!(out_json.get("source//bar.a").is_none());

    let out = buck.audit_config(&["--all-cells"]).await?;
    assert!(out.stdout.contains("# Cell: source\n[bar]\n    a = 1\n"));
    Ok(())
}

#[buck_test]
async fn test_audit_config_with_config_value(buck: &Buck) -> anyhow::Result<()> {
    let result_config = buck
        .audit_config(&["python", "--style", "json", "-cpython.helpers=true"])
        .await?;
    let result_config_json = result_config.audit_config_json()?;

    assert_eq!(get(&result_config_json, "python.helpers"), Some("true"));
    Ok(())
}

#[buck_test]
async fn test_audit_config_with_config_file(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let configfile = tmp_path.join("config.bcfg");
    std::fs::write(&configfile, "[python]\n  helpers = true\n")?;

    let result_file = buck
        .audit_config(&[
            "--config-file",
            &configfile.to_string_lossy(),
            "--style",
            "json",
        ])
        .await?;

    assert_eq!(
        get(&result_file.audit_config_json()?, "python.helpers"),
        Some("true")
    );
    Ok(())
}

#[buck_test]
async fn test_audit_config_location_extended(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.audit_config(&["bar.a", "--location=extended"]).await?;
    assert!(result.stdout.contains("a = 1"));
    assert!(result.stdout.contains("included.bcfg:2"));
    Ok(())
}

#[buck_test]
async fn test_audit_config_with_cell_syntax(buck: &Buck) -> anyhow::Result<()> {
    let result_file = buck
        .audit_config(&["code//test.is_code", "--style", "json"])
        .await?;
    let result_file_json = result_file.audit_config_json()?;

    assert_eq!(get(&result_file_json, "code//test.is_code"), Some("yes"));
    Ok(())
}

#[buck_test]
async fn test_cell_relative_configs(buck: &Buck) -> anyhow::Result<()> {
    let result_root_cell = buck
        .audit_config(&["--config", "root//bar.a=5", "--style", "json"])
        .await?;
    let result_root_cell_json = result_root_cell.audit_config_json()?;

    assert_eq!(get(&result_root_cell_json, "foo.b"), Some("5"));

    let result_nonroot_cell = buck
        .audit_config(&[
            "foo",
            "--config",
            "code//bar.a=5",
            "--style",
            "json",
            "--cell",
            "code",
        ])
        .await?;
    let result_nonroot_cell_json = result_nonroot_cell.audit_config_json()?;

    assert_eq!(get(&result_nonroot_cell_json, "foo.b"), Some("5"));

    let result_diff_cell = buck
        .audit_config(&[
            "foo",
            "--config",
            "code//bar.a=5",
            "--style",
            "json",
            "--cell",
            "source",
        ])
        .await?;
    let result_diff_cell_json = result_diff_cell.audit_config_json()?;

    assert_eq!(get(&result_diff_cell_json, "foo.b"), Some("1"));

    let result_all_cell = buck
        .audit_config(&[
            "foo", "--config", "bar.a=5", "--style", "json", "--cell", "source",
        ])
        .await?;
    let result_all_cell_json = result_all_cell.audit_config_json()?;

    assert_eq!(get(&result_all_cell_json, "foo.b"), Some("5"));
    Ok(())
}
