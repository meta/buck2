/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Daemon sharing across checkouts: clients whose startup constraints agree join one daemon.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use buck2_e2e::BuckError;
use buck2_e2e::BuckOutput;
use buck2_e2e::BuckProcess;
use buck2_e2e::prelude::*;
use buck2_e2e::process::Signal;
use buck2_e2e::process::send_signal;
use common_unidaemon::THIRTY_SECONDS;
use common_unidaemon::assert_daemon_exits;
use common_unidaemon::bool_cases;
use common_unidaemon::daemon_pid;
use common_unidaemon::debug_tenants;
use common_unidaemon::generation_of;
use common_unidaemon::isolated_buck;
use common_unidaemon::pid_of;
use common_unidaemon::proxy_slot;
use common_unidaemon::status_json;
use common_unidaemon::tenant_entry;
use common_unidaemon::tenant_generation;
use common_unidaemon::unidaemon_checkouts;
use common_unidaemon::wait_for_evicted_generation;
use common_unidaemon::wait_for_marker;
use regex::Regex;
use serde_json::Value;

/// Whether Python takes `value` as true.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn touch(path: &Path) -> anyhow::Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(())
}

/// The `pid` of the `buckd.info` at `path`, if it is there and complete.
fn slot_pid(path: &Path) -> Option<u32> {
    let info: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    u32::try_from(info["pid"].as_u64()?).ok()
}

/// The pid of the proxy published in the tenant's slot.
fn proxy_pid(tenant: &Buck) -> anyhow::Result<u32> {
    let path = proxy_slot(tenant).join("buckd.info");
    slot_pid(&path).with_context(|| format!("No proxy is published at {}", path.display()))
}

/// What the Python tests' `asyncio.wait_for(asyncio.shield(task), timeout)` gave: the command's
/// outcome, or an error if it took longer.
async fn wait_within(
    process: BuckProcess,
    timeout: Duration,
) -> anyhow::Result<Result<BuckOutput, BuckError>> {
    tokio::time::timeout(timeout, process.wait())
        .await
        .with_context(|| format!("The command did not finish within {timeout:?}"))
}

/// Lets a started command finish, whatever its outcome, as the Python tests'
/// `asyncio.gather(task, return_exceptions=True)` did.
async fn finish(process: Option<BuckProcess>) {
    if let Some(process) = process {
        let _ignored = process.wait().await;
    }
}

/// What `assertRaisesRegex(BuckException, "evicted")` checked: the command failed, saying so.
#[track_caller]
fn assert_evicted(outcome: Result<BuckOutput, BuckError>) {
    let error = outcome.expect_err("The command was expected to fail");
    assert!(error.to_string().contains("evicted"), "{error}");
}

/// Builds `//:tenant_value` in `tenant` and checks that the output holds `value` and lies in
/// the tenant's own checkout.
async fn assert_tenant_value(tenant: &Buck, value: &str) -> anyhow::Result<()> {
    let result = tenant.build(&["//:tenant_value"]).await?;
    let output = result
        .build_report()?
        .output_for_target("//:tenant_value")?;
    assert_eq!(std::fs::read_to_string(&output)?, value);
    assert!(output.starts_with(tenant.cwd()));
    Ok(())
}

fn namespace_keys(tmp_path: &Path) -> anyhow::Result<Vec<String>> {
    std::fs::read_dir(
        tmp_path
            .join("home")
            .join(".buck")
            .join("buckd")
            .join(".unidaemons"),
    )?
    .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
    .collect::<anyhow::Result<_>>()
}

async fn kill_all<'a>(tenants: impl IntoIterator<Item = &'a Buck>) -> anyhow::Result<()> {
    for tenant in tenants {
        tenant.kill().await?;
    }
    Ok(())
}

async fn unidaemon_compatibility_discovery(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, share_dice)?;
    let result = async {
        let mut pids = Vec::new();
        for (tenant, value) in tenants.iter().zip(["one", "two"]) {
            assert_tenant_value(tenant, value).await?;
            pids.push(daemon_pid(tenant).await?);
        }
        assert_eq!(pids[0], pids[1]);
        let keys = namespace_keys(tmp_path)?;
        assert_eq!(keys.len(), 1, "{keys:?}");
        assert!(Regex::new("^[0-9a-f]{64}$")?.is_match(&keys[0]), "{keys:?}");
        tenants[0].kill().await?;
        tenants[1].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[1]).await?, pids[0]);
        tenants[0].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[0]).await?, pids[0]);
        anyhow::Ok(pids[0])
    }
    .await;
    let killed = kill_all(&tenants).await;
    let pid = result?;
    killed?;
    assert_daemon_exits(pid).await;
    Ok(())
}

bool_cases! { [buck, tmp_path] unidaemon_compatibility_discovery {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_compatibility_discovery_False(false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_compatibility_discovery_True(true);
}}

async fn unidaemon_incompatible_settings_select_another_process(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, share_dice)?;
    let config = tenants[1].cwd().join(".buckconfig");
    let original = std::fs::read_to_string(&config)?;
    let alternative = isolated_buck(&tenants[1], "other-mode");
    let result = async {
        for tenant in &tenants {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let pid = daemon_pid(&tenants[0]).await?;
        std::fs::write(
            &config,
            format!("{original}\n[buck2]\nshare_dice = {}\n", !share_dice),
        )?;
        alternative.build(&["//:tenant_value"]).await?;
        assert_ne!(daemon_pid(&alternative).await?, pid);
        assert_eq!(namespace_keys(tmp_path)?.len(), 2);
        // Killed while its configuration still names its namespace: a client finds the
        // tenant's daemon by hashing the configuration it reads.
        alternative.kill().await?;
        std::fs::write(&config, &original)?;
        tenants[0].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[0]).await?, pid);
        anyhow::Ok(pid)
    }
    .await;
    let killed = async {
        alternative.kill().await?;
        std::fs::write(&config, &original)?;
        kill_all(&tenants).await
    }
    .await;
    let pid = result?;
    killed?;
    assert_daemon_exits(pid).await;
    Ok(())
}

