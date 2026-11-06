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

/// Test that targets --streaming --imports handles JSON file imports.
#[buck_test]
async fn test_imports_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .targets(&["//...", "--json", "--streaming", "--imports"])
        .await?;
    let xs: Vec<Value> = serde_json::from_str(&result.stdout)?;

    let mut found_targets = false;
    let mut found_bzl = false;
    let mut found_json = false;

    for x in &xs {
        if x.get("buck.imports").is_none() {
            continue;
        }
        let file = &x["buck.file"];
        let imports = x["buck.imports"].as_array().unwrap();

        if file == "root//TARGETS.fixture" {
            assert!(imports.contains(&json!("root//uses_json.bzl")));
            found_targets = true;
        } else if file == "root//uses_json.bzl" {
            assert!(imports.contains(&json!("root//data.json")));
            found_bzl = true;
        } else if file == "root//data.json" {
            assert!(imports.is_empty());
            found_json = true;
        }
    }

    assert!(found_targets, "TARGETS.fixture imports should be reported");
    assert!(
        found_bzl,
        "uses_json.bzl imports (including data.json) should be reported"
    );
    assert!(
        found_json,
        "data.json should appear as an import with empty sub-imports"
    );
    Ok(())
}
