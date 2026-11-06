/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;
use std::path::Path;

use anyhow::Context;
use buck2_e2e::golden::golden_replace_temp_path;
use buck2_e2e::json::Json;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use serde_json::Value;

const PREVIOUS_COMMAND_WITH_MISMATCHED_CONFIG: &[&str] = &[
    "Event",
    "data",
    "Instant",
    "data",
    "PreviousCommandWithMismatchedConfig",
];

fn extra_external_config(buck: &Buck) -> String {
    buck.env_var("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG")
        .expect("the test fixture sets `BUCK2_TEST_EXTRA_EXTERNAL_CONFIG`")
        .to_string_lossy()
        .into_owned()
}

/// Whether a config value is the `section.key = value` given, and whether it came from the CLI.
#[track_caller]
fn assert_config_value(value: &Value, section: &str, key: &str, val: &str, is_cli: bool) {
    assert!(
        value["section"] == section
            && value["key"] == key
            && value["value"] == val
            && value
                .get("is_cli")
                .is_some_and(|v| v.as_bool().unwrap_or(false) == is_cli),
        "{value}"
    );
}

#[buck_test(extra_buck_config = [(
    "external_path_configs_section",
    "external_path_configs_key",
    "external_path_configs_value"
)])]
async fn test_external_buckconfigs(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let config_file = tmp_path.join("config_file");
    std::fs::write(&config_file, "[test_section]\ntest_key = test_value\n")?;
    let config_file = config_file.to_str().unwrap();
    buck.build(&[
        "@root//mode/my_mode",
        "//:test",
        "-c",
        "my_section.my_key=my_value",
        "--config-file",
        config_file,
    ])
    .await?;
    // Make a spurious file change to trigger DICE updater state comparison
    std::fs::write(buck.cwd().join("src"), "test")?;

    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "[local_section]\nlocal_key = local_value\n<file:included.bcfg>\n",
    )?;
    buck.build(&[
        "@root//mode/my_mode",
        "//:test",
        "-c",
        "my_section.my_key=my_value",
        "--config-file",
        config_file,
    ])
    .await?;

    let buckconfig_input_values = filter_events(
        buck,
        &[
            "Event",
            "data",
            "Instant",
            "data",
            "BuckconfigInputValues",
            "components",
        ],
    )
    .await?;

    assert_eq!(buckconfig_input_values.len(), 1);
    let external_configs = buckconfig_input_values[0].as_array().unwrap();

    assert_eq!(external_configs.len(), 5);
    // The order is important here. We first have the buckconfig values from external sources
    let external_path_configs = &external_configs[0]["data"]["GlobalExternalConfigFile"];
    assert_eq!(
        external_path_configs["origin_path"],
        extra_external_config(buck).as_str()
    );
    let values = external_path_configs["values"].as_array().unwrap();
    assert_eq!(values.len(), 3);

    // Our tests inject file_watcher and readonly_outputs to external configs in test setup
    // stage
    assert_config_value(
        &values[0],
        "buck2",
        "file_watcher",
        "fs_hash_crawler",
        false,
    );
    assert_config_value(&values[1], "buck2", "readonly_outputs", "true", false);
    assert_config_value(
        &values[2],
        "external_path_configs_section",
        "external_path_configs_key",
        "external_path_configs_value",
        false,
    );

    // Next comes the values from the buckconfig.local file (which may include other files https://fburl.com/wd54jnpu)
    let local_path_configs = &external_configs[1]["data"]["GlobalExternalConfigFile"];
    let values = local_path_configs["values"].as_array().unwrap();
    assert_eq!(values.len(), 2);
    assert_eq!(local_path_configs["origin_path"], ".buckconfig.local");
    // Note that buck parses configfiles ordered by section: https://fburl.com/rnzlt05n
    // That's why, we first have the values from the included file,
    assert_config_value(
        &values[0],
        "included_section",
        "included_key",
        "included_value",
        false,
    );
    // The second one is for the values in the .buckconfig.local file
    assert_config_value(
        &values[1],
        "local_section",
        "local_key",
        "local_value",
        false,
    );
    // The rest matches the same order provided by the above buck command
    // i.e. modefile, command line config flag, followed by the config file

    // We only store the path of the modefile
    assert_eq!(
        external_configs[2]["data"]["ConfigFile"]["data"]["ProjectRelativePath"],
        "my_mode.bcfg"
    );

    // Then comes the config flag from the cli
    let config_flag = &external_configs[3]["data"]["ConfigValue"];
    assert_config_value(config_flag, "my_section", "my_key", "my_value", true);
    let config_file_config =
        &external_configs[4]["data"]["ConfigFile"]["data"]["GlobalExternalConfig"];

    // Finally, the config file whose values are marked as coming from cli too
    let config_file_value = &config_file_config["values"][0];
    assert_config_value(
        config_file_value,
        "test_section",
        "test_key",
        "test_value",
        true,
    );
    assert!(
        config_file_config["origin_path"]
            .as_str()
            .unwrap_or_default()
            .contains(config_file),
        "Origin path should contain config-file name: {config_file}"
    );
    Ok(())
}

