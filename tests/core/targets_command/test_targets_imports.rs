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
async fn test_imports(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .targets(&["//...", "--json", "--streaming", "--imports"])
        .await?;
    let xs: Vec<Value> = serde_json::from_str(&result.stdout)?;
    let mut found = 0;
    for x in &xs {
        if x.get("buck.imports").is_some() {
            if x["buck.file"] == "root//TARGETS.fixture" {
                assert_eq!(x["buck.package"], "root//");
                assert_eq!(
                    x["buck.imports"],
                    json!(["prelude//prelude.bzl", "root//a.bzl"])
                );
                found += 1;
            } else if x["buck.file"] == "root//a.bzl" || x["buck.file"] == "root//PACKAGE" {
                assert_eq!(
                    x["buck.imports"],
                    json!(["prelude//prelude.bzl", "root//b.bzl"])
                );
                assert!(x.get("buck.package").is_none());
                found += 1;
            }
        }
    }
    assert_eq!(found, 3);
    Ok(())
}
