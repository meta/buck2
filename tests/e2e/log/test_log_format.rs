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
use buck2_e2e::utils::json_get;
use serde_json::Value;

/// The value at `path` in `value`, which must be there.
fn get<'a>(value: &'a Value, path: &[&str]) -> anyhow::Result<&'a Value> {
    json_get(value, path).with_context(|| format!("No `{}` in {value}", path.join(".")))
}

/// The number at `path` in `value`, which must be there.
fn number(value: &Value, path: &[&str]) -> anyhow::Result<f64> {
    get(value, path)?
        .as_f64()
        .with_context(|| format!("`{}` is not a number", path.join(".")))
}

// Tests to ensure that the log format for a few important fields hasn't
// changed. This ensures compatibility with downstream processing tools.
//
// If this test needs to be updated, please sync with @athmasagar or the
// fbcode_build_infra oncall to ensure that log parsers are also migrated.
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_log_format(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["fbcode//buck2/tests/targets/rules/cxx/hello_world:welcome"])
        .await?;
    let out = buck.log(&["show"]).await?;

    let test_line = out
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| line.contains("output_size") && line.contains("cpu_instructions_user"));

    let test_line = test_line.expect("no line has both `output_size` and `cpu_instructions_user`");
    let decoded_line: Value = serde_json::from_str(test_line)?;

    let span_end = get(&decoded_line, &["Event", "data", "SpanEnd"])?;
    assert!(number(span_end, &["duration_us"])? >= 0.0);

    let action_execution = get(span_end, &["data", "ActionExecution"])?;
    assert!(number(action_execution, &["execution_kind"])? >= 0.0);

    let target_label = get(action_execution, &["key", "owner", "TargetLabel"])?;
    let label = get(target_label, &["label"])?;
    assert!(json_get(label, &["package"]).is_some());
    assert!(json_get(label, &["name"]).is_some());
    assert!(json_get(target_label, &["configuration", "full_name"]).is_some());

    let action_name = get(action_execution, &["name"])?;
    assert!(json_get(action_name, &["category"]).is_some());
    assert!(json_get(action_name, &["identifier"]).is_some());
    assert!(number(action_execution, &["output_size"])? >= 0.0);

    let commands = get(action_execution, &["commands"])?
        .as_array()
        .context("`commands` is not a list")?;
    let execution_stats = get(
        commands.last().context("No commands")?,
        &["details", "metadata", "execution_stats"],
    )?;
    assert!(number(execution_stats, &["cpu_instructions_user"])? >= 0.0);
    assert!(number(execution_stats, &["userspace_events", "count"])? >= 0.0);
    assert!(number(execution_stats, &["userspace_events", "time_enabled"])? >= 0.0);
    assert!(number(execution_stats, &["userspace_events", "time_running"])? >= 0.0);
    Ok(())
}

// Placeholder for tests to be listed successfully on Windows.
#[buck_test(inplace = true)]
async fn test_noop(_buck: &Buck) {}
