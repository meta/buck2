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

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use buck2_e2e::BuckError;
use buck2_e2e::BuckOutput;
use buck2_e2e::BuckProcess;
use buck2_e2e::prelude::*;
use buck2_e2e::process::Signal;
use buck2_e2e::process::daemon_is_alive;
use buck2_e2e::process::send_signal;
use common_unidaemon::THIRTY_SECONDS;
use common_unidaemon::assert_daemon_exits;
use common_unidaemon::bool_cases;
use common_unidaemon::cases;
use common_unidaemon::daemon_pid;
use common_unidaemon::debug_tenants;
use common_unidaemon::generation_of;
use common_unidaemon::isolated_buck;
use common_unidaemon::isolation_of;
use common_unidaemon::isolations;
use common_unidaemon::path_str;
use common_unidaemon::pid_of;
use common_unidaemon::proxy_slot;
use common_unidaemon::start_generation;
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

fn remove_file_if_exists(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => Ok(result?),
    }
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

/// Waits up to 30 seconds for `path` to be gone.
async fn wait_for_missing_path(path: &Path) -> anyhow::Result<()> {
    tokio::time::timeout(THIRTY_SECONDS, async {
        while path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .with_context(|| format!("Timed out waiting for {} to go", path.display()))
}

/// Waits up to 3 seconds for `path`.
async fn wait_for_path(path: &Path) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .with_context(|| format!("Timed out waiting for {}", path.display()))
}

/// Whether the process `pid` has exited: it is gone, or a zombie not yet reaped by its parent.
fn process_terminated(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => {
            let state = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next());
            matches!(state, Some("Z" | "X"))
        }
        Err(_) => true,
    }
}

