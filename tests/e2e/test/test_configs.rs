/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use anyhow::Context;
use buck2_e2e::prelude::*;
use serde_json::Value;
use serde_json::json;

const FBCODE_TARGET: &str = "fbcode//testinfra/playground/cpp/tests:test_example";
const ARVR_TARGET: &str = "fbsource//arvr/projects/tcc_playground/python_unittest:test_example";

/////////
// Helpers
/////////

// run the buck test command and return the actual used buck config
async fn execute_test_with_args(
    buck: &Buck,
    tmp_path: &Path,
    args: &[&str],
    target: &str,
) -> anyhow::Result<Value> {
    // A trace file of its own for every run.
    let tpx_trace_path = tmp_path.join(format!(
        "tpx_trace_{}",
        std::fs::read_dir(tmp_path)?.count()
    ));
    std::fs::write(&tpx_trace_path, "")?;
    let tpx_trace_path = tpx_trace_path
        .to_str()
        .context("A non-UTF-8 temporary path")?;
    buck.test(&[args, &[target, "--", "--trace-file-path", tpx_trace_path]].concat())
        .await?;

    get_buck_config(Path::new(tpx_trace_path))
}

fn get_buck_config(tpx_trace_path: &Path) -> anyhow::Result<Value> {
    // Each row in the log is a json string
    // Find the first one with 'event name == run.external_buck_config_finalized'
    for line in std::fs::read_to_string(tpx_trace_path)?.lines() {
        let data: Value = serde_json::from_str(line)?;
        if let Some(event_name) = data.get("fields").and_then(|f| f.get("event_name"))
            && event_name == "utf.process_selector.include"
        {
            let external_config = data["fields"]
                .get("external_config")
                .and_then(Value::as_str)
                .with_context(|| format!("No `external_config` in {data}"))?;
            let config: Value = serde_json::from_str(external_config)?;
            anyhow::ensure!(
                config.is_object(),
                "`external_config` is not an object: {config}"
            );
            return Ok(config);
        }
    }
    Ok(json!({}))
}

#[track_caller]
fn assert_buck_args_config_equal(actual_config: &Value, expected_config: &Value) {
    assert_eq!(
        actual_config, expected_config,
        "Expected {expected_config}, got {actual_config}"
    );
}

