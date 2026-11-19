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

/// The JSON after the timestamp of a log line, as Python's `line.split(maxsplit=1)[1]` gives it.
fn json_after_timestamp(line: &str) -> anyhow::Result<Value> {
    // Terrible way to strip out the timestamp from the log line...
    let (_, json) = line
        .trim_start()
        .split_once(char::is_whitespace)
        .with_context(|| format!("No JSON after the timestamp in `{line}`"))?;
    Ok(serde_json::from_str(json.trim())?)
}

fn get<'a>(value: &'a Value, key: &str) -> anyhow::Result<&'a Value> {
    value
        .get(key)
        .with_context(|| format!("No `{key}` in {value}"))
}

#[buck_test(inplace = false)]
async fn test_buck_config_logging_disabled(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.targets(&["//:"]).await?;
    assert!(!result.stderr.contains("starlark_log_buckconfig"));
    assert!(!result.stderr.contains("starlark_log_all_buckconfigs"));
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_buck_config_logging_enabled(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .targets(&["//:", "--config", "buckconfig.log=test.read1"])
        .await?;
    let lines: Vec<&str> = result
        .stderr
        .lines()
        .filter(|line| {
            line.contains("starlark_log_buckconfig")
            // Unfortunately, "starlark_log_buckconfig" also shows up inside the stacktrace, so
            // try to exclude these lines here
            && !line.contains("print(")
        })
        .collect();
    assert_eq!(lines.len(), 1);

    let result = buck
        .targets(&[
            "//:",
            "--config",
            "buckconfig.log=test.not_a_valid_buckconfig",
        ])
        .await?;
    let lines: Vec<&str> = result
        .stderr
        .lines()
        .filter(|line| line.contains("starlark_log_buckconfig"))
        .collect();
    assert_eq!(lines.len(), 0);
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_buck_config_logging_enabled_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .targets(&["//:", "--config", "buckconfig.log_json=test.read1"])
        .await?;
    let lines: Vec<&str> = result
        .stderr
        .lines()
        .filter(|line| line.contains("starlark_log_buckconfig"))
        .collect();
    assert_eq!(lines.len(), 1, "{}", result.stderr);
    let log = json_after_timestamp(lines[0])?;
    let read_config = get(&log, "starlark_log_buckconfig")?;
    assert_eq!(get(read_config, "cell")?, "root");

    let result = buck
        .targets(&[
            "//:",
            "--config",
            "buckconfig.log_json=test.not_a_valid_buckconfig",
        ])
        .await?;
    let lines: Vec<&str> = result
        .stderr
        .lines()
        .filter(|line| line.contains("starlark_log_buckconfig"))
        .collect();
    assert_eq!(lines.len(), 0);
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_buck_config_logging_all_enabled(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .targets(&["//:", "--config", "buckconfig.log_all_in_json=true"])
        .await?;
    eprintln!("{}", result.stderr);
    let jsons = result
        .stderr
        .lines()
        .filter(|line| line.contains(r#"{"starlark_log_all_buckconfigs"#))
        .map(|line| Ok(get(&json_after_timestamp(line)?, "starlark_log_all_buckconfigs")?.clone()))
        .collect::<anyhow::Result<Vec<Value>>>()?;
    let mut filtered = Vec::new();
    for j in &jsons {
        if get(j, "section")? == "test" {
            filtered.push(j);
        }
    }
    assert_eq!(filtered.len(), 2);
    for j in filtered {
        let key = get(j, "key")?;
        assert!(key == "read1" || key == "read2", "{key}");
        assert_eq!(get(j, "cell")?, "root");
    }
    Ok(())
}