async fn wait_for_terminated(pid: u32, timeout: Duration) -> anyhow::Result<()> {
    tokio::time::timeout(timeout, async {
        while !process_terminated(pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .with_context(|| format!("Process {pid} did not terminate within {timeout:?}"))
}

/// The daemon's record of `tenant`, read through `via`, a client of the same daemon.
async fn tenant_entry_via(via: &Buck, tenant: &Buck) -> anyhow::Result<Option<Value>> {
    let isolation = tenant.isolation_prefix().unwrap_or("v2");
    let entries = debug_tenants(via).await?;
    for entry in entries {
        if isolation_of(&entry)? == isolation
            && entry["project_root"].as_str() == Some(path_str(tenant.cwd()))
        {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

async fn tenant_generation_via(via: &Buck, tenant: &Buck) -> anyhow::Result<u64> {
    let entry = tenant_entry_via(via, tenant)
        .await?
        .with_context(|| format!("{} has no tenant in the daemon", tenant.cwd().display()))?;
    generation_of(&entry)
}

/// Waits until `tenant` has a generation above `previous` whose proxy is published and alive,
/// asking the daemon through `via`, a sibling tenant that is not being replaced.
async fn wait_for_proxy_generation(
    via: &Buck,
    tenant: &Buck,
    previous: u64,
) -> anyhow::Result<u64> {
    let path = proxy_slot(tenant).join("buckd.info");
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(entry) = tenant_entry_via(via, tenant).await?
                && let Some(pid) = slot_pid(&path)
                && generation_of(&entry)? > previous
                && !process_terminated(pid)
            {
                return generation_of(&entry);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("Timed out waiting for the tenant's new proxy")?
}

/// The rows of the namespace's `unidaemon-events.jsonl`.
fn unidaemon_events(tenant: &Buck) -> anyhow::Result<Vec<Value>> {
    let log = proxy_slot(tenant)
        .parent()
        .context("the slot has a parent")?
        .join("unidaemon-events.jsonl");
    std::fs::read_to_string(&log)
        .with_context(|| format!("Reading {}", log.display()))?
        .lines()
        .map(|row| Ok(serde_json::from_str(row)?))
        .collect()
}

fn has_event(events: &[Value], event: &str, generation: Option<u64>) -> bool {
    events.iter().any(|row| {
        row["event"].as_str() == Some(event)
            && generation.is_none_or(|generation| row["generation"].as_u64() == Some(generation))
    })
}

fn pid_file(path: &Path) -> anyhow::Result<u32> {
    Ok(std::fs::read_to_string(path)?.trim().parse()?)
}

fn kill_process(pid: u32, signal: Signal) -> anyhow::Result<()> {
    send_signal(pid, signal)
}

/// A client of the released buck2 the test target supplies as `BUCK2_TEST_LEGACY_BINARY`, routed
/// without sharing, for the tenant's checkout and isolation.
fn legacy_buck(tenant: &Buck, executable: &Path) -> Buck {
    let mut legacy = tenant.clone();
    legacy.set_executable(executable);
    legacy.set_env("BUCK2_UNIDAEMON", "false");
    legacy.set_env("BUCK2_SHARE_DAEMON", "false");
    legacy
}

/// The released buck2 the test target supplies as `BUCK2_TEST_LEGACY_BINARY`, as the harness
/// gives it to `buck`'s commands: an absolute path. The target passes a DotSlash file, which is
/// not itself executable, so run it through `dotslash`.
fn legacy_executable(buck: &Buck, tmp_path: &Path) -> anyhow::Result<PathBuf> {
    let path = PathBuf::from(
        buck.env_var("BUCK2_TEST_LEGACY_BINARY")
            .context("the test target supplies BUCK2_TEST_LEGACY_BINARY")?,
    );
    if is_executable(&path) {
        return Ok(path);
    }
    let wrapper = tmp_path.join("legacy-buck2");
    std::fs::write(
        &wrapper,
        format!("#!/bin/sh\nexec dotslash \"{}\" \"$@\"\n", path.display()),
    )?;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))?;
    Ok(wrapper)
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

fn set_gate(tenants: &mut [&mut Buck], variable: &str, isolation: &str, path: &Path) {
    for tenant in tenants {
        tenant.set_env(variable, format!("{isolation},{}", path.display()));
    }
}

/// A proxy that crashes or loses its liveness link is replaced by a fresh generation; one
/// stopped with a signal, as `buck2 killall` does, retires its tenant and clears its slot,
/// and the next command registers the tenant again.
async fn unidaemon_proxy_loss_retires_then_recovers_generation(
    buck: &Buck,
    tmp_path: &Path,
    failure: &str,
    share_dice: bool,
) -> anyhow::Result<()> {
    let mut tenants = unidaemon_checkouts(buck, tmp_path, share_dice)?;
    let mut victim = isolated_buck(&tenants[0], "crashing");
    let gate = tmp_path.join("eviction");
    let close = tmp_path.join("close-monitor");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(
            &mut all,
            "BUCK2_TEST_UNIDAEMON_EVICTION_GATE",
            "crashing",
            &gate,
        );
        set_gate(
            &mut all,
            "BUCK2_TEST_PROXY_CLOSE_LIVENESS",
            "crashing",
            &close,
        );
    }
    let mut writer = None;
    let result = async {
        tenants[1].build(&["//:tenant_value"]).await?;
        victim.build(&["//:tenant_value"]).await?;
        let daemon = daemon_pid(&victim).await?;
        let slot = proxy_slot(&victim);
        let old = proxy_pid(&victim)?;
        let generation = tenant_generation_via(&tenants[1], &victim).await?;
        assert!(has_event(
            &unidaemon_events(&victim)?,
            "proxy-monitor-attached",
            Some(generation)
        ));
        writer = Some(start_generation(&victim, tmp_path, "old")?);
        wait_for_marker(&tmp_path.join("old.started"), &mut writer).await?;
        let writer_pid = pid_file(&tmp_path.join("old.started.pid"))?;
        match failure {
            "crash" => kill_process(old, Signal::Abort)?,
            "killed" => kill_process(old, Signal::Kill)?,
            _ => touch(&close)?,
        }
        wait_for_path(&tmp_path.join("eviction.started")).await?;
        wait_for_evicted_generation(&tenants[1], "crashing", generation).await?;
        wait_for_terminated(writer_pid, Duration::from_secs(3)).await?;
        if failure == "socket-eof" {
            assert!(
                !process_terminated(old),
                "Live proxy exited before its tenant drained"
            );
        }
        let entries = debug_tenants(&tenants[1]).await?;
        assert!(!isolations(&entries)?.contains(&"crashing"));
        remove_file_if_exists(&close)?;
        touch(&tmp_path.join("eviction.release"))?;
        if failure == "killed" {
            for name in ["buckd.info", "buckd.pid", "proxy-spec.json"] {
                wait_for_missing_path(&slot.join(name)).await?;
            }
            victim.build(&["//:tenant_value"]).await?;
            assert!(tenant_generation_via(&tenants[1], &victim).await? > generation);
        } else {
            wait_for_proxy_generation(&tenants[1], &victim, generation).await?;
        }
        assert_ne!(proxy_pid(&victim)?, old);
        victim.build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    remove_file_if_exists(&close)?;
    touch(&tmp_path.join("eviction.release"))?;
    touch(&tmp_path.join("old.release"))?;
    finish(writer).await;
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

cases! { [buck, tmp_path] unidaemon_proxy_loss_retires_then_recovers_generation {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_proxy_loss_retires_then_recovers_generation_False_crash("crash", false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_proxy_loss_retires_then_recovers_generation_False_socket_eof("socket-eof", false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_proxy_loss_retires_then_recovers_generation_False_killed("killed", false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_proxy_loss_retires_then_recovers_generation_True_crash("crash", true);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_proxy_loss_retires_then_recovers_generation_True_socket_eof("socket-eof", true);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_proxy_loss_retires_then_recovers_generation_True_killed("killed", true);
}}

async fn unidaemon_loss_recovery_respects_legacy_slot_owner(
    buck: &Buck,
    tmp_path: &Path,
    operation: &str,
) -> anyhow::Result<()> {
    let executable = legacy_executable(buck, tmp_path)?;
    let mut tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let mut victim = isolated_buck(&tenants[0], "crashing");
    let gate = tmp_path.join("eviction");
    let close = tmp_path.join("close-monitor");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(
            &mut all,
            "BUCK2_TEST_UNIDAEMON_EVICTION_GATE",
            "crashing",
            &gate,
        );
        set_gate(
            &mut all,
            "BUCK2_TEST_PROXY_CLOSE_LIVENESS",
            "crashing",
            &close,
        );
    }
    let legacy = legacy_buck(&victim, &executable);
    let mut transition = None;
    let result = async {
        tenants[1].build(&["//:tenant_value"]).await?;
        victim.build(&["//:tenant_value"]).await?;
        let daemon = daemon_pid(&victim).await?;
        let generation = tenant_generation_via(&tenants[1], &victim).await?;
        touch(&close)?;
        wait_for_path(&tmp_path.join("eviction.started")).await?;
        transition = Some(if operation == "clean" {
            legacy.clean(&[]).spawn()
        } else {
            legacy.build(&["//:tenant_value"]).spawn()
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(transition.as_mut().unwrap().try_wait().is_none());
        remove_file_if_exists(&close)?;
        touch(&tmp_path.join("eviction.release"))?;
        wait_within(transition.take().unwrap(), Duration::from_secs(15)).await??;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let entries = debug_tenants(&tenants[1]).await?;
        assert!(!isolations(&entries)?.contains(&"crashing"));
        if operation == "clean" {
            assert!(!proxy_slot(&victim).join("buckd.info").exists());
            assert!(!victim.cwd().join("buck-out/crashing/cache").exists());
        } else {
            let owner = daemon_pid(&legacy).await?;
            assert_ne!(owner, daemon);
            assert_eq!(proxy_pid(&victim)?, owner);
        }
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        assert!(generation > 0);
        anyhow::Ok(())
    }
    .await;
    remove_file_if_exists(&close)?;
    touch(&tmp_path.join("eviction.release"))?;
    finish(transition).await;
    let killed = kill_all([&legacy, &victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

cases! { [buck, tmp_path] unidaemon_loss_recovery_respects_legacy_slot_owner {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_loss_recovery_respects_legacy_slot_owner_clean("clean");
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_loss_recovery_respects_legacy_slot_owner_startup("startup");
}}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_proxy_death_before_forwarding_kill(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mut tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let mut victim = isolated_buck(&tenants[0], "crashing");
    let gate = tmp_path.join("before-forward");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(&mut all, "BUCK2_TEST_PROXY_BEFORE_EVICT", "crashing", &gate);
    }
    let mut killing = None;
    let result = async {
        tenants[1].build(&["//:tenant_value"]).await?;
        victim.build(&["//:tenant_value"]).await?;
        let daemon = daemon_pid(&victim).await?;
        let pid = proxy_pid(&victim)?;
        killing = Some(victim.kill().spawn());
        wait_for_path(&tmp_path.join("before-forward.started")).await?;
        kill_process(pid, Signal::Kill)?;
        let _outcome = wait_within(killing.take().unwrap(), Duration::from_secs(10)).await?;
        wait_for_path(
            &proxy_slot(&victim)
                .parent()
                .unwrap()
                .join("unidaemon-events.jsonl"),
        )
        .await?;
        tokio::time::timeout(Duration::from_secs(3), async {
            while isolations(&debug_tenants(&tenants[1]).await?)?.contains(&"crashing") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            anyhow::Ok(())
        })
        .await
        .context("The tenant was not retired")??;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("before-forward.release"))?;
    finish(killing).await;
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_later_eof_does_not_extend_eviction_deadline(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mut tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let mut victim = isolated_buck(&tenants[0], "crashing");
    let gate = tmp_path.join("eviction");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(
            &mut all,
            "BUCK2_TEST_UNIDAEMON_EVICTION_GATE",
            "crashing",
            &gate,
        );
    }
    let mut daemon = None;
    let result = async {
        tenants[1].build(&["//:tenant_value"]).await?;
        victim.build(&["//:tenant_value"]).await?;
        let pid = daemon_pid(&victim).await?;
        daemon = Some(pid);
        let generation = tenant_generation_via(&tenants[1], &victim).await?;
        victim
            .debug(&[
                "tenants",
                "--evict-generation",
                &generation.to_string(),
                "--eviction-timeout-ms",
                "2000",
                "--no-wait",
            ])
            .await?;
        // The deadline counts from the eviction's start in the daemon, not from the client
        // command that asked for it, which can take a while to start under load.
        wait_for_path(&tmp_path.join("eviction.started")).await?;
        let start = Instant::now();
        tokio::time::sleep(Duration::from_millis(700)).await;
        kill_process(proxy_pid(&victim)?, Signal::Kill)?;
        wait_for_terminated(pid, Duration::from_secs(3)).await?;
        assert!(
            start.elapsed() < Duration::from_millis(3500),
            "EOF extended the original eviction deadline"
        );
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("eviction.release"))?;
    if let Some(daemon) = daemon
        && !process_terminated(daemon)
    {
        let _ignored = kill_process(daemon, Signal::Kill);
    }
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

async fn unidaemon_migrates_same_slot_between_configurations(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, share_dice)?;
    let config = tenants[0].cwd().join(".buckconfig");
    let original = std::fs::read_to_string(&config)?;
    let result = async {
        for tenant in &tenants {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let first = daemon_pid(&tenants[1]).await?;
        std::fs::write(
            &config,
            format!("{original}\n[buck2]\nshare_dice = {}\n", !share_dice),
        )?;
        tenants[0].build(&["//:tenant_value"]).await?;
        let second = daemon_pid(&tenants[0]).await?;
        assert_ne!(first, second);
        tenants[1].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[1]).await?, first);
        std::fs::write(&config, &original)?;
        tenants[0].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[0]).await?, first);
        assert_daemon_exits(second).await;
        assert_eq!(namespace_keys(tmp_path)?.len(), 1);
        anyhow::Ok(())
    }
    .await;
    std::fs::write(&config, &original)?;
    let killed = kill_all(&tenants).await;
    result?;
    killed
}

bool_cases! { [buck, tmp_path] unidaemon_migrates_same_slot_between_configurations {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_migrates_same_slot_between_configurations_False(false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_migrates_same_slot_between_configurations_True(true);
}}

async fn unidaemon_registration_waits_for_clean(
    buck: &Buck,
    background: bool,
) -> anyhow::Result<()> {
    let tmp_path = buck.cwd().parent().unwrap().join("sharing");
    std::fs::create_dir(&tmp_path)?;
    let mut tenants = unidaemon_checkouts(buck, &tmp_path, false)?;
    let mut victim = isolated_buck(&tenants[0], "evicting");
    let gate = tmp_path.join("eviction");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(
            &mut all,
            "BUCK2_TEST_UNIDAEMON_EVICTION_GATE",
            "evicting",
            &gate,
        );
    }
    let mut cleaning = None;
    let mut creating = None;
    let result = async {
        for tenant in [&victim, &tenants[1]] {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let daemon = daemon_pid(&victim).await?;
        let sentinel = victim.cwd().join("buck-out/evicting/preserved");
        std::fs::write(&sentinel, "old generation")?;
        let clean_args: &[&str] = if background { &["--background"] } else { &[] };
        cleaning = Some(victim.clean(clean_args).spawn());
        wait_for_path(&tmp_path.join("eviction.started")).await?;
        creating = Some(victim.build(&["//:tenant_value"]).spawn());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(cleaning.as_mut().unwrap().try_wait().is_none());
        assert!(creating.as_mut().unwrap().try_wait().is_none());
        assert!(sentinel.exists());
        touch(&tmp_path.join("eviction.release"))?;
        wait_within(cleaning.take().unwrap(), Duration::from_secs(10)).await??;
        wait_within(creating.take().unwrap(), Duration::from_secs(10)).await??;
        assert!(!sentinel.exists());
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("eviction.release"))?;
    finish(cleaning).await;
    finish(creating).await;
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

bool_cases! { [buck] unidaemon_registration_waits_for_clean {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_registration_waits_for_clean_False(false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_registration_waits_for_clean_True(true);
}}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_stale_eviction_cannot_remove_replacement(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let result = async {
        for tenant in &tenants {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let old = tenant_generation_via(&tenants[0], &tenants[0]).await?;
        tenants[0].kill().await?;
        tenants[0].build(&["//:tenant_value"]).await?;
        let current = tenant_generation_via(&tenants[0], &tenants[0]).await?;
        assert!(current > old);
        let result = tenants[0]
            .debug(&["tenants", "--evict-generation", &old.to_string()])
            .await?;
        let evicted: Value = serde_json::from_str(&result.stdout)?;
        assert_eq!(evicted["generation"].as_u64(), Some(old));
        tenants[0].build(&["//:tenant_value"]).await?;
        assert_eq!(
            tenant_generation_via(&tenants[0], &tenants[0]).await?,
            current
        );
        anyhow::Ok(())
    }
    .await;
    let killed = kill_all(&tenants).await;
    result?;
    killed
}

async fn unidaemon_eviction_fallback_terminates_writers(
    buck: &Buck,
    failure: &str,
) -> anyhow::Result<()> {
    let tmp_path = buck.cwd().parent().unwrap().join("sharing");
    std::fs::create_dir(&tmp_path)?;
    let mut tenants = unidaemon_checkouts(buck, &tmp_path, false)?;
    let mut victim = isolated_buck(&tenants[0], "evicting");
    let gate = tmp_path.join("eviction");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(
            &mut all,
            "BUCK2_TEST_UNIDAEMON_EVICTION_GATE",
            "evicting",
            &gate,
        );
    }
    tenants[1].build(&["//:tenant_value"]).await?;
    victim.build(&["//:tenant_value"]).await?;
    let daemon = daemon_pid(&victim).await?;
    let proxy = proxy_pid(&victim)?;
    let cleaner = &victim;
    let mut writer = Some(start_generation(&tenants[1], &tmp_path, "old")?);
    let mut cleaning = None;
    let result = async {
        wait_for_marker(&tmp_path.join("old.started"), &mut writer).await?;
        let writer_pid = pid_file(&tmp_path.join("old.started.pid"))?;
        if failure == "unresponsive-daemon" {
            kill_process(daemon, Signal::Stop)?;
        }
        let start = Instant::now();
        cleaning = Some(cleaner.clean(&[]).spawn());
        if failure != "unresponsive-daemon" {
            tokio::time::timeout(Duration::from_secs(3), async {
                while !tmp_path.join("eviction.started").exists() {
                    if cleaning.as_mut().unwrap().try_wait().is_some() {
                        cleaning.take().unwrap().wait().await?;
                        anyhow::bail!("Clean completed without arming eviction");
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                anyhow::Ok(())
            })
            .await
            .context("Clean did not arm the eviction")??;
        }
        if failure == "proxy-death" {
            kill_process(proxy, Signal::Kill)?;
        }
        let outcome =
            tokio::time::timeout(Duration::from_secs(8), cleaning.take().unwrap().wait()).await;
        let Ok(outcome) = outcome else {
            eprintln!(
                "daemon_alive={} proxy_alive={} writer_alive={} root={}",
                daemon_is_alive(daemon),
                daemon_is_alive(proxy),
                daemon_is_alive(writer_pid),
                tmp_path.display()
            );
            if let Ok(events) = unidaemon_events(&victim) {
                eprintln!("{events:#?}");
            }
            anyhow::bail!("Legacy clean did not finish within 8s");
        };
        outcome?;
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(10),
            "Legacy clean did not finish: {elapsed:?}"
        );
        if failure != "proxy-death" {
            assert!(
                process_terminated(writer_pid),
                "Legacy clean returned with a live output writer"
            );
        }
        assert_daemon_exits(daemon).await;
        wait_for_terminated(writer_pid, Duration::from_secs(3)).await?;
        finish(writer.take()).await;
        assert!(!proxy_slot(&victim).join("buckd.info").exists());
        assert!(!victim.cwd().join("buck-out/evicting/cache").exists());
        let events = unidaemon_events(&victim)?;
        assert!(has_event(&events, "eviction-start", None));
        if failure != "proxy-death" {
            let exits: Vec<&Value> = events
                .iter()
                .filter(|row| {
                    matches!(
                        row["event"].as_str(),
                        Some("forced-daemon-exit" | "daemon-exit")
                    )
                })
                .collect();
            assert!(
                !exits.is_empty(),
                "Missing verified daemon termination telemetry"
            );
            assert!(
                exits
                    .iter()
                    .all(|row| row["elapsed_ms"].as_u64().is_some_and(|ms| ms < 4000)),
                "{exits:?}"
            );
        }
        touch(&tmp_path.join("eviction.release"))?;
        for tenant in [&victim, &tenants[1]] {
            tenant.build(&["//:tenant_value"]).await?;
            assert_ne!(daemon_pid(tenant).await?, daemon);
        }
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("eviction.release"))?;
    touch(&tmp_path.join("old.release"))?;
    if daemon_is_alive(daemon) {
        let _ignored = kill_process(daemon, Signal::Continue);
    }
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    if let Some(writer) = writer.take() {
        let _ignored = writer.kill().await;
    }
    if let Some(cleaning) = cleaning.take() {
        let _ignored = cleaning.kill().await;
    }
    result?;
    killed
}

cases! { [buck] unidaemon_eviction_fallback_terminates_writers {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_eviction_fallback_terminates_writers_deadline("deadline");
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_eviction_fallback_terminates_writers_proxy_death("proxy-death");
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_eviction_fallback_terminates_writers_unresponsive_daemon("unresponsive-daemon");
}}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_legacy_client_build_kill_clean_and_return(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let executable = legacy_executable(buck, tmp_path)?;
    let tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let legacy = legacy_buck(&tenants[0], &executable);
    let result = async {
        for tenant in &tenants {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let daemon = daemon_pid(&tenants[1]).await?;
        legacy.kill().await?;
        tenants[1].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        tenants[0].build(&["//:tenant_value"]).await?;
        legacy.clean(&[]).await?;
        assert!(!tenants[0].cwd().join("buck-out/tenant/cache").exists());
        tenants[0].build(&["//:tenant_value"]).await?;
        legacy.build(&["//:tenant_value"]).await?;
        let legacy_pid = daemon_pid(&legacy).await?;
        assert_ne!(legacy_pid, daemon);
        tenants[0].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[0]).await?, daemon);
        assert!(!daemon_is_alive(legacy_pid));
        tenants[1].build(&["//:tenant_value"]).await?;
        anyhow::Ok(())
    }
    .await;
    let killed = kill_all([&legacy, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

async fn unidaemon_final_tenant_can_restart_immediately(
    buck: &Buck,
    tmp_path: &Path,
    operation: &str,
) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    let result = async {
        tenant.build(&["//:tenant_value"]).await?;
        let old = daemon_pid(&tenant).await?;
        if operation == "kill" {
            tenant.kill().await?;
        } else {
            tenant.clean(&[]).await?;
        }
        assert!(process_terminated(old));
        tenant.build(&["//:tenant_value"]).await?;
        assert_ne!(daemon_pid(&tenant).await?, old);
        anyhow::Ok(())
    }
    .await;
    let killed = tenant.kill().await;
    result?;
    killed?;
    Ok(())
}

cases! { [buck, tmp_path] unidaemon_final_tenant_can_restart_immediately {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_final_tenant_can_restart_immediately_kill("kill");
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_final_tenant_can_restart_immediately_clean("clean");
}}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_failed_proxy_publication_releases_generation(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let victim = isolated_buck(&tenants[0], "failed");
    tenants[1].build(&["//:tenant_value"]).await?;
    let daemon = daemon_pid(&tenants[1]).await?;
    let slot = proxy_slot(&victim);
    std::fs::create_dir_all(&slot)?;
    let spec = slot.join("proxy-spec.json");
    std::fs::create_dir(&spec)?;
    let result = async {
        victim
            .build(&["//:tenant_value"])
            .await
            .expect_failure()
            .assert_stderr_matches("Launching tenant proxy");
        assert!(!slot.join("buckd.info").exists());
        let entries = debug_tenants(&tenants[1]).await?;
        assert!(!isolations(&entries)?.contains(&"failed"));
        std::fs::remove_dir(&spec)?;
        victim.build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&victim).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    if spec.is_dir() {
        std::fs::remove_dir(&spec)?;
    }
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_repo_killall_preserves_other_checkout(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let sibling = isolated_buck(&tenants[0], "sibling");
    let result = async {
        for tenant in [&tenants[0], &tenants[1], &sibling] {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let daemon = daemon_pid(&tenants[1]).await?;
        tenants[0].command(&["killall", "--repo"]).await?;
        for tenant in [&tenants[0], &sibling] {
            assert!(!proxy_slot(tenant).join("buckd.info").exists());
        }
        tenants[1].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    let killed = kill_all([&tenants[0], &tenants[1], &sibling]).await;
    result?;
    killed
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_concurrent_first_commands_share_a_generation(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    let result = async {
        let builds: Vec<BuckProcess> = (0..4)
            .map(|_| tenant.build(&["//:tenant_value"]).spawn())
            .collect();
        for outcome in futures::future::join_all(builds.into_iter().map(BuckProcess::wait)).await {
            outcome?;
        }
        tenant.build(&["//:tenant_value"]).await?;
        let entries = debug_tenants(&tenant).await?;
        assert_eq!(entries.len(), 1, "{entries:?}");
        anyhow::Ok(())
    }
    .await;
    let killed = tenant.kill().await;
    result?;
    killed?;
    Ok(())
}
