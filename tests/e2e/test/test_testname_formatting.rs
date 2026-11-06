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

/////////
// Helpers
/////////

fn get_test_name_from_end_event(event: &Value) -> anyhow::Result<Vec<String>> {
    let test_names = event
        .pointer("/Event/data/SpanEnd/data/TestRun/suite/test_names")
        .and_then(Value::as_array)
        .with_context(|| format!("No `test_names` in the `TestRun` end event: {event}"))?;
    test_names
        .iter()
        .map(|name| {
            name.as_str()
                .map(str::to_owned)
                .with_context(|| format!("A test name that is not a string: {name}"))
        })
        .collect()
}

fn get_events_test_names(log: &[&str]) -> anyhow::Result<Vec<String>> {
    let mut test_names = Vec::new();
    for line in log
        .iter()
        .filter(|line| line.contains("\"SpanEnd\"") && line.contains("\"TestRun\""))
    {
        let event: Value = serde_json::from_str(line)?;
        test_names.extend(get_test_name_from_end_event(&event)?);
    }
    test_names.sort();
    Ok(test_names)
}

async fn name_formatting(
    buck: &Buck,
    adapter: &str,
    listing: &str,
    python_version: &str,
) -> anyhow::Result<()> {
    let target = format!("{adapter}_{listing}_{python_version}");

    if python_version == "3.12" && adapter == "new" {
        // Test name formatting is different in 3.12
        return Ok(());
    }

    buck.test(&[&format!(
        "fbcode//buck2/tests/targets/rules/python/test_name_formatting:{target}"
    )])
    .await
    .expect_failure();
    let log = buck.log(&["show"]).await?.stdout;
    let log: Vec<&str> = log.trim().lines().collect();
    let actual_tests = get_events_test_names(&log)?;
    let expected_tests = [
        "test_failure (buck2.tests.targets.rules.python.test_name_formatting.test_name_formatting.TestCase)",
        "test_nested_test_class (buck2.tests.targets.rules.python.test_name_formatting.test_name_formatting.TestCase)",
        "test_success (buck2.tests.targets.rules.python.test_name_formatting.test_name_formatting.TestCase)",
    ];
    assert_eq!(expected_tests.as_slice(), actual_tests.as_slice());
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_name_formatting_3_12_static_testpilot(buck: &Buck) -> anyhow::Result<()> {
    name_formatting(buck, "testpilot", "static", "3.12").await
}

#[buck_test(inplace = true)]
async fn test_name_formatting_3_12_static_builtin(buck: &Buck) -> anyhow::Result<()> {
    name_formatting(buck, "builtin", "static", "3.12").await
}

#[buck_test(inplace = true)]
async fn test_name_formatting_3_12_dynamic_testpilot(buck: &Buck) -> anyhow::Result<()> {
    name_formatting(buck, "testpilot", "dynamic", "3.12").await
}

#[buck_test(inplace = true)]
async fn test_name_formatting_3_12_dynamic_builtin(buck: &Buck) -> anyhow::Result<()> {
    name_formatting(buck, "builtin", "dynamic", "3.12").await
}