#[buck_test(
    extra_buck_config = [(
        "external_path_configs_section",
        "external_path_configs_key",
        "external_path_configs_value"
    )],
    write_invocation_record = true,
)]
async fn test_previous_command_with_mismatched_config(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "@root//mode/my_mode",
        "//:test",
        "-c",
        "my_section.my_key=my_value",
    ])
    .await?;
    let previous_invalidating_command =
        filter_events(buck, PREVIOUS_COMMAND_WITH_MISMATCHED_CONFIG).await?;
    // No previous command, no PreviousCommandWithMismatchedConfig fired
    assert_eq!(previous_invalidating_command.len(), 0);

    // Rerun without any changes
    let res = buck
        .build(&[
            "@root//mode/my_mode",
            "//:test",
            "-c",
            "my_section.my_key=my_value",
        ])
        .await?;
    let trace_id = serde_json::from_str::<Value>(&res.stdout)?
        .get("trace_id")
        .cloned()
        .context("No `trace_id` in the build output")?;
    let previous_invalidating_command =
        filter_events(buck, PREVIOUS_COMMAND_WITH_MISMATCHED_CONFIG).await?;
    // No invalidation, no PreviousInvalidatingCommand fired
    assert_eq!(previous_invalidating_command.len(), 0);

    // Rerun with changes to commandline config
    buck.build(&[
        "@root//mode/my_mode",
        "//:test",
        "-c",
        "my_section.my_key=my_new_value",
    ])
    .await?;

    let previous_invalidating_command =
        filter_events(buck, PREVIOUS_COMMAND_WITH_MISMATCHED_CONFIG).await?;
    assert_eq!(previous_invalidating_command.len(), 1);
    assert_eq!(previous_invalidating_command[0]["trace_id"], trace_id);
    let sanitized_argv = &previous_invalidating_command[0]["sanitized_argv"];
    assert!(
        // sanitized_argv[0] contains the path to the buck executable which is different on every machine
        sanitized_argv[1] == "build"
            && sanitized_argv[2] == "@root//mode/my_mode"
            && sanitized_argv[3] == "//:test"
            && sanitized_argv[4] == "-c"
            && sanitized_argv[5] == "my_section.my_key=my_value",
        "{sanitized_argv}"
    );
    // Make a change to .buckconfig. It lands after this build: the Python version of this test
    // appended in a `with` block around the build, and the write stayed buffered until the block
    // closed.
    buck.build(&[
        "@root//mode/my_mode",
        "//:test",
        "-c",
        "my_section.my_key=my_new_value",
    ])
    .await?;
    std::fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(b"\n[test_section]\ntest_key = test_value\n")?;

    // Previous command didn't change any external configs but still has new_configs_used = 1 due to changes to project-relative configs.
    // We don't capture that by design at the moment.
    let res = buck
        .build(&[
            "@root//mode/my_mode",
            "//:test",
            "-c",
            "my_section.my_key=my_new_value",
        ])
        .await?;
    let trace_id = serde_json::from_str::<Value>(&res.stdout)?
        .get("trace_id")
        .cloned()
        .context("No `trace_id` in the build output")?;

    let previous_invalidating_command =
        filter_events(buck, PREVIOUS_COMMAND_WITH_MISMATCHED_CONFIG).await?;
    assert_eq!(previous_invalidating_command.len(), 0);
    let new_configs_used = &res.invocation_record()?["new_configs_used"];
    assert!(
        *new_configs_used == Value::Bool(true) || *new_configs_used == 1,
        "{new_configs_used}"
    );

    // Make a change to .buckconfig.local
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "\n[local_section]\nlocal_key = local_value\n",
    )?;
    buck.build(&[
        "@root//mode/my_mode",
        "//:test",
        "-c",
        "my_section.my_key=my_new_value",
    ])
    .await?;
    let previous_invalidating_command =
        filter_events(buck, PREVIOUS_COMMAND_WITH_MISMATCHED_CONFIG).await?;
    assert_eq!(previous_invalidating_command.len(), 1);
    assert_eq!(previous_invalidating_command[0]["trace_id"], trace_id);
    Ok(())
}

