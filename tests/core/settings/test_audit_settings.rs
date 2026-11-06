/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::io::Write;

use buck2_e2e::prelude::*;
use serde_json::Value;
use serde_json::json;

fn entries(report: &Value) -> BTreeMap<String, &Value> {
    report["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|section| {
            section["settings"]
                .as_array()
                .into_iter()
                .flatten()
                .map(move |setting| {
                    (
                        format!(
                            "{}.{}",
                            section["name"].as_str().unwrap_or_default(),
                            setting["name"].as_str().unwrap_or_default()
                        ),
                        setting,
                    )
                })
        })
        .collect()
}

#[buck_test]
async fn test_report_names_values_and_origins(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join(".bucksettings.toml"),
        "[analysis]\nrecord_requested_anon_targets = false\n",
    )?;
    let home_local = buck.settings_home_dir().join(".bucksettings.local.toml");
    std::fs::write(&home_local, "[log_download]\nlog_url = \"https://home/\"\n")?;
    std::fs::File::options()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(b"[buck2]\ndaemon_idle_timeout_s = 100\n")?;

    let result = buck
        .audit(&[
            "settings",
            "--json",
            "--setting",
            "agent_materialization.enabled=true",
        ])
        .await?;
    let report: Value = serde_json::from_str(&result.stdout)?;
    assert!(
        report["hash"]
            .as_str()
            .unwrap_or_default()
            .starts_with("buck2settings-")
    );

    let layers: Vec<(&Value, &Value)> = report["layers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|layer| (&layer["origin"]["kind"], &layer["applied"]))
        .collect();
    assert!(layers.contains(&(&json!("repository"), &json!(true))));
    assert!(layers.contains(&(&json!("local"), &json!(true))));
    assert!(layers.contains(&(&json!("command_line"), &json!(true))));

    let entries = entries(&report);
    let anon = entries["analysis.record_requested_anon_targets"];
    assert_eq!(anon["value"], false);
    assert_eq!(anon["origin"]["kind"], "repository");
    assert_eq!(anon["internal_default"], true);

    let log_url = entries["log_download.log_url"];
    assert_eq!(log_url["value"], "https://home/");
    assert_eq!(log_url["origin"]["kind"], "local");
    assert_eq!(
        log_url["origin"]["path"],
        home_local.to_string_lossy().as_ref()
    );

    let enabled = entries["agent_materialization.enabled"];
    assert_eq!(enabled["value"], true);
    assert_eq!(enabled["origin"]["kind"], "command_line");

    let defer = entries["hydration.defer_field_reads"];
    assert_eq!(defer["value"], true);
    assert_eq!(defer.get("origin"), Some(&Value::Null));

    // A declared fallback is read from the root buckconfig when no layer
    // sets the key.
    let idle = entries["daemon.idle_timeout_s"];
    assert_eq!(idle["value"], 100);
    assert_eq!(idle.get("set"), Some(&Value::Null));
    assert_eq!(
        idle["origin"],
        json!({
            "kind": "buckconfig",
            "path": "buck2.daemon_idle_timeout_s",
        })
    );
    assert_eq!(
        idle["buckconfig_fallback"]["key"],
        "buck2.daemon_idle_timeout_s"
    );

    let scopes: BTreeMap<&str, &Value> = report["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|section| {
            (
                section["name"].as_str().unwrap_or_default(),
                &section["scope"],
            )
        })
        .collect();
    assert_eq!(scopes["hydration"], "daemon");
    assert_eq!(scopes["agent_materialization"], "invocation");
    Ok(())
}

#[buck_test]
async fn test_text_report(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join(".bucksettings.toml"),
        "[analysis]\nrecord_requested_anon_targets = false\n",
    )?;
    let result = buck.audit(&["settings", "--verbose"]).await?;
    assert!(result.stdout.contains("Buck settings buck2settings-"));
    assert!(result.stdout.contains("Layers, lowest priority first:"));
    assert!(result.stdout.contains("[analysis]  daemon scope, version"));
    assert!(
        result
            .stdout
            .contains("record_requested_anon_targets = false")
    );
    assert!(result.stdout.contains("(repository)"));
    assert!(result.stdout.contains("defer_field_reads = true"));
    assert!(result.stdout.contains("(default)"));
    assert!(result.stdout.contains("overridable in:"));
    Ok(())
}
