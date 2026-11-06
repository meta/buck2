/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::HashMap;

use buck2_e2e::prelude::*;
use serde_json::Value;
use serde_json::json;

#[buck_test]
async fn test_imports_cross_cell(buck: &Buck) -> anyhow::Result<()> {
    // `nested//` has no `PACKAGE` files of its own, but the root cell's
    // `PACKAGE` file applies to it, so the imports walk must report that
    // file and its loads, matching `PACKAGE` evaluation.
    let result = buck
        .targets(&["nested//...", "--json", "--streaming", "--imports"])
        .await?;
    let xs: Vec<Value> = serde_json::from_str(&result.stdout)?;
    let files: HashMap<&str, &Value> = xs
        .iter()
        .filter(|x| x.get("buck.imports").is_some())
        .map(|x| (x["buck.file"].as_str().unwrap(), x))
        .collect();

    assert!(files.contains_key("root//PACKAGE"));
    let package_record = files["root//PACKAGE"];
    assert_eq!(
        package_record["buck.imports"],
        json!(["prelude//prelude.bzl", "root//a.bzl"])
    );
    assert!(package_record.get("buck.package").is_none());

    // The PACKAGE file's own imports are chased transitively.
    assert!(files.contains_key("root//a.bzl"));
    Ok(())
}
