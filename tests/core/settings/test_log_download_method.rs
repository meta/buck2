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

use buck2_e2e::prelude::*;
use serde_json::Value;
use serde_json::json;

async fn log_download_method(buck: &Buck) -> anyhow::Result<Value> {
    let result = buck.status(&[]).await?;
    let status: Value = serde_json::from_str(&result.stdout)?;
    let config: Value = serde_json::from_str(
        status["daemon_constraints"]["daemon_startup_config"]
            .as_str()
            .expect("a daemon startup config"),
    )?;
    Ok(config["log_download_method"].clone())
}

fn append_to_buckconfig(buck: &Buck, text: &str) -> anyhow::Result<()> {
    std::fs::File::options()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(text.as_bytes())?;
    Ok(())
}

fn write_buckconfig_log_use_manifold(buck: &Buck, value: bool) -> anyhow::Result<()> {
    append_to_buckconfig(buck, &format!("\n[buck2]\nlog_use_manifold = {value}\n"))
}

fn write_buckconfig_log_url(buck: &Buck, value: &str) -> anyhow::Result<()> {
    append_to_buckconfig(buck, &format!("\n[buck2]\nlog_url = {value}\n"))
}

fn write_home_local_settings(buck: &Buck, settings: &str) -> anyhow::Result<()> {
    let home = buck.settings_home_dir();
    std::fs::write(home.join(".bucksettings.local.toml"), settings)?;
    Ok(())
}

#[buck_test]
async fn test_settings_override_buckconfig(buck: &Buck) -> anyhow::Result<()> {
    write_buckconfig_log_use_manifold(buck, true)?;
    write_buckconfig_log_url(buck, "abc.com")?;
    std::fs::write(
        buck.cwd().join(".bucksettings.toml"),
        "[log_download]\nlog_use_manifold = false\nlog_url = \"test.com\"\n",
    )?;

    buck.server(&[]).await?;
    assert_eq!(
        log_download_method(buck).await?,
        json!({"Curl": "test.com"})
    );
    Ok(())
}

#[buck_test]
async fn test_log_use_manifold_fallback_to_buckconfig(buck: &Buck) -> anyhow::Result<()> {
    write_buckconfig_log_use_manifold(buck, true)?;

    buck.server(&[]).await?;
    assert_eq!(log_download_method(buck).await?, "Manifold");
    Ok(())
}

#[buck_test]
async fn test_log_url_fallback_to_buckconfig(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join(".bucksettings.toml"),
        "[log_download]\nlog_use_manifold = false\n",
    )?;

    buck.server(&[]).await?;
    assert_eq!(log_download_method(buck).await?, "None");

    write_buckconfig_log_url(buck, "test.com")?;

    buck.server(&[]).await?;
    assert_eq!(
        log_download_method(buck).await?,
        json!({"Curl": "test.com"})
    );
    Ok(())
}

#[buck_test]
async fn test_home_local_settings_override_repo(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join(".bucksettings.toml"),
        "[log_download]\nlog_use_manifold = false\n",
    )?;
    write_home_local_settings(buck, "[log_download]\nlog_use_manifold = true\n")?;

    buck.server(&[]).await?;
    assert_eq!(log_download_method(buck).await?, "Manifold");
    Ok(())
}

#[buck_test]
async fn test_repo_local_settings_override_repo_and_home_local(buck: &Buck) -> anyhow::Result<()> {
    write_buckconfig_log_use_manifold(buck, false)?;
    std::fs::write(
        buck.cwd().join(".bucksettings.toml"),
        "[log_download]\nlog_url = \"repo_root\"\n",
    )?;
    write_home_local_settings(buck, "[log_download]\nlog_url = \"home_local\"\n")?;
    std::fs::write(
        buck.cwd().join(".bucksettings.local.toml"),
        "[log_download]\nlog_url = \"repo_local\"\n",
    )?;

    buck.server(&[]).await?;
    assert_eq!(
        log_download_method(buck).await?,
        json!({"Curl": "repo_local"})
    );
    Ok(())
}

#[buck_test]
async fn test_cli_settings_override_local(buck: &Buck) -> anyhow::Result<()> {
    write_home_local_settings(buck, "[log_download]\nlog_use_manifold = true\n")?;
    std::fs::write(
        buck.cwd().join(".bucksettings.local.toml"),
        "[log_download]\nlog_url = \"repo_local\"\n",
    )?;

    buck.server(&[]).await?;
    let result = buck
        .server(&[
            "--setting",
            "log_download.log_use_manifold=false",
            "--setting",
            "log_download.log_url=args",
            "--setting",
            "agent_materialization.enabled=true",
        ])
        .await?;
    assert!(result.stderr.contains(concat!(
        "Buck settings changed (log_download.log_url: \"repo_local\" -> \"args\"; ",
        "log_download.log_use_manifold: true -> false); restarting daemon..."
    )));
    assert_eq!(log_download_method(buck).await?, json!({"Curl": "args"}));
    Ok(())
}

#[buck_test]
async fn test_cli_settings_after_mode(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(buck.cwd().join("mode"), "--verbose=0\n")?;

    buck.server(&["@mode", "--setting", "log_download.log_use_manifold=true"])
        .await?;
    assert_eq!(log_download_method(buck).await?, "Manifold");
    Ok(())
}
