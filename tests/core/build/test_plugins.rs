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

use buck2_e2e::prelude::*;
use serde_json::Value;
use serde_json::json;

#[buck_test]
async fn test_deps_in_cquery_not_uquery(buck: &Buck) -> anyhow::Result<()> {
    // Check that plugin deps appear as deps in uquery but not in cquery
    let result = buck.uquery(&["deps(//tests:reg_a)"]).await?;
    assert!(result.stdout.contains("//tests:reg_a_REAL"));
    let result = buck.cquery(&["deps(//tests:reg_a)"]).await?;
    assert!(!result.stdout.contains("//tests:reg_a_REAL"));
    // And make sure that the attribute itself is serialized correctly in cquery and uquery
    let result = buck.uquery(&["-a", "actual", "//tests:reg_a"]).await?;
    assert_eq!(
        serde_json::from_str::<Value>(&result.stdout)?,
        json!({"root//tests:reg_a": {"actual": "root//tests:reg_a_REAL"}})
    );
    let result = buck.cquery(&["-a", "actual", "//tests:reg_a"]).await?;
    assert_eq!(
        serde_json::from_str::<Value>(&result.stdout)?,
        json!({"root//tests:reg_a (<unspecified>)": {"actual": "root//tests:reg_a_REAL"}})
    );
    Ok(())
}

/// The strings of a JSON array.
fn strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

#[buck_test]
async fn test_cquery(buck: &Buck) -> anyhow::Result<()> {
    // ##### Check that everything is correctly configured as reported by cquery
    let result = buck
        .cquery(&[
            "--json",
            "-a",
            "buck.deps",
            "-a",
            "buck.execution_platform",
            "-a",
            "buck.plugins",
            "deps(//tests:b)",
        ])
        .await?;
    let result: serde_json::Map<String, Value> = serde_json::from_str(&result.stdout)?;

    let find = |prefix: &str| {
        result
            .iter()
            .find(|(k, _)| k.starts_with(prefix))
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("No `{prefix}` in the cquery result"))
    };
    let b = find("root//tests:b");
    let l = find("root//tests:l");

    assert_eq!(
        strings(&b["buck.plugins"]["RustProcMacro"])
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "root//tests:reg_a_REAL",
            "root//tests:reg_b_REAL",
            "root//tests:doc_a_REAL",
        ])
    );
    assert_eq!(
        strings(&l["buck.plugins"]["RustProcMacro"])
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["root//tests:reg_a_REAL", "root//tests:doc_b_REAL"])
    );

    assert!(
        b["buck.execution_platform"]
            .as_str()
            .unwrap()
            .starts_with("root//config:platform_linux")
    );
    assert!(
        strings(&b["buck.deps"])
            .iter()
            .any(|dep| dep.starts_with("root//tests:reg_a_REAL (root//config:platform_linux"))
    );
    assert!(
        l["buck.execution_platform"]
            .as_str()
            .unwrap()
            .starts_with("root//config:platform_windows")
    );
    assert!(
        strings(&l["buck.deps"])
            .iter()
            .any(|dep| dep.starts_with("root//tests:reg_a_REAL (root//config:platform_windows"))
    );

    assert!(
        result
            .keys()
            .any(|k| k.starts_with("root//tests:reg_a_REAL (root//config:platform_linux"))
    );
    assert!(
        result
            .keys()
            .any(|k| k.starts_with("root//tests:reg_a_REAL (root//config:platform_windows"))
    );
    Ok(())
}

#[buck_test]
async fn test_analysis(buck: &Buck) -> anyhow::Result<()> {
    // Check that we can properly identify all the different plugin deps in analysis
    let result = buck.build(&["root//tests:b", "root//tests:l"]).await?;

    let b: Value = serde_json::from_str(&std::fs::read_to_string(
        result.build_report()?.output_for_target("root//tests:b")?,
    )?)?;
    assert_eq!(
        b,
        json!({
            "indirect": ["Reg A (linux)"],
            "direct": ["Reg B (linux)"],
            "indirect_doc": ["Doc A (linux)"],
            "direct_doc": [],
        })
    );

    let l: Value = serde_json::from_str(&std::fs::read_to_string(
        result.build_report()?.output_for_target("root//tests:l")?,
    )?)?;
    assert_eq!(
        l,
        json!({
            "indirect": [],
            "direct": ["Reg A (windows)"],
            "indirect_doc": [],
            "direct_doc": ["Doc B (windows)"],
        })
    );
    Ok(())
}

#[buck_test]
async fn test_plugin_dep_errors(buck: &Buck) -> anyhow::Result<()> {
    // Tests are explained in the file
    buck.build(&["//test_errors:recursive_target_1"]).await?;

    buck.build(&["//test_errors:regular_a"]).await?;

    let result = buck.uquery(&["deps(//test_errors:regular_b)"]).await?;
    assert!(result.stdout.contains("//test_errors:toolchain"));
    let result = buck
        .build(&["//test_errors:regular_b"])
        .await
        .expect_failure();
    assert!(
        result
            .stderr
            .contains("Plugin dep `root//test_errors:toolchain` is a toolchain rule")
    );

    let result = buck
        .build(&["//test_errors:wrong_plugin_kind"])
        .await
        .expect_failure();
    assert!(
        result
            .stderr
            .contains("The rule did not declare that it uses plugins of kind A")
    );
    Ok(())
}

#[buck_test]
async fn test_repeated_insertion(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "-a",
            "buck.plugins",
            "//repeated_insertion:different_deps_alias",
        ])
        .await?;
    let result: serde_json::Map<String, Value> = serde_json::from_str(&result.stdout)?;
    let first = result.values().next().expect("a cquery result");
    assert_eq!(
        json!({"Plugin": ["root//repeated_insertion:plugin"]}),
        first["buck.plugins"]
    );
    Ok(())
}

#[buck_test]
async fn test_visibility(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&["//visibility:missing_access"])
        .await
        .expect_failure();
    assert!(result.stderr.contains(
        "`root//visibility/package:hidden` is not visible to `root//visibility:missing_access`"
    ));

    buck.build(&["//visibility:has_access"]).await?;
    Ok(())
}
