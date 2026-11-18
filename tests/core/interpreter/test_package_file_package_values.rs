/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context;
use buck2_e2e::prelude::*;
use serde_json::Value;
use serde_json::json;
use serde_json::value::RawValue;

fn sort_streaming_targets(stdout: &str) -> anyhow::Result<String> {
    let start = stdout.find('[').context("No JSON list on stdout")?;
    let entries = serde_json::Deserializer::from_str(&stdout[start..])
        .into_iter::<Vec<&RawValue>>()
        .next()
        .context("No JSON list on stdout")??;
    let mut entries = entries
        .into_iter()
        .map(|raw| {
            let entry: Value = serde_json::from_str(raw.get())?;
            let field = |key: &str| {
                entry[key]
                    .as_str()
                    .map(str::to_owned)
                    .with_context(|| format!("No `{key}` in `{entry}`"))
            };
            Ok(((field("buck.package")?, field("name")?), raw.get()))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    if entries.is_empty() {
        return Ok("[]\n".to_owned());
    }
    let entries: Vec<String> = entries
        .into_iter()
        .map(|(_, raw)| format!("  {raw}"))
        .collect();
    Ok(format!("[\n{}\n]\n", entries.join(",\n")))
}

#[buck_test]
async fn test_package_file_package_values(buck: &Buck) -> anyhow::Result<()> {
    // Build file does all the assertions.
    let output = buck.build(&["//:"]).await?;
    assert!(output.stderr.contains("TEST PASSED"));
    Ok(())
}

#[buck_test]
async fn test_audit_package_values(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck.audit(&["package-values", "//"]).await?.stdout;
    golden(&stdout, "audit-package-values.golden.json");
    Ok(())
}

#[buck_test]
async fn test_audit_package_values_select(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck.audit(&["package-values", "//"]).await?.stdout;
    let result: Value = serde_json::from_str(&stdout)?;
    let pkg = &result["root//"];
    // Verify select value has expected JSON structure
    assert_eq!(
        pkg["sel.ector"],
        json!({
            "__type": "selector",
            "entries": {"//config:a": "val_a", "DEFAULT": "default_val"},
        })
    );
    // Verify concat (select + select) has expected JSON structure
    assert_eq!(pkg["sel.concat"]["__type"], "concat");
    assert_eq!(pkg["sel.concat"]["items"].as_array().map(Vec::len), Some(2));
    // Verify visibility fields exist alongside package values
    assert!(pkg.get("visibility").is_some());
    assert!(pkg.get("within_view").is_some());
    assert!(pkg.get("visibility_cap").is_some());
    assert!(pkg.get("within_view_cap").is_some());
    assert!(pkg.get("visibility_intersection").is_some());
    Ok(())
}

#[buck_test]
async fn test_audit_package_values_visibility_intersection(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck
        .audit(&["package-values", "//intersection/child"])
        .await?
        .stdout;
    let result: Value = serde_json::from_str(&stdout)?;
    let pkg = &result["root//intersection/child"];
    // Legacy collapsed shape is kept for backwards compatibility.
    let legacy = &pkg["visibility_cap"];
    assert!(
        legacy.is_object(),
        "Expected dict for intersection, got {legacy}"
    );
    assert!(legacy.get("intersection").is_some());
    assert_eq!(legacy["intersection"].as_array().map(Vec::len), Some(2));
    // New per-layer shape never collapses.
    let layers = &pkg["visibility_intersection"];
    assert_eq!(layers.as_array().map(Vec::len), Some(2));
    assert_eq!(layers[0]["patterns"], json!(["root//intersection/..."]));
    assert_eq!(layers[0]["exempt_targets"], json!([]));
    assert_eq!(
        layers[1]["patterns"],
        json!(["root//intersection/child/..."])
    );
    assert_eq!(layers[1]["exempt_targets"], json!([]));
    Ok(())
}

#[buck_test]
async fn test_audit_package_values_visibility_intersection_exempt(
    buck: &Buck,
) -> anyhow::Result<()> {
    let stdout = buck
        .audit(&["package-values", "//intersection/exempt"])
        .await?
        .stdout;
    let result: Value = serde_json::from_str(&stdout)?;
    let pkg = &result["root//intersection/exempt"];
    let intersection = &pkg["visibility_intersection"];
    assert_eq!(
        *intersection,
        json!([
            {"patterns": ["root//intersection/..."], "exempt_targets": []},
            {
                "patterns": ["root//intersection/exempt/..."],
                "exempt_targets": ["root//intersection/exempt:"],
            },
        ]),
        "Unexpected exempt intersection shape: {intersection}"
    );
    // Legacy collapsed shape ignores exemptions.
    let legacy = &pkg["visibility_cap"];
    assert_eq!(
        *legacy,
        json!({
            "intersection": [
                ["root//intersection/..."],
                ["root//intersection/exempt/..."],
            ],
        }),
        "Unexpected legacy shape: {legacy}"
    );
    Ok(())
}

#[buck_test]
async fn test_targets_package_values(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck.targets(&["--package-values", "//..."]).await?.stdout;
    golden(&stdout, "targets-package-values.golden.json");
    Ok(())
}

#[buck_test]
async fn test_targets_package_values_regex(buck: &Buck) -> anyhow::Result<()> {
    async fn package_values(buck: &Buck, args: &[&str]) -> anyhow::Result<Value> {
        let out = buck.targets(args).await?.stdout;
        let json_result: Value = serde_json::from_str(&out)?;
        Ok(json_result[0]["buck.package_values"].clone())
    }

    // Empty string as regex matches all keys.
    let pv = package_values(buck, &["--package-values-regex", "", "//..."]).await?;
    assert_eq!(pv["aaa.bbb"], "ccc");
    assert_eq!(pv["xxx.yyy"], "zzz");
    assert_eq!(pv["sel.ector"]["__type"], "selector");
    assert_eq!(pv["sel.concat"]["__type"], "concat");

    let pv = package_values(buck, &["--package-values-regex", "aaa.bbb", "//..."]).await?;
    let expected = json!({"aaa.bbb": "ccc"});
    assert_eq!(pv, expected);

    let pv = package_values(buck, &["--package-values-regex", "xxx", "//..."]).await?;
    let expected = json!({"xxx.yyy": "zzz"});
    assert_eq!(pv, expected);

    let pv = package_values(
        buck,
        &[
            "--package-values-regex",
            "aaa.bbb",
            "--package-values-regex",
            "xxx.yyy",
            "//...",
        ],
    )
    .await?;
    let expected = json!({"aaa.bbb": "ccc", "xxx.yyy": "zzz"});
    assert_eq!(pv, expected);

    // Regex matching select keys.
    let pv = package_values(buck, &["--package-values-regex", "sel", "//..."]).await?;
    assert_eq!(pv.as_object().map(serde_json::Map::len), Some(2));
    assert_eq!(pv["sel.ector"]["__type"], "selector");
    assert_eq!(pv["sel.concat"]["__type"], "concat");

    let pv = package_values(buck, &["--package-values-regex", "non_existent", "//..."]).await?;
    let expected = json!({});
    assert_eq!(pv, expected);

    let args = ["allow", "only", "one", "arg", "per", "flag", "occurrence"];
    let mut argv = vec!["--package-values-regex"];
    argv.extend(args);
    argv.push("//...");
    buck.targets(&argv)
        .await
        .expect_failure()
        .assert_stderr_matches("Error parsing root//arg");
    Ok(())
}

#[buck_test]
async fn test_targets_streaming_package_values(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck
        .targets(&["--streaming", "--package-values", "//..."])
        .await?
        .stdout;
    golden(
        &sort_streaming_targets(&stdout)?,
        "targets-streaming-package-values.golden.json",
    );
    Ok(())
}