/// A line of `buck2 log external-configs`.
struct ExternalConfigsLog<'a> {
    descriptor: &'a str,
    origin: Option<&'a str>,
}

#[buck_test]
async fn test_log_external_configs(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "@root//mode/my_mode",
        "//:test",
        "-c",
        "my_section.my_key=my_value",
        "-c",
        "my_section.my_key=my_new_value",
    ])
    .await?;

    let log = buck.log(&["external-configs"]).await?;
    let external_configs: Vec<ExternalConfigsLog> = log
        .stdout
        .trim()
        .lines()
        .map(|line| {
            let e: Vec<&str> = line.split('\t').collect();
            ExternalConfigsLog {
                descriptor: e[0],          // section.key = value
                origin: e.get(2).copied(), // origin; e[1] is the cell
            }
        })
        .collect();
    let extra_external_config = extra_external_config(buck);
    let expected = [
        // Our tests inject file_watcher and readonly_outputs to external configs in test setup
        // stage
        ExternalConfigsLog {
            descriptor: "buck2.file_watcher = fs_hash_crawler",
            origin: Some(&extra_external_config),
        },
        ExternalConfigsLog {
            descriptor: "buck2.readonly_outputs = true",
            origin: Some(&extra_external_config),
        },
        ExternalConfigsLog {
            descriptor: "my_mode.bcfg",
            origin: Some("config-file"),
        },
        ExternalConfigsLog {
            descriptor: "my_section.my_key = my_value",
            origin: Some("cli"),
        },
        // We don't override but just display the input as it is provided by the cli
        ExternalConfigsLog {
            descriptor: "my_section.my_key = my_new_value",
            origin: Some("cli"),
        },
    ];
    assert_eq!(external_configs.len(), 5);

    for (s, e) in external_configs.iter().zip(&expected) {
        assert_eq!(s.descriptor, e.descriptor);
        if let (Some(s_origin), Some(e_origin)) = (s.origin, e.origin)
            && !s_origin.is_empty()
            && !e_origin.is_empty()
        {
            assert_eq!(
                e_origin, s_origin,
                "Expected origin '{e_origin}', but got '{s_origin}'"
            );
        }
    }
    Ok(())
}

#[buck_test]
async fn test_log_external_configs_json(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "@root//mode/my_mode",
        "//:test",
        "-c",
        "my_section.my_key=my_value",
        "-c",
        "my_section.my_key=my_new_value",
    ])
    .await?;

    let log = buck.log(&["external-configs", "--format", "json"]).await?;
    let external_configs = log
        .stdout
        .trim()
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;

    let extra_external_config = extra_external_config(buck);
    let tmp_path = Path::new(&extra_external_config)
        .parent()
        .unwrap()
        .to_str()
        .unwrap();
    golden_replace_temp_path(
        // The keys are sorted: `serde_json` maps are ordered.
        &Json::from(external_configs.clone()).dumps(Some(2), true),
        "events.golden.json",
        tmp_path,
    );
    Ok(())
}
