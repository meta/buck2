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
use buck2_e2e::utils::replace_hash;
use serde_json::Value;
use serde_json::json;

#[buck_test]
async fn test_ctargets_basic(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&["root//:gum", "--target-platforms=root//:p"])
        .await?;
    let lines: Vec<&str> = result.stdout.lines().collect();
    let [line] = lines.as_slice() else {
        panic!("expected one line: {lines:?}");
    };
    let line = replace_hash(line);
    assert_eq!(line, "root//:gum (root//:p#<HASH>)");
    Ok(())
}

/// Panics unless the target `output` has the attribute `key`.
#[track_caller]
fn attribute<'a>(output: &'a Value, key: &str) -> &'a Value {
    output
        .get(key)
        .unwrap_or_else(|| panic!("no `{key}` in {output}"))
}

#[buck_test]
async fn test_ctargets_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.ctargets(&["root//:chocolate", "--json"]).await?;

    let outputs: Vec<Value> = serde_json::from_str(&result.stdout)?;
    let [output] = outputs.as_slice() else {
        panic!("expected one target: {outputs:?}");
    };

    attribute(output, "buck.type");
    attribute(output, "buck.deps");
    attribute(output, "buck.inputs");
    attribute(output, "buck.package");
    attribute(output, "name");
    assert_eq!(attribute(output, "default_target_platform"), "root//:p");
    attribute(output, "visibility");
    attribute(output, "within_view");
    Ok(())
}

#[buck_test]
async fn test_ctargets_multi_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.ctargets(&["root//:", "--json"]).await?;

    let outputs: Vec<Value> = serde_json::from_str(&result.stdout)?;

    assert_eq!(outputs.len(), 3);

    for output in &outputs {
        attribute(output, "buck.type");
        attribute(output, "buck.deps");
        attribute(output, "buck.inputs");
        attribute(output, "buck.package");

        let name = attribute(output, "name");
        if name == "chocolate" {
            assert_eq!(attribute(output, "default_target_platform"), "root//:p");
        }

        attribute(output, "visibility");
        attribute(output, "within_view");
    }
    Ok(())
}

#[buck_test]
async fn test_ctargets_output_attribute(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "root//:chocolate",
            "--output-attribute=default_*",
            "--output-attribute=name",
        ])
        .await?;

    let outputs: Vec<Value> = serde_json::from_str(&result.stdout)?;
    let [output] = outputs.as_slice() else {
        panic!("expected one target: {outputs:?}");
    };

    assert_eq!(
        &json!({
            "name": "chocolate",
            "default_target_platform": "root//:p",
        }),
        output
    );
    Ok(())
}