bool_cases! { [buck, tmp_path] unidaemon_incompatible_settings_select_another_process {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_incompatible_settings_select_another_process_False(false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_incompatible_settings_select_another_process_True(true);
}}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_unawaited_final_eviction_stops_daemon(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    tenant.build(&["//:tenant_value"]).await?;
    let pid = daemon_pid(&tenant).await?;
    let entries = debug_tenants(&tenant).await?;
    let [entry] = entries.as_slice() else {
        anyhow::bail!("Expected one tenant, got {entries:?}");
    };
    tenant
        .debug(&[
            "tenants",
            "--evict-generation",
            &generation_of(entry)?.to_string(),
            "--no-wait",
        ])
        .await?;
    assert_daemon_exits(pid).await;
    Ok(())
}

/// The tenant's slot is served while its generation still initializes: the proxy there
/// describes the daemon as the tenant will see it afterwards, and another client of the
/// tenant joins the pending generation instead of starting one of its own.
#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_proxy_publishes_before_tenant_initializes(
    buck: &mut Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let gate = tmp_path.join("initialization");
    buck.set_env(
        "BUCK2_TEST_BLOCK_TENANT_INITIALIZATION",
        format!("initializing,{}", gate.display()),
    );
    let tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let victim = isolated_buck(&tenants[0], "initializing");
    let mut first = None;
    let mut second = None;
    let result = async {
        tenants[1].build(&["//:tenant_value"]).await?;
        let status = status_json(&tenants[1]).await?;
        let daemon = pid_of(&status)?;
        first = Some(victim.build(&["//:tenant_value"]).spawn());
        wait_for_marker(&tmp_path.join("initialization.started"), &mut first).await?;
        let entries = debug_tenants(&tenants[1]).await?;
        let entry = tenant_entry(&entries, "initializing")?;
        assert!(!truthy(&entry["initialized"]));
        let generation = generation_of(entry)?;
        let slot = proxy_slot(&victim);
        assert!(slot.join("buckd.info").exists());
        let spec: Value =
            serde_json::from_str(&std::fs::read_to_string(slot.join("proxy-spec.json"))?)?;
        assert_eq!(spec["reference"]["generation"].as_u64(), Some(generation));
        assert_eq!(
            spec["constraints"]["daemon_id"],
            status["daemon_constraints"]["daemon_id"]
        );
        assert!(!truthy(&spec["constraints"]["extra"]["trace_io_enabled"]));
        second = Some(victim.build(&["//:tenant_value"]).spawn());
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(second.as_mut().unwrap().try_wait().is_none());
        touch(&tmp_path.join("initialization.release"))?;
        wait_within(first.take().unwrap(), THIRTY_SECONDS).await??;
        wait_within(second.take().unwrap(), THIRTY_SECONDS).await??;
        let entries = debug_tenants(&tenants[1]).await?;
        let initializing: Vec<(u64, bool)> = entries
            .iter()
            .filter(|entry| entry["isolation"].as_str() == Some("initializing"))
            .map(|entry| Ok((generation_of(entry)?, truthy(&entry["initialized"]))))
            .collect::<anyhow::Result<_>>()?;
        assert_eq!(initializing, [(generation, true)]);
        let status = status_json(&victim).await?;
        assert_eq!(pid_of(&status)?, daemon);
        assert!(!truthy(
            &status["daemon_constraints"]["extra"]["trace_io_enabled"]
        ));
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("initialization.release"))?;
    finish(first).await;
    finish(second).await;
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_proxy_crash_during_initialization_evicts_generation(
    buck: &mut Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let gate = tmp_path.join("initialization");
    buck.set_env(
        "BUCK2_TEST_BLOCK_TENANT_INITIALIZATION",
        format!("initializing,{}", gate.display()),
    );
    let tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let victim = isolated_buck(&tenants[0], "initializing");
    let mut build = None;
    let result = async {
        tenants[1].build(&["//:tenant_value"]).await?;
        let daemon = daemon_pid(&tenants[1]).await?;
        build = Some(victim.build(&["//:tenant_value"]).spawn());
        wait_for_marker(&tmp_path.join("initialization.started"), &mut build).await?;
        let generation = tenant_generation(&debug_tenants(&tenants[1]).await?, "initializing")?;
        let slot = proxy_slot(&victim);
        let proxy = proxy_pid(&victim)?;
        send_signal(proxy, Signal::Kill)?;
        wait_for_evicted_generation(&tenants[1], "initializing", generation).await?;
        touch(&tmp_path.join("initialization.release"))?;
        assert_evicted(wait_within(build.take().unwrap(), THIRTY_SECONDS).await?);
        victim.build(&["//:tenant_value"]).await?;
        let entries = debug_tenants(&tenants[1]).await?;
        let newer: Vec<bool> = entries
            .iter()
            .filter(|entry| entry["isolation"].as_str() == Some("initializing"))
            .map(|entry| Ok(generation_of(entry)? > generation))
            .collect::<anyhow::Result<_>>()?;
        assert_eq!(newer, [true]);
        assert_ne!(slot_pid(&slot.join("buckd.info")), Some(proxy));
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("initialization.release"))?;
    finish(build).await;
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}
