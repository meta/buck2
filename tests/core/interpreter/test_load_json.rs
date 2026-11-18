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

/// Extract the DATA_LOAD_TEST_OUTPUT dict from buck2 stderr.
fn extract_test_output(stderr: &str) -> anyhow::Result<Value> {
    let marker = "DATA_LOAD_TEST_OUTPUT: ";
    let repr = stderr
        .lines()
        .find_map(|line| line.find(marker).map(|idx| &line[idx + marker.len()..]))
        .with_context(|| format!("Could not find '{marker}' in stderr:\n{stderr}"))?;
    serde_json::from_str(&starlark_repr_to_json(repr)).with_context(|| format!("Parsing `{repr}`"))
}

/// The Starlark `repr` of dicts, lists, strings, numbers, booleans and `None`, as JSON.
fn starlark_repr_to_json(repr: &str) -> String {
    let mut json = String::with_capacity(repr.len());
    let mut chars = repr.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                json.push(c);
                while let Some(c) = chars.next() {
                    json.push(c);
                    match c {
                        '\\' => json.extend(chars.next()),
                        '"' => break,
                        _ => {}
                    }
                }
            }
            'T' if chars.as_str().starts_with("rue") => {
                json.push_str("true");
                chars.nth(2);
            }
            'F' if chars.as_str().starts_with("alse") => {
                json.push_str("false");
                chars.nth(3);
            }
            'N' if chars.as_str().starts_with("one") => {
                json.push_str("null");
                chars.nth(2);
            }
            c => json.push(c),
        }
    }
    json
}

/// Assertions for types shared between JSON and TOML.
#[expect(
    clippy::approx_constant,
    reason = "3.14 is the value in the test data, not an approximation of pi"
)]
fn assert_common_types(types: &Value) {
    assert_eq!(types["string_val"], "hello");
    assert_eq!(types["int_val"], 42);
    assert_eq!(types["negative_int"], -7);
    assert_eq!(types["float_val"], 3.14);
    assert_eq!(types["negative_float"], -0.5);
    assert_eq!(types["bool_true"], true);
    assert_eq!(types["bool_false"], false);
    assert_eq!(types["string_array"], json!(["a", "b", "c"]));
    assert_eq!(types["int_array"], json!([1, 2, 3]));
    assert_eq!(types["bool_array"], json!([true, false, true]));
    assert_eq!(types["nested_table"], json!({"key": "value", "number": 99}));
}

/// Assertions for the fruits array-of-objects structure.
fn assert_common_fruits(data: &Value) {
    let fruits = data["fruits"].as_array().expect("`fruits` is a list");
    assert_eq!(fruits.len(), 2);
    assert_eq!(fruits[0]["name"], "apple");
    assert_eq!(
        fruits[0]["physical"],
        json!({"color": "red", "shape": "round"})
    );
    assert_eq!(fruits[1]["name"], "banana");
}

#[buck_test]
async fn test_load_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.targets(&["root//:"]).await?;

    // Original tests: list and object loading
    assert!(
        result
            .stderr
            .contains(r#"[{"name": "item-0"}, {"name": "item-1"}]"#)
    );
    assert!(
        result
            .stderr
            .contains(r#"{"name": "example", "object": {"foo": "bar"}}"#)
    );

    // Type coverage
    let data = extract_test_output(&result.stderr)?;

    assert_common_types(&data["types"]);
    assert_common_fruits(&data);

    // JSON-specific: null and mixed arrays
    assert_eq!(data["types"].get("null_val"), Some(&Value::Null));
    assert_eq!(data["types"]["mixed_array"], json!([1, "two", true, null]));
    Ok(())
}