#[buck_test(inplace = true)]
async fn test_no_args(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let buck_config = execute_test_with_args(buck, tmp_path, &[], FBCODE_TARGET).await?;

    assert_buck_args_config_equal(
        &buck_config,
        &json!({
            "mode": "@fbcode//mode/dev",
            "config": "",
            "host": "linux",
        }),
    );

    let buck_config = execute_test_with_args(buck, tmp_path, &[], ARVR_TARGET).await?;

    assert_buck_args_config_equal(
        &buck_config,
        &json!({
            "mode": "",
            "config": "",
            "host": "linux",
        }),
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_mode_file(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let all_configs_to_test: &[&[&str]] =
        &[&["@fbcode//mode/dev"], &["--flagfile", "fbcode//mode/dev"]];
    for config in all_configs_to_test {
        let buck_config = execute_test_with_args(buck, tmp_path, config, FBCODE_TARGET).await?;
        assert_buck_args_config_equal(
            &buck_config,
            &json!({
                "mode": "@fbcode//mode/dev",
                "config": "",
                "host": "linux",
            }),
        );
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_mode_file_non_default(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let all_configs_to_test: &[&[&str]] =
        &[&["@fbcode//mode/opt"], &["--flagfile", "fbcode//mode/opt"]];
    for config in all_configs_to_test {
        let buck_config = execute_test_with_args(buck, tmp_path, config, FBCODE_TARGET).await?;
        assert_buck_args_config_equal(
            &buck_config,
            &json!({
                "mode": "@fbcode//mode/opt",
                "config": "",
                "host": "linux",
            }),
        );
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_multi_mode_file(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let all_configs_to_test: &[&[&str]] = &[
        &["@fbcode//mode/opt", "@fbcode//mode/dev"],
        &[
            "--flagfile",
            "fbcode//mode/opt",
            "--flagfile",
            "fbcode//mode/dev",
        ],
        &["--flagfile", "fbcode//mode/opt", "@fbcode//mode/dev"],
    ];
    for config in all_configs_to_test {
        let buck_config = execute_test_with_args(buck, tmp_path, config, FBCODE_TARGET).await?;
        assert_buck_args_config_equal(
            &buck_config,
            &json!({
                "mode": "@fbcode//mode/opt;@fbcode//mode/dev",
                "config": "",
                "host": "linux",
            }),
        );
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_multi_mode_file_deduplication(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let all_configs_to_test: &[&[&str]] = &[
        &[
            "@fbcode//mode/opt",
            "@fbcode//mode/dev",
            "@fbcode//mode/dev",
        ],
        &[
            "@fbcode//mode/opt",
            "@fbcode//mode/opt",
            "@fbcode//mode/dev",
        ],
        &[
            "@fbcode//mode/dev",
            "@fbcode//mode/opt",
            "@fbcode//mode/dev",
        ],
    ];
    for config in all_configs_to_test {
        let buck_config = execute_test_with_args(buck, tmp_path, config, FBCODE_TARGET).await?;
        assert_buck_args_config_equal(
            &buck_config,
            &json!({
                "mode": "@fbcode//mode/opt;@fbcode//mode/dev",
                "config": "",
                "host": "linux",
            }),
        );
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_config(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // certain config makes it to the buck config
    let all_configs_to_test: &[&[&str]] = &[
        &["--config", "fbcode.use_link_groups_in_dev=True"],
        &["--config=fbcode.use_link_groups_in_dev=True"],
        &["-cfbcode.use_link_groups_in_dev=True"],
        &["-c", "fbcode.use_link_groups_in_dev=True"],
    ];
    for config in all_configs_to_test {
        let buck_config = execute_test_with_args(buck, tmp_path, config, FBCODE_TARGET).await?;
        assert_buck_args_config_equal(
            &buck_config,
            &json!({
                "mode": "@fbcode//mode/dev",
                "config": "fbcode.use_link_groups_in_dev=True",
                "host": "linux",
            }),
        );
    }

    // some configs are dropped
    let buck_config = execute_test_with_args(
        buck,
        tmp_path,
        &["-c", "buck2.log_configured_graph_size=true"],
        FBCODE_TARGET,
    )
    .await?;
    assert_buck_args_config_equal(
        &buck_config,
        &json!({
            "mode": "@fbcode//mode/dev",
            "config": "",
            "host": "linux",
        }),
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_config_deduplication(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // certain config makes it to the buck config
    let all_configs_to_test: &[&[&str]] = &[
        &[
            "--config=fbcode.use_link_groups_in_dev=True",
            "--config=fbcode.split-dwarf=false",
        ],
        &[
            "--config=fbcode.use_link_groups_in_dev=True",
            "--config=fbcode.split-dwarf=true",
            "--config=fbcode.split-dwarf=false",
        ],
        &[
            "--config=fbcode.use_link_groups_in_dev=False",
            "--config=fbcode.use_link_groups_in_dev=True",
            "--config=fbcode.split-dwarf=false",
        ],
        &[
            "--config=fbcode.use_link_groups_in_dev=False",
            "--config=fbcode.use_link_groups_in_dev=True",
            "--config=fbcode.split-dwarf=true",
            "--config=fbcode.split-dwarf=false",
        ],
    ];
    for config in all_configs_to_test {
        let buck_config = execute_test_with_args(buck, tmp_path, config, FBCODE_TARGET).await?;
        assert_buck_args_config_equal(
            &buck_config,
            &json!({
                "mode": "@fbcode//mode/dev",
                "config": "fbcode.split-dwarf=false;fbcode.use_link_groups_in_dev=True",
                "host": "linux",
            }),
        );
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_modifier(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let all_configs_to_test: &[&[&str]] = &[
        &["--modifier", "dev"],
        &["--modifier=dev"],
        &["-m", "dev"],
        &["-mdev"],
    ];
    for config in all_configs_to_test {
        let buck_config = execute_test_with_args(buck, tmp_path, config, FBCODE_TARGET).await?;
        assert_buck_args_config_equal(
            &buck_config,
            &json!({
                "mode": "@fbcode//mode/dev",
                "config": "",
                "host": "linux",
                "modifier": "dev",
            }),
        );
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_modifier_deduplication(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let all_configs_to_test: &[&[&str]] = &[&["-m", "dev", "-m", "opt", "-m", "dev"]];
    for config in all_configs_to_test {
        let buck_config = execute_test_with_args(buck, tmp_path, config, FBCODE_TARGET).await?;
        assert_buck_args_config_equal(
            &buck_config,
            &json!({
                "mode": "@fbcode//mode/dev",
                "config": "",
                "host": "linux",
                "modifier": "opt;dev",
            }),
        );
    }
    Ok(())
}
